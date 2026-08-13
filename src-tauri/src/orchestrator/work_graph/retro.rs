//! Evidence-bounded post-run work-graph evaluation for issue #217.
//!
//! A retro can establish that an archived topology was followed, abandoned, or
//! leaked at an observed boundary. It cannot establish that the topology was
//! optimal, better than an unobserved alternative, or causally responsible for
//! an outcome. Barrier timing therefore reports observed sibling wait and
//! release delay separately; gotcha hits report only that an informed task was
//! attempted; review escape revisions require an explicit later evidence link.
//! The evaluator is read-only. It returns request-shaped, `unreviewed` learning
//! proposals for the sanctioned session-learning endpoint and never writes a
//! template, `.ai-docs`, a learning JSONL file, or a wiki page.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::archetypes::{
    DeviationPromotionProposal, PromotionTier, GRAPH_ARCHETYPE_EXPANSION,
};
use super::archive::{
    list_archives, read_archive, ArchiveSourceKind, WorkGraphArchive,
    WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
};
use super::divergence::{DivergenceKind, DivergenceRecord};
use super::review::{
    JUDGE_PRINCE_REMEDIATION_TEMPLATE, MULTI_LENS_REVIEW_TEMPLATE,
};
use super::runtime::{GraphMutationType, RuntimeOutcome};
use super::schema::{
    EdgeKind, EdgeProvenance, NodeKind, NodeStatus, TaskGraph,
    WorkGraphOmissionReason, WorkNode,
};

pub const PROMOTION_RUN_THRESHOLD: usize = 2;
pub const REVIEW_ESCAPE_EFFECT_KIND: &str = "review_escape";
pub const UNREVIEWED_OUTCOME: &str = "unreviewed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateKey {
    pub template_id: String,
    pub template_version: u32,
}

impl PartialOrd for TemplateKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TemplateKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.template_id
            .cmp(&other.template_id)
            .then(self.template_version.cmp(&other.template_version))
    }
}

/// The archive stores the aggregation portion of `ArchetypeLineage` on each
/// archetype lane. Source and applied overrides are not reconstructed here.
fn template_key(graph: &TaskGraph) -> Result<TemplateKey, RetroOmission> {
    let mut keys = BTreeSet::new();
    let mut malformed = Vec::new();
    for node in &graph.nodes {
        let Some(expansion) = &node.expansion else {
            continue;
        };
        if expansion.template != GRAPH_ARCHETYPE_EXPANSION {
            continue;
        }
        let Some(template_id) = expansion.parameters.get("template_id") else {
            malformed.push(node.id.clone());
            continue;
        };
        let Some(version) = expansion.parameters.get("template_version") else {
            malformed.push(node.id.clone());
            continue;
        };
        match version.parse::<u32>() {
            Ok(template_version) => {
                keys.insert(TemplateKey {
                    template_id: template_id.clone(),
                    template_version,
                });
            }
            Err(_) => malformed.push(node.id.clone()),
        }
    }
    if !malformed.is_empty() {
        return Err(RetroOmission::new(
            RetroOmissionReason::TemplateLineageUnavailable,
            "template_lineage",
            "archetype lane metadata is missing a valid template id or version",
            malformed,
        ));
    }
    if keys.len() != 1 {
        return Err(RetroOmission::new(
            RetroOmissionReason::TemplateLineageUnavailable,
            "template_lineage",
            if keys.is_empty() {
                "the plan has no archived archetype lineage metadata"
            } else {
                "the plan contains conflicting archetype lineage metadata"
            },
            keys.into_iter()
                .map(|key| format!("{}@{}", key.template_id, key.template_version))
                .collect(),
        ));
    }
    Ok(keys.into_iter().next().expect("one lineage key was checked"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetroOmissionReason {
    NoArchives,
    ArchiveUnreadable,
    UnsupportedSchemaVersion,
    PlanGraphUnavailable,
    TemplateLineageUnavailable,
    EvaluatorProvenanceUnavailable,
    EventEvidenceUnavailable,
    RunLedgerEvidenceUnavailable,
    MutationEvidenceUnavailable,
    KnowledgeEvidenceUnavailable,
    TimingEvidenceIncomplete,
    ResolutionIncomplete,
    InvalidEvidence,
    NoEligibleEdges,
    LearningSubmissionFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetroOmission {
    pub reason: RetroOmissionReason,
    pub metric: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_id: Option<String>,
    #[serde(default)]
    pub examples: Vec<String>,
}

impl RetroOmission {
    pub(super) fn new(
        reason: RetroOmissionReason,
        metric: impl Into<String>,
        detail: impl Into<String>,
        examples: Vec<String>,
    ) -> Self {
        Self {
            reason,
            metric: metric.into(),
            detail: detail.into(),
            archive_id: None,
            examples: examples.into_iter().take(5).collect(),
        }
    }

    pub(super) fn for_archive(mut self, archive_id: &str) -> Self {
        self.archive_id = Some(archive_id.to_string());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum EvidenceMetric<T> {
    Available { value: T },
    Partial {
        value: T,
        omissions: Vec<RetroOmission>,
    },
    Unavailable { omissions: Vec<RetroOmission> },
}

impl<T> EvidenceMetric<T> {
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Available { value } | Self::Partial { value, .. } => Some(value),
            Self::Unavailable { .. } => None,
        }
    }

    fn value_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Available { value } | Self::Partial { value, .. } => Some(value),
            Self::Unavailable { .. } => None,
        }
    }

    fn add_omission(&mut self, omission: RetroOmission) {
        let current = std::mem::replace(
            self,
            Self::Unavailable {
                omissions: Vec::new(),
            },
        );
        *self = match current {
            Self::Available { value } => Self::Partial {
                value,
                omissions: vec![omission],
            },
            Self::Partial {
                value,
                mut omissions,
            } => {
                omissions.push(omission);
                Self::Partial { value, omissions }
            }
            Self::Unavailable { mut omissions } => {
                omissions.push(omission);
                Self::Unavailable { omissions }
            }
        };
    }

    fn omissions(&self) -> &[RetroOmission] {
        match self {
            Self::Available { .. } => &[],
            Self::Partial { omissions, .. } | Self::Unavailable { omissions } => {
                omissions
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndependentEvaluator {
    evaluator_id: String,
    planner_agent_ids: BTreeSet<String>,
    supervisor_agent_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetroError {
    MissingEvaluatorIdentity,
    EvaluatorNotIndependent(String),
}

impl fmt::Display for RetroError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEvaluatorIdentity => {
                write!(formatter, "retro evaluator identity cannot be empty")
            }
            Self::EvaluatorNotIndependent(id) => write!(
                formatter,
                "retro evaluator {id} planned or supervised an evaluated run"
            ),
        }
    }
}

impl Error for RetroError {}

impl IndependentEvaluator {
    /// The caller attests the complete planner and supervisor identity sets.
    /// Archives do not contain those roles, so independence cannot be inferred
    /// from archived execution identities alone.
    pub fn new(
        evaluator_id: impl Into<String>,
        planner_agent_ids: impl IntoIterator<Item = String>,
        supervisor_agent_ids: impl IntoIterator<Item = String>,
    ) -> Result<Self, RetroError> {
        let evaluator_id = evaluator_id.into();
        if evaluator_id.trim().is_empty() {
            return Err(RetroError::MissingEvaluatorIdentity);
        }
        let planner_agent_ids: BTreeSet<String> =
            planner_agent_ids.into_iter().collect();
        let supervisor_agent_ids: BTreeSet<String> =
            supervisor_agent_ids.into_iter().collect();
        if planner_agent_ids.contains(&evaluator_id)
            || supervisor_agent_ids.contains(&evaluator_id)
        {
            return Err(RetroError::EvaluatorNotIndependent(evaluator_id));
        }
        Ok(Self {
            evaluator_id,
            planner_agent_ids,
            supervisor_agent_ids,
        })
    }

    pub fn evaluator_id(&self) -> &str {
        &self.evaluator_id
    }
}

#[derive(Debug, Clone)]
pub struct RetroRunInput {
    pub repo_id: String,
    pub archive: WorkGraphArchive,
}

#[derive(Debug, Clone)]
pub struct RetroArchivePath {
    pub repo_id: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanActualEditDistance {
    /// Unit-cost archived structural divergences. An edge rewire is one unit.
    pub unit_edits: usize,
    pub by_kind: BTreeMap<DivergenceKind, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeExecutionMetric {
    pub node_id: String,
    /// Additional event-backed attempts beyond the first observed attempt.
    /// The frozen runtime schema increments this on claims and claim failures;
    /// it does not preserve raw reclaim events as a separate typed count.
    pub additional_attempts: Option<usize>,
    /// Structured `RemediationDetour` deltas targeted at this node.
    pub remediation_detours: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointBarrierMetric {
    pub checkpoint_id: String,
    pub prerequisite_ids: Vec<String>,
    pub ready_at: Option<DateTime<Utc>>,
    pub checkpoint_started_at: Option<DateTime<Utc>>,
    /// Sum of each faster prerequisite's observed wait for the last sibling.
    pub sibling_barrier_idle_millis: Option<i64>,
    /// Time between the last prerequisite and the checkpoint starting.
    pub gate_release_delay_millis: Option<i64>,
    /// Observed prerequisite wait through checkpoint start, including release delay.
    pub total_pre_checkpoint_wait_millis: Option<i64>,
    #[serde(default)]
    pub missing_timestamp_node_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewEvidenceState {
    Caught,
    PassedNoKnownEscape,
    Escaped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewEfficacyRevision {
    pub discovering_archive_id: String,
    pub source_ref: String,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewEfficacyMetric {
    pub verdict_id: String,
    pub target_id: String,
    pub state: ReviewEvidenceState,
    pub caught_defects: usize,
    pub escaped_defects: usize,
    pub remediation_detours: usize,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub revisions: Vec<ReviewEfficacyRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GotchaEdgeHitRate {
    pub eligible_knowledge_edges: usize,
    /// Edges whose target has an event-backed observed attempt.
    pub targets_attempted: usize,
    /// False for a 0/0 observation; no numeric rate is implied.
    pub rate_defined: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerRunRetro {
    pub repo_id: String,
    pub archive_id: String,
    pub session_id: String,
    pub archived_at: DateTime<Utc>,
    pub template: Option<TemplateKey>,
    pub edit_distance: EvidenceMetric<PlanActualEditDistance>,
    pub nodes: EvidenceMetric<Vec<NodeExecutionMetric>>,
    pub checkpoints: EvidenceMetric<Vec<CheckpointBarrierMetric>>,
    pub reviews: EvidenceMetric<Vec<ReviewEfficacyMetric>>,
    pub gotcha_edge_hit_rate: EvidenceMetric<GotchaEdgeHitRate>,
    #[serde(default)]
    pub omissions: Vec<RetroOmission>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateAggregate {
    pub template: TemplateKey,
    pub run_count: usize,
    pub observed_edit_units: Option<usize>,
    pub additional_attempts_by_node: BTreeMap<String, usize>,
    pub remediation_detours_by_node: BTreeMap<String, usize>,
    pub sibling_barrier_idle_millis_by_checkpoint: BTreeMap<String, i64>,
    pub review_efficacy: EvidenceMetric<ReviewEfficacyAggregate>,
    pub gotcha_edges_eligible: Option<usize>,
    pub gotcha_targets_attempted: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReviewEfficacyAggregate {
    pub caught_defects: usize,
    pub escaped_defects: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreviewedLearningSubmission {
    pub session: String,
    pub task: String,
    pub outcome: String,
    pub keywords: Vec<String>,
    pub insight: String,
    pub files_touched: Vec<String>,
}

impl UnreviewedLearningSubmission {
    pub fn endpoint_path(&self) -> String {
        format!("/api/sessions/{}/learnings", self.session)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetroReport {
    pub evaluator_id: String,
    pub runs: Vec<PerRunRetro>,
    pub template_aggregates: Vec<TemplateAggregate>,
    pub promotion_proposals: Vec<DeviationPromotionProposal>,
    /// Request bodies exposed for the sanctioned session-learning endpoint.
    pub learning_submissions: Vec<UnreviewedLearningSubmission>,
    #[serde(default)]
    pub omissions: Vec<RetroOmission>,
}

impl RetroReport {
    pub(super) fn unavailable(
        evaluator_id: impl Into<String>,
        omission: RetroOmission,
    ) -> Self {
        Self {
            evaluator_id: evaluator_id.into(),
            runs: Vec::new(),
            template_aggregates: Vec::new(),
            promotion_proposals: Vec::new(),
            learning_submissions: Vec::new(),
            omissions: vec![omission],
        }
    }
}

pub fn evaluate_archive_paths(
    evaluator: &IndependentEvaluator,
    paths: &[RetroArchivePath],
) -> Result<RetroReport, RetroError> {
    let mut inputs = Vec::new();
    let mut omissions = Vec::new();
    for input in paths {
        match read_archive(&input.path) {
            Ok(archive) => inputs.push(RetroRunInput {
                repo_id: input.repo_id.clone(),
                archive,
            }),
            Err(error) => {
                let rendered = error.to_string();
                let reason = if rendered.contains("unsupported schema version") {
                    RetroOmissionReason::UnsupportedSchemaVersion
                } else {
                    RetroOmissionReason::ArchiveUnreadable
                };
                omissions.push(RetroOmission::new(
                    reason,
                    "archive",
                    rendered,
                    vec![input.path.display().to_string()],
                ));
            }
        }
    }
    evaluate_inputs(evaluator, &inputs, omissions, paths.is_empty())
}

/// Discover and evaluate immutable archives beneath a completed session.
/// Directory/read failures become omissions so one corrupt corpus does not
/// manufacture an empty, clean report.
pub fn evaluate_completed_session(
    evaluator: &IndependentEvaluator,
    repo_id: &str,
    session_dir: &Path,
) -> Result<RetroReport, RetroError> {
    match list_archives(session_dir) {
        Ok(paths) => {
            let inputs = paths
                .into_iter()
                .map(|path| RetroArchivePath {
                    repo_id: repo_id.to_string(),
                    path,
                })
                .collect::<Vec<_>>();
            evaluate_archive_paths(evaluator, &inputs)
        }
        Err(error) => evaluate_inputs(
            evaluator,
            &[],
            vec![RetroOmission::new(
                RetroOmissionReason::ArchiveUnreadable,
                "archive",
                error.to_string(),
                vec![session_dir.display().to_string()],
            )],
            false,
        ),
    }
}

pub fn evaluate_archives(
    evaluator: &IndependentEvaluator,
    inputs: &[RetroRunInput],
) -> Result<RetroReport, RetroError> {
    evaluate_inputs(evaluator, inputs, Vec::new(), inputs.is_empty())
}

fn evaluate_inputs(
    evaluator: &IndependentEvaluator,
    inputs: &[RetroRunInput],
    mut omissions: Vec<RetroOmission>,
    no_candidates: bool,
) -> Result<RetroReport, RetroError> {
    // Re-check the invariant at execution time even though construction also
    // enforces it. This makes the safety boundary local to the evaluator.
    if evaluator.planner_agent_ids.contains(&evaluator.evaluator_id)
        || evaluator.supervisor_agent_ids.contains(&evaluator.evaluator_id)
    {
        return Err(RetroError::EvaluatorNotIndependent(
            evaluator.evaluator_id.clone(),
        ));
    }
    if no_candidates {
        omissions.push(RetroOmission::new(
            RetroOmissionReason::NoArchives,
            "archive",
            "no completed work-graph archives were supplied",
            Vec::new(),
        ));
    }

    let mut valid = Vec::new();
    let mut archive_ids = BTreeSet::new();
    for input in inputs {
        if input.archive.schema_version != WORK_GRAPH_ARCHIVE_SCHEMA_VERSION {
            omissions.push(
                RetroOmission::new(
                    RetroOmissionReason::UnsupportedSchemaVersion,
                    "archive",
                    format!(
                        "unsupported work-graph archive schema version {}",
                        input.archive.schema_version
                    ),
                    vec![input.archive.archive_id.clone()],
                )
                .for_archive(&input.archive.archive_id),
            );
            continue;
        }
        if !archive_ids.insert(input.archive.archive_id.clone()) {
            omissions.push(
                RetroOmission::new(
                    RetroOmissionReason::InvalidEvidence,
                    "archive",
                    "duplicate archive id was excluded from aggregation",
                    vec![input.archive.archive_id.clone()],
                )
                .for_archive(&input.archive.archive_id),
            );
            continue;
        }
        valid.push(input.clone());
    }
    valid.sort_by(|left, right| {
        left.archive
            .archived_at
            .cmp(&right.archive.archived_at)
            .then(left.archive.archive_id.cmp(&right.archive.archive_id))
    });

    let mut runs: Vec<_> = valid.iter().map(evaluate_run).collect();
    apply_review_escape_revisions(&valid, &mut runs, &mut omissions);
    let (promotion_proposals, learning_submissions) =
        systematic_divergence(&valid, &runs, &mut omissions);
    let template_aggregates = aggregate_templates(&runs);
    for run in &runs {
        omissions.extend(run.omissions.iter().cloned());
    }
    Ok(RetroReport {
        evaluator_id: evaluator.evaluator_id.clone(),
        runs,
        template_aggregates,
        promotion_proposals,
        learning_submissions,
        omissions,
    })
}

fn evaluate_run(input: &RetroRunInput) -> PerRunRetro {
    let archive = &input.archive;
    let mut run_omissions = Vec::new();
    let template = archive.plan_graph.as_ref().and_then(|plan| match template_key(plan) {
        Ok(key) => Some(key),
        Err(error) => {
            run_omissions.push(error.for_archive(&archive.archive_id));
            None
        }
    });
    if archive.plan_graph.is_none() {
        run_omissions.push(
            RetroOmission::new(
                RetroOmissionReason::PlanGraphUnavailable,
                "plan_graph",
                "the archive has no plan graph, so plan-relative metrics are unavailable",
                Vec::new(),
            )
            .for_archive(&archive.archive_id),
        );
    }

    PerRunRetro {
        repo_id: input.repo_id.clone(),
        archive_id: archive.archive_id.clone(),
        session_id: archive.session_id.clone(),
        archived_at: archive.archived_at,
        template,
        edit_distance: edit_distance(archive),
        nodes: node_metrics(archive),
        checkpoints: checkpoint_metrics(archive),
        reviews: review_metrics(archive),
        gotcha_edge_hit_rate: gotcha_hit_rate(archive),
        omissions: run_omissions,
    }
}

fn edit_distance(
    archive: &WorkGraphArchive,
) -> EvidenceMetric<PlanActualEditDistance> {
    if archive.plan_graph.is_none() {
        return EvidenceMetric::Unavailable {
            omissions: vec![plan_omission(archive, "edit_distance")],
        };
    }
    let value = PlanActualEditDistance {
        unit_edits: archive.divergence.records.len(),
        by_kind: archive.divergence.counts_by_mutation_type.clone(),
    };
    if !source_available(archive, ArchiveSourceKind::MutationLog) {
        return EvidenceMetric::Unavailable {
            omissions: vec![source_omission(
                archive,
                RetroOmissionReason::MutationEvidenceUnavailable,
                "edit_distance",
                "the final graph cannot prove complete structural history without a tracked mutation log",
            )],
        };
    }
    let mutation_omissions = source_report_omissions(
        archive,
        ArchiveSourceKind::MutationLog,
        "edit_distance",
    );
    if mutation_omissions.is_empty() {
        EvidenceMetric::Available { value }
    } else {
        EvidenceMetric::Partial {
            value,
            omissions: mutation_omissions,
        }
    }
}

fn node_metrics(
    archive: &WorkGraphArchive,
) -> EvidenceMetric<Vec<NodeExecutionMetric>> {
    let Some(plan) = &archive.plan_graph else {
        return EvidenceMetric::Unavailable {
            omissions: vec![plan_omission(archive, "node_execution")],
        };
    };
    let event_available = source_available(archive, ArchiveSourceKind::EventLog);
    let mutation_available = source_available(archive, ArchiveSourceKind::MutationLog);
    let remediation = remediation_counts(archive);
    let value = plan
        .nodes
        .iter()
        .map(|node| {
            let additional_attempts = event_available.then(|| {
                archive
                    .outcomes
                    .iter()
                    .filter(|outcome| outcome_matches_node(outcome, &node.id))
                    .filter(|outcome| event_backed(outcome))
                    .map(|outcome| outcome.attempt_count.saturating_sub(1))
                    .sum()
            });
            NodeExecutionMetric {
                node_id: node.id.clone(),
                additional_attempts,
                remediation_detours: mutation_available
                    .then(|| remediation.get(&node.id).copied().unwrap_or(0)),
            }
        })
        .collect();
    let mut metric_omissions = Vec::new();
    if !event_available {
        metric_omissions.push(source_omission(
            archive,
            RetroOmissionReason::EventEvidenceUnavailable,
            "node_execution",
            "event evidence is unavailable, so additional attempts are not reported as zero",
        ));
    } else {
        metric_omissions.extend(source_report_omissions(
            archive,
            ArchiveSourceKind::EventLog,
            "node_execution",
        ));
    }
    if !mutation_available {
        metric_omissions.push(source_omission(
            archive,
            RetroOmissionReason::MutationEvidenceUnavailable,
            "node_execution",
            "mutation evidence is unavailable, so remediation detours are not reported as zero",
        ));
    } else {
        metric_omissions.extend(source_report_omissions(
            archive,
            ArchiveSourceKind::MutationLog,
            "node_execution",
        ));
    }
    if graph_resolution_incomplete(archive) {
        metric_omissions.push(source_omission(
            archive,
            RetroOmissionReason::ResolutionIncomplete,
            "node_execution",
            "some runtime observations could not be resolved to plan nodes, so per-node counts may be incomplete",
        ));
    }
    if metric_omissions.is_empty() {
        EvidenceMetric::Available { value }
    } else {
        EvidenceMetric::Partial {
            value,
            omissions: metric_omissions,
        }
    }
}

fn remediation_counts(archive: &WorkGraphArchive) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for delta in &archive.deltas {
        if delta.mutation_type != GraphMutationType::RemediationDetour {
            continue;
        }
        let before_ids: BTreeSet<_> =
            delta.before.nodes.iter().map(|node| &node.id).collect();
        let mut targets = BTreeSet::new();
        for node in &delta.after.nodes {
            if before_ids.contains(&node.id) {
                continue;
            }
            let Some(expansion) = &node.expansion else {
                continue;
            };
            if expansion.template == JUDGE_PRINCE_REMEDIATION_TEMPLATE {
                if let Some(target) = expansion.parameters.get("target") {
                    targets.insert(target.clone());
                }
            }
        }
        for target in targets {
            *counts.entry(target).or_default() += 1;
        }
    }
    counts
}

fn checkpoint_metrics(
    archive: &WorkGraphArchive,
) -> EvidenceMetric<Vec<CheckpointBarrierMetric>> {
    let Some(plan) = &archive.plan_graph else {
        return EvidenceMetric::Unavailable {
            omissions: vec![plan_omission(archive, "checkpoint_barrier")],
        };
    };
    let mut values = Vec::new();
    let mut omissions = Vec::new();
    for checkpoint in plan.nodes.iter().filter(|node| node.kind == NodeKind::Checkpoint) {
        let mut prerequisite_ids: Vec<_> = plan
            .edges
            .iter()
            .filter(|edge| {
                edge.kind == EdgeKind::DependsOn && edge.target == checkpoint.id
            })
            .map(|edge| edge.source.clone())
            .collect();
        prerequisite_ids.sort();
        prerequisite_ids.dedup();
        let mut finishes = Vec::new();
        let mut missing = Vec::new();
        for id in &prerequisite_ids {
            match outcome_for_node(archive, id).and_then(|outcome| outcome.finished_at) {
                Some(finished_at) => finishes.push((id.clone(), finished_at)),
                None => missing.push(id.clone()),
            }
        }
        let checkpoint_started_at =
            outcome_for_node(archive, &checkpoint.id).and_then(|outcome| outcome.started_at);
        if checkpoint_started_at.is_none() {
            missing.push(checkpoint.id.clone());
        }
        let ready_at = if finishes.len() == prerequisite_ids.len() && !finishes.is_empty() {
            finishes.iter().map(|(_, time)| *time).max()
        } else {
            None
        };
        let mut sibling_barrier_idle_millis = None;
        let mut gate_release_delay_millis = None;
        let mut total_pre_checkpoint_wait_millis = None;
        if let Some(ready) = ready_at {
            sibling_barrier_idle_millis = Some(
                finishes
                    .iter()
                    .map(|(_, finished)| (ready - *finished).num_milliseconds())
                    .sum(),
            );
            if let Some(started) = checkpoint_started_at {
                let release = (started - ready).num_milliseconds();
                if release < 0 {
                    omissions.push(
                        RetroOmission::new(
                            RetroOmissionReason::InvalidEvidence,
                            "checkpoint_barrier",
                            "checkpoint start precedes the last prerequisite finish",
                            vec![checkpoint.id.clone()],
                        )
                        .for_archive(&archive.archive_id),
                    );
                    sibling_barrier_idle_millis = None;
                } else {
                    gate_release_delay_millis = Some(release);
                    total_pre_checkpoint_wait_millis = Some(
                        finishes
                            .iter()
                            .map(|(_, finished)| (started - *finished).num_milliseconds())
                            .sum(),
                    );
                }
            }
        }
        if !missing.is_empty() {
            omissions.push(
                RetroOmission::new(
                    RetroOmissionReason::TimingEvidenceIncomplete,
                    "checkpoint_barrier",
                    "checkpoint timing is missing a prerequisite finish or checkpoint start",
                    missing.clone(),
                )
                .for_archive(&archive.archive_id),
            );
        }
        values.push(CheckpointBarrierMetric {
            checkpoint_id: checkpoint.id.clone(),
            prerequisite_ids,
            ready_at,
            checkpoint_started_at,
            sibling_barrier_idle_millis,
            gate_release_delay_millis,
            total_pre_checkpoint_wait_millis,
            missing_timestamp_node_ids: missing,
        });
    }
    if omissions.is_empty() {
        EvidenceMetric::Available { value: values }
    } else {
        EvidenceMetric::Partial {
            value: values,
            omissions,
        }
    }
}

fn review_metrics(
    archive: &WorkGraphArchive,
) -> EvidenceMetric<Vec<ReviewEfficacyMetric>> {
    if !source_available(archive, ArchiveSourceKind::MutationLog) {
        return EvidenceMetric::Unavailable {
            omissions: vec![source_omission(
                archive,
                RetroOmissionReason::MutationEvidenceUnavailable,
                "review_efficacy",
                "review verdicts require the structured mutation log",
            )],
        };
    }
    let remediation = remediation_counts(archive);
    let mut reviews: BTreeMap<String, ReviewEfficacyMetric> = BTreeMap::new();
    let mut omissions = Vec::new();
    for delta in &archive.deltas {
        if !matches!(
            delta.mutation_type,
            GraphMutationType::ReviewVerdictRecorded
                | GraphMutationType::RemediationDetour
        ) {
            continue;
        }
        for node in &delta.after.nodes {
            let Some(expansion) = &node.expansion else {
                continue;
            };
            if expansion.template != MULTI_LENS_REVIEW_TEMPLATE
                || node.kind != NodeKind::Join
                || !matches!(node.status, NodeStatus::Completed | NodeStatus::Failed)
            {
                continue;
            }
            let prior_status = delta
                .before
                .nodes
                .iter()
                .find(|prior| prior.id == node.id)
                .map(|prior| prior.status);
            if prior_status == Some(node.status) {
                continue;
            }
            let Some(target_id) = expansion.parameters.get("target") else {
                omissions.push(
                    RetroOmission::new(
                        RetroOmissionReason::ResolutionIncomplete,
                        "review_efficacy",
                        "review verdict metadata has no target",
                        vec![node.id.clone()],
                    )
                    .for_archive(&archive.archive_id),
                );
                continue;
            };
            let state = if node.status == NodeStatus::Failed {
                ReviewEvidenceState::Caught
            } else {
                ReviewEvidenceState::PassedNoKnownEscape
            };
            let review = reviews.entry(node.id.clone()).or_insert_with(|| {
                ReviewEfficacyMetric {
                    verdict_id: node.id.clone(),
                    target_id: target_id.clone(),
                    state,
                    caught_defects: usize::from(state == ReviewEvidenceState::Caught),
                    escaped_defects: 0,
                    remediation_detours: remediation
                        .get(target_id)
                        .copied()
                        .unwrap_or(0),
                    evidence_refs: Vec::new(),
                    revisions: Vec::new(),
                }
            });
            if state == ReviewEvidenceState::Caught {
                review.state = state;
                review.caught_defects = 1;
            }
            review.evidence_refs.extend(delta.source_refs.iter().cloned());
            review.evidence_refs.push(format!("mutation:delta:{}", delta.sequence));
            review.evidence_refs.sort();
            review.evidence_refs.dedup();
        }
    }
    omissions.extend(source_report_omissions(
        archive,
        ArchiveSourceKind::MutationLog,
        "review_efficacy",
    ));
    let value = reviews.into_values().collect();
    if omissions.is_empty() {
        EvidenceMetric::Available { value }
    } else {
        EvidenceMetric::Partial { value, omissions }
    }
}

fn gotcha_hit_rate(
    archive: &WorkGraphArchive,
) -> EvidenceMetric<GotchaEdgeHitRate> {
    let knowledge_missing = archive.runtime_graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::ProjectKnowledgeUnavailable
    });
    if knowledge_missing {
        return EvidenceMetric::Unavailable {
            omissions: vec![source_omission(
                archive,
                RetroOmissionReason::KnowledgeEvidenceUnavailable,
                "gotcha_edge_hit_rate",
                "project knowledge was unavailable, so zero edges would not prove zero gotchas",
            )],
        };
    }
    let eligible: Vec<_> = archive
        .runtime_graph
        .edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::Informs
                && edge.provenance == EdgeProvenance::Knowledge
        })
        .collect();
    if eligible.is_empty() {
        return EvidenceMetric::Partial {
            value: GotchaEdgeHitRate {
                eligible_knowledge_edges: 0,
                targets_attempted: 0,
                rate_defined: false,
            },
            omissions: vec![
                RetroOmission::new(
                    RetroOmissionReason::NoEligibleEdges,
                    "gotcha_edge_hit_rate",
                    "the archived graph has no eligible knowledge edges, so a hit rate is undefined",
                    Vec::new(),
                )
                .for_archive(&archive.archive_id),
            ],
        };
    }
    if !source_available(archive, ArchiveSourceKind::EventLog) {
        return EvidenceMetric::Unavailable {
            omissions: vec![source_omission(
                archive,
                RetroOmissionReason::EventEvidenceUnavailable,
                "gotcha_edge_hit_rate",
                "event evidence is unavailable, so target attempts cannot be observed",
            )],
        };
    }
    let targets_attempted = eligible
        .iter()
        .filter(|edge| {
            archive.outcomes.iter().any(|outcome| {
                outcome_matches_node(outcome, &edge.target)
                    && outcome.attempt_count > 0
                    && event_backed(outcome)
            })
        })
        .count();
    let value = GotchaEdgeHitRate {
        eligible_knowledge_edges: eligible.len(),
        targets_attempted,
        rate_defined: true,
    };
    let mut omissions = source_report_omissions(
        archive,
        ArchiveSourceKind::EventLog,
        "gotcha_edge_hit_rate",
    );
    if graph_resolution_incomplete(archive) {
        omissions.push(source_omission(
            archive,
            RetroOmissionReason::ResolutionIncomplete,
            "gotcha_edge_hit_rate",
            "unresolved runtime observations may undercount attempted informed targets",
        ));
    }
    if !omissions.is_empty() {
        EvidenceMetric::Partial {
            value,
            omissions,
        }
    } else {
        EvidenceMetric::Available { value }
    }
}

fn apply_review_escape_revisions(
    inputs: &[RetroRunInput],
    runs: &mut [PerRunRetro],
    omissions: &mut Vec<RetroOmission>,
) {
    let positions: BTreeMap<_, _> = runs
        .iter()
        .enumerate()
        .map(|(index, run)| (run.archive_id.clone(), index))
        .collect();
    let mut seen = BTreeSet::new();
    for discovering in inputs {
        if !source_available(&discovering.archive, ArchiveSourceKind::RunLedger) {
            omissions.push(source_omission(
                &discovering.archive,
                RetroOmissionReason::RunLedgerEvidenceUnavailable,
                "review_efficacy",
                "run-ledger evidence is unavailable, so later review escapes may be absent",
            ));
        }
        for outcome in &discovering.archive.outcomes {
            for effect in &outcome.effects {
                if effect.kind != REVIEW_ESCAPE_EFFECT_KIND || !effect.confirmed {
                    continue;
                }
                let Some(reference) = effect.reference.as_deref() else {
                    let omission = RetroOmission::new(
                        RetroOmissionReason::ResolutionIncomplete,
                        "review_efficacy",
                        "confirmed review_escape effect has no <archive_id>#<verdict_id> reference; no prior verdict was guessed",
                        vec![effect.source_ref.clone()],
                    )
                    .for_archive(&discovering.archive.archive_id);
                    downgrade_prior_review_evidence(
                        runs,
                        discovering.archive.archived_at,
                        &omission,
                    );
                    omissions.push(omission);
                    continue;
                };
                let Some((archive_id, verdict_id)) = reference.split_once('#') else {
                    omissions.push(
                        RetroOmission::new(
                            RetroOmissionReason::ResolutionIncomplete,
                            "review_efficacy",
                            "review_escape effect reference must be <archive_id>#<verdict_id>",
                            vec![reference.to_string()],
                        )
                        .for_archive(&discovering.archive.archive_id),
                    );
                    continue;
                };
                let identity = (
                    archive_id.to_string(),
                    verdict_id.to_string(),
                    discovering.archive.archive_id.clone(),
                    effect.source_ref.clone(),
                );
                if !seen.insert(identity) {
                    continue;
                }
                let Some(index) = positions.get(archive_id).copied() else {
                    omissions.push(
                        RetroOmission::new(
                            RetroOmissionReason::ResolutionIncomplete,
                            "review_efficacy",
                            "review_escape effect references an archive outside the evaluated corpus",
                            vec![reference.to_string()],
                        )
                        .for_archive(&discovering.archive.archive_id),
                    );
                    continue;
                };
                if runs[index].archived_at >= discovering.archive.archived_at {
                    omissions.push(
                        RetroOmission::new(
                            RetroOmissionReason::InvalidEvidence,
                            "review_efficacy",
                            "review_escape evidence does not occur after the referenced review",
                            vec![reference.to_string()],
                        )
                        .for_archive(&discovering.archive.archive_id),
                    );
                    continue;
                }
                let Some(reviews) = runs[index].reviews.value_mut() else {
                    continue;
                };
                let Some(review) = reviews
                    .iter_mut()
                    .find(|review| review.verdict_id == verdict_id)
                else {
                    omissions.push(
                        RetroOmission::new(
                            RetroOmissionReason::ResolutionIncomplete,
                            "review_efficacy",
                            "review_escape effect references an unknown verdict",
                            vec![reference.to_string()],
                        )
                        .for_archive(&discovering.archive.archive_id),
                    );
                    continue;
                };
                if review.state != ReviewEvidenceState::PassedNoKnownEscape {
                    continue;
                }
                review.state = ReviewEvidenceState::Escaped;
                review.escaped_defects = review.escaped_defects.saturating_add(1);
                review.revisions.push(ReviewEfficacyRevision {
                    discovering_archive_id: discovering.archive.archive_id.clone(),
                    source_ref: effect.source_ref.clone(),
                    observed_at: discovering.archive.archived_at,
                });
            }
        }
    }
}

fn downgrade_prior_review_evidence(
    runs: &mut [PerRunRetro],
    discovering_at: DateTime<Utc>,
    omission: &RetroOmission,
) {
    for run in runs
        .iter_mut()
        .filter(|run| run.archived_at < discovering_at)
    {
        run.reviews.add_omission(omission.clone());
    }
}

#[derive(Debug, Clone)]
struct DeviationOccurrence {
    repo_id: String,
    archive_id: String,
    session_id: String,
    archived_at: DateTime<Utc>,
}

fn systematic_divergence(
    inputs: &[RetroRunInput],
    runs: &[PerRunRetro],
    omissions: &mut Vec<RetroOmission>,
) -> (
    Vec<DeviationPromotionProposal>,
    Vec<UnreviewedLearningSubmission>,
) {
    let templates: BTreeMap<_, _> = runs
        .iter()
        .filter_map(|run| run.template.clone().map(|key| (run.archive_id.clone(), key)))
        .collect();
    let mut groups: BTreeMap<(TemplateKey, String), Vec<DeviationOccurrence>> =
        BTreeMap::new();
    for input in inputs {
        let Some(template) = templates.get(&input.archive.archive_id) else {
            continue;
        };
        let mut per_archive = BTreeSet::new();
        for record in &input.archive.divergence.records {
            match canonical_deviation_key(&input.archive, record) {
                Some(key) => {
                    per_archive.insert(key);
                }
                None => omissions.push(
                    RetroOmission::new(
                        RetroOmissionReason::ResolutionIncomplete,
                        "systematic_divergence",
                        "a structural deviation could not be resolved to one canonical signature",
                        vec![format!("{:?}", record)],
                    )
                    .for_archive(&input.archive.archive_id),
                ),
            }
        }
        for key in per_archive {
            groups
                .entry((template.clone(), key))
                .or_default()
                .push(DeviationOccurrence {
                    repo_id: input.repo_id.clone(),
                    archive_id: input.archive.archive_id.clone(),
                    session_id: input.archive.session_id.clone(),
                    archived_at: input.archive.archived_at,
                });
        }
    }
    let mut proposals = Vec::new();
    let mut learnings = Vec::new();
    for ((template, deviation_key), mut occurrences) in groups {
        occurrences.sort_by(|left, right| {
            left.archived_at
                .cmp(&right.archived_at)
                .then(left.archive_id.cmp(&right.archive_id))
        });
        let repo_ids: BTreeSet<_> =
            occurrences.iter().map(|item| item.repo_id.clone()).collect();
        if occurrences.len() < PROMOTION_RUN_THRESHOLD {
            continue;
        }
        let tier = if repo_ids.len() >= 2 {
            PromotionTier::InstitutionalRevision
        } else {
            PromotionTier::ProjectOverride
        };
        let archetype_id = format!("{}@{}", template.template_id, template.template_version);
        let proposal = DeviationPromotionProposal {
            tier,
            archetype_id: archetype_id.clone(),
            deviation_key: deviation_key.clone(),
            observation_count: occurrences.len(),
            repo_ids: repo_ids.into_iter().collect(),
            rationale: match tier {
                PromotionTier::ProjectOverride => "the same archived structural deviation recurred in at least two runs of this template version; propose a Tier 1 override for review".to_string(),
                PromotionTier::InstitutionalRevision => "the same archived structural deviation recurred across at least two repositories using this template version; propose a PR-gated Tier 2 revision for review".to_string(),
            },
        };
        let latest = occurrences
            .last()
            .expect("promotion threshold guarantees an occurrence");
        learnings.push(UnreviewedLearningSubmission {
            session: latest.session_id.clone(),
            task: "post-run graph retro promotion proposal".to_string(),
            outcome: UNREVIEWED_OUTCOME.to_string(),
            keywords: vec![
                "work-graph".to_string(),
                "systematic-divergence".to_string(),
                archetype_id,
            ],
            insight: format!(
                "Archived evidence recorded the same structural deviation ({deviation_key}) in {} distinct runs. This is a proposal for independent curation, not a claim that another topology is better.",
                occurrences.len()
            ),
            files_touched: Vec::new(),
        });
        proposals.push(proposal);
    }
    (proposals, learnings)
}

fn canonical_deviation_key(
    archive: &WorkGraphArchive,
    record: &DivergenceRecord,
) -> Option<String> {
    let plan = archive.plan_graph.as_ref()?;
    let payload = match record.kind {
        DivergenceKind::NodeAdded => {
            let id = record.node_id.as_deref()?;
            serde_json::json!({
                "kind": "node_added",
                "node_id": id,
                "actual": canonical_node(find_node(&archive.runtime_graph, id)?)
            })
        }
        DivergenceKind::NodeRemoved => {
            let id = record.node_id.as_deref()?;
            serde_json::json!({
                "kind": "node_removed",
                "node_id": id,
                "planned": canonical_node(find_node(plan, id)?)
            })
        }
        DivergenceKind::NodeRestructured => {
            let id = record.node_id.as_deref()?;
            serde_json::json!({
                "kind": "node_restructured",
                "node_id": id,
                "planned": canonical_node(find_node(plan, id)?),
                "actual": canonical_node(find_node(&archive.runtime_graph, id)?)
            })
        }
        DivergenceKind::EdgeAdded => {
            let source = record.source.as_deref()?;
            let target = record.target.as_deref()?;
            serde_json::json!({
                "kind": "edge_added",
                "source": source,
                "target": target,
                "edge_kind": unique_edge_kind(&archive.runtime_graph, source, target)?
            })
        }
        DivergenceKind::EdgeRemoved => {
            let source = record.source.as_deref()?;
            let target = record.target.as_deref()?;
            serde_json::json!({
                "kind": "edge_removed",
                "source": source,
                "target": target,
                "edge_kind": unique_edge_kind(plan, source, target)?
            })
        }
        DivergenceKind::EdgeRewired => {
            let source = record.source.as_deref()?;
            let target = record.target.as_deref()?;
            let replacement_source = record.replacement_source.as_deref()?;
            let replacement_target = record.replacement_target.as_deref()?;
            serde_json::json!({
                "kind": "edge_rewired",
                "source": source,
                "target": target,
                "edge_kind": unique_edge_kind(plan, source, target)?,
                "replacement_source": replacement_source,
                "replacement_target": replacement_target,
                "replacement_edge_kind": unique_edge_kind(
                    &archive.runtime_graph,
                    replacement_source,
                    replacement_target,
                )?
            })
        }
    };
    serde_json::to_string(&payload).ok()
}

fn canonical_node(node: &WorkNode) -> serde_json::Value {
    serde_json::json!({
        "kind": node.kind,
        "title": node.title,
        "contract": node.contract,
        "binding": node.binding,
        "expansion": node.expansion,
    })
}

fn unique_edge_kind(graph: &TaskGraph, source: &str, target: &str) -> Option<String> {
    let kinds: BTreeSet<_> = graph
        .edges
        .iter()
        .filter(|edge| edge.source == source && edge.target == target)
        .filter_map(|edge| serde_json::to_string(&edge.kind).ok())
        .collect();
    (kinds.len() == 1).then(|| kinds.into_iter().next()).flatten()
}

fn aggregate_templates(runs: &[PerRunRetro]) -> Vec<TemplateAggregate> {
    let mut aggregates: BTreeMap<TemplateKey, TemplateAggregate> = BTreeMap::new();
    for run in runs {
        let Some(template) = &run.template else {
            continue;
        };
        let aggregate = aggregates
            .entry(template.clone())
            .or_insert_with(|| TemplateAggregate {
                template: template.clone(),
                run_count: 0,
                observed_edit_units: None,
                additional_attempts_by_node: BTreeMap::new(),
                remediation_detours_by_node: BTreeMap::new(),
                sibling_barrier_idle_millis_by_checkpoint: BTreeMap::new(),
                review_efficacy: EvidenceMetric::Available {
                    value: ReviewEfficacyAggregate::default(),
                },
                gotcha_edges_eligible: None,
                gotcha_targets_attempted: None,
            });
        aggregate.run_count += 1;
        if let Some(distance) = run.edit_distance.value() {
            *aggregate.observed_edit_units.get_or_insert(0) += distance.unit_edits;
        }
        if let Some(nodes) = run.nodes.value() {
            for node in nodes {
                if let Some(count) = node.additional_attempts {
                    *aggregate
                        .additional_attempts_by_node
                        .entry(node.node_id.clone())
                        .or_default() += count;
                }
                if let Some(count) = node.remediation_detours {
                    *aggregate
                        .remediation_detours_by_node
                        .entry(node.node_id.clone())
                        .or_default() += count;
                }
            }
        }
        if let Some(checkpoints) = run.checkpoints.value() {
            for checkpoint in checkpoints {
                if let Some(idle) = checkpoint.sibling_barrier_idle_millis {
                    *aggregate
                        .sibling_barrier_idle_millis_by_checkpoint
                        .entry(checkpoint.checkpoint_id.clone())
                        .or_default() += idle;
                }
            }
        }
        if let Some(reviews) = run.reviews.value() {
            for review in reviews {
                if let Some(review_aggregate) = aggregate.review_efficacy.value_mut() {
                    review_aggregate.caught_defects += review.caught_defects;
                    review_aggregate.escaped_defects += review.escaped_defects;
                }
            }
        }
        for omission in run.reviews.omissions() {
            aggregate.review_efficacy.add_omission(omission.clone());
        }
        if let Some(gotcha) = run.gotcha_edge_hit_rate.value() {
            if gotcha.rate_defined {
                *aggregate.gotcha_edges_eligible.get_or_insert(0) +=
                    gotcha.eligible_knowledge_edges;
                *aggregate.gotcha_targets_attempted.get_or_insert(0) +=
                    gotcha.targets_attempted;
            }
        }
    }
    aggregates.into_values().collect()
}

fn source_available(archive: &WorkGraphArchive, kind: ArchiveSourceKind) -> bool {
    archive
        .sources
        .iter()
        .find(|source| source.kind == kind)
        .map(|source| source.available)
        .unwrap_or(false)
}

fn source_report_omissions(
    archive: &WorkGraphArchive,
    kind: ArchiveSourceKind,
    metric: &str,
) -> Vec<RetroOmission> {
    archive
        .sources
        .iter()
        .find(|source| source.kind == kind)
        .into_iter()
        .flat_map(|source| &source.omissions)
        .map(|omission| {
            RetroOmission::new(
                RetroOmissionReason::ResolutionIncomplete,
                metric,
                omission.detail.clone(),
                omission.examples.clone(),
            )
            .for_archive(&archive.archive_id)
        })
        .collect()
}

fn graph_resolution_incomplete(archive: &WorkGraphArchive) -> bool {
    archive.runtime_graph.omissions.iter().any(|omission| {
        omission.reason == WorkGraphOmissionReason::ResolutionIncomplete
            || omission.reason == WorkGraphOmissionReason::SourceUnreadable
    })
}

fn source_omission(
    archive: &WorkGraphArchive,
    reason: RetroOmissionReason,
    metric: &str,
    detail: &str,
) -> RetroOmission {
    RetroOmission::new(reason, metric, detail, Vec::new())
        .for_archive(&archive.archive_id)
}

fn plan_omission(archive: &WorkGraphArchive, metric: &str) -> RetroOmission {
    source_omission(
        archive,
        RetroOmissionReason::PlanGraphUnavailable,
        metric,
        "the archive has no plan graph baseline",
    )
}

fn event_backed(outcome: &RuntimeOutcome) -> bool {
    outcome
        .source_refs
        .iter()
        .any(|source| source.starts_with("event:"))
}

fn outcome_matches_node(outcome: &RuntimeOutcome, node_id: &str) -> bool {
    outcome.subject_id == node_id || outcome.task_id.as_deref() == Some(node_id)
}

fn outcome_for_node<'a>(
    archive: &'a WorkGraphArchive,
    node_id: &str,
) -> Option<&'a RuntimeOutcome> {
    archive
        .outcomes
        .iter()
        .find(|outcome| outcome.subject_id == node_id)
        .or_else(|| {
            archive
                .outcomes
                .iter()
                .find(|outcome| outcome.task_id.as_deref() == Some(node_id))
        })
}

fn find_node<'a>(graph: &'a TaskGraph, node_id: &str) -> Option<&'a WorkNode> {
    graph.nodes.iter().find(|node| node.id == node_id)
}
