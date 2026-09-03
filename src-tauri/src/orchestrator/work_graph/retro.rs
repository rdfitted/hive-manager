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
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::http::handlers::workers::TierResolutionSource;

use super::archetypes::{
    propose_task_tier_calibrations, DeviationPromotionProposal, PromotionTier,
    TaskTierCalibrationObservation, TaskTierCalibrationProposal, TaskTierCalibrationTarget,
    GRAPH_ARCHETYPE_EXPANSION,
};
use super::archive::{
    list_archives, read_archive, ArchiveSourceKind, WorkGraphArchive,
    WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
};
use super::divergence::{DivergenceKind, DivergenceRecord};
use super::review::{JUDGE_PRINCE_REMEDIATION_TEMPLATE, MULTI_LENS_REVIEW_TEMPLATE};
use super::runtime::{GraphMutationType, RuntimeOutcome};
use super::schema::{
    EdgeKind, EdgeProvenance, NodeKind, NodeStatus, TaskGraph, TaskTier, WorkGraphOmissionReason,
    WorkNode,
};

pub const PROMOTION_RUN_THRESHOLD: usize = 2;
pub const REVIEW_ESCAPE_EFFECT_KIND: &str = "review_escape";
pub const ROLE_SCOPE_GAP_EFFECT_KIND: &str = "role_scope_gap";
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

/// Exact role-definition lineage used for historical aggregation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDefinitionKey {
    pub definition_id: String,
    pub definition_version: u32,
}

impl PartialOrd for RoleDefinitionKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RoleDefinitionKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.definition_id
            .cmp(&other.definition_id)
            .then(self.definition_version.cmp(&other.definition_version))
    }
}

/// Immutable attribution captured when an agent resolves its role definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRoleDefinitionAttribution {
    pub session_id: String,
    pub agent_id: String,
    pub definition: RoleDefinitionKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleRefinementSignal {
    AdditionalAttempts,
    RemediationDetours,
    ReviewEscapes,
    UnusedKnowledgeScope,
    DeclaredScopeGap,
}

impl RoleRefinementSignal {
    pub const fn scope_impact(self) -> ScopeImpact {
        match self {
            Self::AdditionalAttempts | Self::RemediationDetours => ScopeImpact::NoScopeChange,
            Self::ReviewEscapes | Self::UnusedKnowledgeScope => ScopeImpact::Narrowing,
            Self::DeclaredScopeGap => ScopeImpact::Widening,
        }
    }

    const fn change_key(self) -> &'static str {
        match self {
            Self::AdditionalAttempts => "additional_attempts",
            Self::RemediationDetours => "remediation_detours",
            Self::ReviewEscapes => "review_escapes",
            Self::UnusedKnowledgeScope => "unused_knowledge_scope",
            Self::DeclaredScopeGap => "declared_scope_gap",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeImpact {
    Narrowing,
    Widening,
    NoScopeChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDefinitionRefinementObservation {
    pub repo_id: String,
    pub session_id: String,
    pub archive_id: String,
    pub definition: RoleDefinitionKey,
    pub signal: RoleRefinementSignal,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDefinitionRefinementProposal {
    pub tier: PromotionTier,
    pub definition: RoleDefinitionKey,
    pub change_key: String,
    pub scope_impact: ScopeImpact,
    pub observation_count: usize,
    pub repo_ids: Vec<String>,
    pub session_ids: Vec<String>,
    pub archive_ids: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub rationale: String,
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
    Ok(keys
        .into_iter()
        .next()
        .expect("one lineage key was checked"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetroOmissionReason {
    NoArchives,
    ArchiveUnreadable,
    UnsupportedSchemaVersion,
    PlanGraphUnavailable,
    TemplateLineageUnavailable,
    RoleDefinitionLineageUnavailable,
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
    ExecutedAsUnavailable,
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
    Available {
        value: T,
    },
    Partial {
        value: T,
        omissions: Vec<RetroOmission>,
    },
    Unavailable {
        omissions: Vec<RetroOmission>,
    },
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
            Self::Partial { omissions, .. } | Self::Unavailable { omissions } => omissions,
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
        let planner_agent_ids: BTreeSet<String> = planner_agent_ids.into_iter().collect();
        let supervisor_agent_ids: BTreeSet<String> = supervisor_agent_ids.into_iter().collect();
        if planner_agent_ids.contains(&evaluator_id) || supervisor_agent_ids.contains(&evaluator_id)
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

#[derive(Debug, Deserialize)]
struct PersistedRoleAttributionSession {
    id: String,
    #[serde(default)]
    agents: Vec<PersistedRoleAttributionAgent>,
}

#[derive(Debug, Deserialize)]
struct PersistedRoleAttributionAgent {
    id: String,
    #[serde(default)]
    role_definition_id: Option<String>,
    #[serde(default)]
    role_definition_version: Option<u32>,
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
pub struct RoleDefinitionRunMetric {
    pub definition: RoleDefinitionKey,
    pub agent_ids: Vec<String>,
    pub node_ids: Vec<String>,
    pub additional_attempts: Option<usize>,
    pub remediation_detours: Option<usize>,
    pub caught_defects: Option<usize>,
    pub escaped_defects: Option<usize>,
    pub gotcha_edges_eligible: Option<usize>,
    pub gotcha_targets_attempted: Option<usize>,
    pub confirmed_scope_gaps: usize,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskTierRunMetric {
    pub tier: TaskTier,
    pub provider: String,
    pub completion_count: usize,
    pub node_ids: Vec<String>,
    pub additional_attempts: Option<usize>,
    pub remediation_detours: Option<usize>,
    pub caught_defects: Option<usize>,
    pub escaped_defects: Option<usize>,
    pub elapsed_millis: Option<i64>,
    pub confirmed_scope_gaps: usize,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

fn task_tier_metric_unavailable() -> EvidenceMetric<Vec<TaskTierRunMetric>> {
    EvidenceMetric::Unavailable {
        omissions: vec![RetroOmission::new(
            RetroOmissionReason::ExecutedAsUnavailable,
            "task_tier",
            "the persisted retro predates executed_as evidence",
            Vec::new(),
        )],
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDefinitionAggregate {
    pub definition: RoleDefinitionKey,
    pub run_count: usize,
    pub repo_ids: Vec<String>,
    pub session_ids: Vec<String>,
    pub archive_ids: Vec<String>,
    pub additional_attempts: Option<usize>,
    #[serde(default)]
    pub additional_attempts_contributing_runs: usize,
    pub remediation_detours: Option<usize>,
    #[serde(default)]
    pub remediation_detours_contributing_runs: usize,
    pub caught_defects: Option<usize>,
    #[serde(default)]
    pub caught_defects_contributing_runs: usize,
    pub escaped_defects: Option<usize>,
    #[serde(default)]
    pub escaped_defects_contributing_runs: usize,
    pub gotcha_edges_eligible: Option<usize>,
    #[serde(default)]
    pub gotcha_edges_eligible_contributing_runs: usize,
    pub gotcha_targets_attempted: Option<usize>,
    #[serde(default)]
    pub gotcha_targets_attempted_contributing_runs: usize,
    pub confirmed_scope_gaps: usize,
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
    #[serde(default = "task_tier_metric_unavailable")]
    pub task_tiers: EvidenceMetric<Vec<TaskTierRunMetric>>,
    #[serde(default)]
    pub role_definitions: Vec<RoleDefinitionRunMetric>,
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
    #[serde(default)]
    pub role_definition_aggregates: Vec<RoleDefinitionAggregate>,
    #[serde(default)]
    pub role_refinement_proposals: Vec<RoleDefinitionRefinementProposal>,
    #[serde(default)]
    pub planner_calibration_proposals: Vec<TaskTierCalibrationProposal>,
    #[serde(default)]
    pub ladder_calibration_proposals: Vec<TaskTierCalibrationProposal>,
    /// Request bodies exposed for the sanctioned session-learning endpoint.
    pub learning_submissions: Vec<UnreviewedLearningSubmission>,
    #[serde(default)]
    pub omissions: Vec<RetroOmission>,
}

impl RetroReport {
    pub(super) fn unavailable(evaluator_id: impl Into<String>, omission: RetroOmission) -> Self {
        Self {
            evaluator_id: evaluator_id.into(),
            runs: Vec::new(),
            template_aggregates: Vec::new(),
            promotion_proposals: Vec::new(),
            role_definition_aggregates: Vec::new(),
            role_refinement_proposals: Vec::new(),
            planner_calibration_proposals: Vec::new(),
            ladder_calibration_proposals: Vec::new(),
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
    let mut role_attributions = Vec::new();
    let mut omissions = Vec::new();
    for input in paths {
        match read_archive(&input.path) {
            Ok(archive) => {
                let (mut loaded_attributions, mut attribution_omissions) =
                    load_role_attributions_for_archive(&input.path, &archive);
                role_attributions.append(&mut loaded_attributions);
                omissions.append(&mut attribution_omissions);
                inputs.push(RetroRunInput {
                    repo_id: input.repo_id.clone(),
                    archive,
                });
            }
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
    evaluate_inputs(
        evaluator,
        &inputs,
        &role_attributions,
        omissions,
        paths.is_empty(),
    )
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
    evaluate_inputs(evaluator, inputs, &[], Vec::new(), inputs.is_empty())
}

/// Evaluate in-memory archives with exact historical role-definition lineage.
/// The legacy entry point remains available and reports missing lineage rather
/// than guessing from a role binding or the current definition on disk.
pub fn evaluate_archives_with_role_attributions(
    evaluator: &IndependentEvaluator,
    inputs: &[RetroRunInput],
    role_attributions: &[AgentRoleDefinitionAttribution],
) -> Result<RetroReport, RetroError> {
    evaluate_inputs(
        evaluator,
        inputs,
        role_attributions,
        Vec::new(),
        inputs.is_empty(),
    )
}

fn evaluate_inputs(
    evaluator: &IndependentEvaluator,
    inputs: &[RetroRunInput],
    role_attributions: &[AgentRoleDefinitionAttribution],
    mut omissions: Vec<RetroOmission>,
    no_candidates: bool,
) -> Result<RetroReport, RetroError> {
    // Re-check the invariant at execution time even though construction also
    // enforces it. This makes the safety boundary local to the evaluator.
    if evaluator
        .planner_agent_ids
        .contains(&evaluator.evaluator_id)
        || evaluator
            .supervisor_agent_ids
            .contains(&evaluator.evaluator_id)
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
    for (input, run) in valid.iter().zip(runs.iter_mut()) {
        let role_definitions =
            role_definition_metrics(input, run, role_attributions, &mut omissions);
        run.role_definitions = role_definitions;
    }
    let (promotion_proposals, mut learning_submissions) =
        systematic_divergence(&valid, &runs, &mut omissions);
    let template_aggregates = aggregate_templates(&runs);
    let role_definition_aggregates = aggregate_role_definitions(&runs);
    let role_observations = role_refinement_observations(&runs);
    let role_refinement_proposals = propose_role_definition_refinements(&role_observations);
    learning_submissions.extend(role_refinement_learnings(&role_refinement_proposals));
    let calibration_observations = task_tier_calibration_observations(&valid, &runs);
    let calibration_proposals = propose_task_tier_calibrations(&calibration_observations);
    let (planner_calibration_proposals, ladder_calibration_proposals): (Vec<_>, Vec<_>) =
        calibration_proposals
            .into_iter()
            .partition(|proposal| proposal.target == TaskTierCalibrationTarget::Planner);
    learning_submissions.extend(task_tier_calibration_learnings(
        planner_calibration_proposals
            .iter()
            .chain(&ladder_calibration_proposals),
    ));
    for run in &runs {
        omissions.extend(run.omissions.iter().cloned());
    }
    Ok(RetroReport {
        evaluator_id: evaluator.evaluator_id.clone(),
        runs,
        template_aggregates,
        promotion_proposals,
        role_definition_aggregates,
        role_refinement_proposals,
        planner_calibration_proposals,
        ladder_calibration_proposals,
        learning_submissions,
        omissions,
    })
}

fn evaluate_run(input: &RetroRunInput) -> PerRunRetro {
    let archive = &input.archive;
    let mut run_omissions = Vec::new();
    let template = archive
        .plan_graph
        .as_ref()
        .and_then(|plan| match template_key(plan) {
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

    let nodes = node_metrics(archive);
    let reviews = review_metrics(archive);
    let task_tiers = task_tier_metrics(archive, &nodes, &reviews);

    PerRunRetro {
        repo_id: input.repo_id.clone(),
        archive_id: archive.archive_id.clone(),
        session_id: archive.session_id.clone(),
        archived_at: archive.archived_at,
        template,
        edit_distance: edit_distance(archive),
        nodes,
        checkpoints: checkpoint_metrics(archive),
        reviews,
        gotcha_edge_hit_rate: gotcha_hit_rate(archive),
        task_tiers,
        role_definitions: Vec::new(),
        omissions: run_omissions,
    }
}

fn load_role_attributions_for_archive(
    archive_path: &Path,
    archive: &WorkGraphArchive,
) -> (Vec<AgentRoleDefinitionAttribution>, Vec<RetroOmission>) {
    let Some(session_dir) = archive_path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
    else {
        return (
            Vec::new(),
            vec![RetroOmission::new(
                RetroOmissionReason::RoleDefinitionLineageUnavailable,
                "role_definition_attribution",
                "the archive path does not identify its session directory",
                vec![archive_path.display().to_string()],
            )
            .for_archive(&archive.archive_id)],
        );
    };
    let session_path = session_dir.join("session.json");
    let persisted = match fs::read_to_string(&session_path)
        .map_err(|error| error.to_string())
        .and_then(|source| {
            serde_json::from_str::<PersistedRoleAttributionSession>(&source)
                .map_err(|error| error.to_string())
        }) {
        Ok(persisted) => persisted,
        Err(detail) => {
            return (
                Vec::new(),
                vec![RetroOmission::new(
                    RetroOmissionReason::RoleDefinitionLineageUnavailable,
                    "role_definition_attribution",
                    format!("persisted agent role-definition lineage is unavailable: {detail}"),
                    vec![session_path.display().to_string()],
                )
                .for_archive(&archive.archive_id)],
            );
        }
    };
    if persisted.id != archive.session_id {
        return (
            Vec::new(),
            vec![RetroOmission::new(
                RetroOmissionReason::RoleDefinitionLineageUnavailable,
                "role_definition_attribution",
                "the persisted session identity does not match the archive",
                vec![persisted.id, archive.session_id.clone()],
            )
            .for_archive(&archive.archive_id)],
        );
    }

    let mut attributions = Vec::new();
    let mut incomplete = Vec::new();
    let relevant_agent_ids: BTreeSet<_> = archive
        .outcomes
        .iter()
        .flat_map(|outcome| outcome.agent_ids.iter().cloned())
        .collect();
    for agent in persisted.agents {
        if !relevant_agent_ids.contains(&agent.id) {
            continue;
        }
        match (agent.role_definition_id, agent.role_definition_version) {
            (Some(definition_id), Some(definition_version))
                if !definition_id.trim().is_empty() && definition_version > 0 =>
            {
                attributions.push(AgentRoleDefinitionAttribution {
                    session_id: archive.session_id.clone(),
                    agent_id: agent.id,
                    definition: RoleDefinitionKey {
                        definition_id,
                        definition_version,
                    },
                });
            }
            _ => incomplete.push(agent.id),
        }
    }
    let omissions = if incomplete.is_empty() {
        Vec::new()
    } else {
        vec![RetroOmission::new(
            RetroOmissionReason::RoleDefinitionLineageUnavailable,
            "role_definition_attribution",
            "persisted agents without both a definition id and version were excluded",
            incomplete,
        )
        .for_archive(&archive.archive_id)]
    };
    (attributions, omissions)
}

#[derive(Default)]
struct RoleMetricAccumulator {
    agent_ids: BTreeSet<String>,
    node_ids: BTreeSet<String>,
    additional_attempts: usize,
    remediation_detours: usize,
    caught_defects: usize,
    escaped_defects: usize,
    gotcha_edges_eligible: usize,
    gotcha_targets_attempted: usize,
    confirmed_scope_gaps: usize,
    evidence_refs: BTreeSet<String>,
}

fn role_definition_metrics(
    input: &RetroRunInput,
    run: &PerRunRetro,
    role_attributions: &[AgentRoleDefinitionAttribution],
    omissions: &mut Vec<RetroOmission>,
) -> Vec<RoleDefinitionRunMetric> {
    let archive = &input.archive;
    let mut by_agent = BTreeMap::new();
    let mut conflicts = BTreeSet::new();
    for attribution in role_attributions
        .iter()
        .filter(|item| item.session_id == archive.session_id)
    {
        if attribution.agent_id.trim().is_empty()
            || attribution.definition.definition_id.trim().is_empty()
            || attribution.definition.definition_version == 0
        {
            conflicts.insert(attribution.agent_id.clone());
            continue;
        }
        match by_agent.get(&attribution.agent_id) {
            Some(existing) if existing != &attribution.definition => {
                conflicts.insert(attribution.agent_id.clone());
            }
            _ => {
                by_agent.insert(attribution.agent_id.clone(), attribution.definition.clone());
            }
        }
    }
    for agent_id in &conflicts {
        by_agent.remove(agent_id);
    }

    let mut by_node: BTreeMap<String, BTreeSet<RoleDefinitionKey>> = BTreeMap::new();
    let mut accumulators: BTreeMap<RoleDefinitionKey, RoleMetricAccumulator> = BTreeMap::new();
    let mut missing_agents = BTreeSet::new();
    for outcome in &archive.outcomes {
        let mut outcome_definitions = BTreeSet::new();
        let confirmed_scope_gaps = outcome
            .effects
            .iter()
            .filter(|effect| effect.confirmed && effect.kind == ROLE_SCOPE_GAP_EFFECT_KIND)
            .count();
        for agent_id in &outcome.agent_ids {
            let Some(definition) = by_agent.get(agent_id) else {
                missing_agents.insert(agent_id.clone());
                continue;
            };
            let newly_inserted_definition = outcome_definitions.insert(definition.clone());
            let accumulator = accumulators.entry(definition.clone()).or_default();
            accumulator.agent_ids.insert(agent_id.clone());
            accumulator
                .evidence_refs
                .extend(outcome.source_refs.iter().cloned());
            if newly_inserted_definition {
                accumulator.confirmed_scope_gaps += confirmed_scope_gaps;
            }
            accumulator.evidence_refs.extend(
                outcome
                    .effects
                    .iter()
                    .filter(|effect| effect.confirmed)
                    .map(|effect| effect.source_ref.clone()),
            );
        }
        let mut node_ids = BTreeSet::from([outcome.subject_id.clone()]);
        if let Some(task_id) = &outcome.task_id {
            node_ids.insert(task_id.clone());
        }
        for node_id in node_ids {
            by_node
                .entry(node_id.clone())
                .or_default()
                .extend(outcome_definitions.iter().cloned());
            for definition in &outcome_definitions {
                accumulators
                    .entry(definition.clone())
                    .or_default()
                    .node_ids
                    .insert(node_id.clone());
            }
        }
    }

    let mut lineage_examples = conflicts;
    lineage_examples.extend(missing_agents);
    let lineage_already_reported = omissions.iter().any(|omission| {
        omission.reason == RetroOmissionReason::RoleDefinitionLineageUnavailable
            && omission.archive_id.as_deref() == Some(archive.archive_id.as_str())
    });
    if (!lineage_examples.is_empty() || (by_agent.is_empty() && !archive.outcomes.is_empty()))
        && !lineage_already_reported
    {
        omissions.push(
            RetroOmission::new(
                RetroOmissionReason::RoleDefinitionLineageUnavailable,
                "role_definition_attribution",
                "runtime agent ids without one exact persisted role-definition version were excluded",
                lineage_examples.into_iter().collect(),
            )
            .for_archive(&archive.archive_id),
        );
    }

    if let Some(nodes) = run.nodes.value() {
        for node in nodes {
            let Some(definitions) = by_node.get(&node.node_id) else {
                continue;
            };
            for definition in definitions {
                let accumulator = accumulators.entry(definition.clone()).or_default();
                accumulator.additional_attempts += node.additional_attempts.unwrap_or(0);
                accumulator.remediation_detours += node.remediation_detours.unwrap_or(0);
            }
        }
    }

    if let Some(reviews) = run.reviews.value() {
        for review in reviews {
            let Some(definitions) = by_node.get(&review.verdict_id) else {
                continue;
            };
            for definition in definitions {
                let accumulator = accumulators.entry(definition.clone()).or_default();
                accumulator.caught_defects += review.caught_defects;
                accumulator.escaped_defects += review.escaped_defects;
                accumulator
                    .evidence_refs
                    .extend(review.evidence_refs.iter().cloned());
                accumulator.evidence_refs.extend(
                    review
                        .revisions
                        .iter()
                        .map(|revision| revision.source_ref.clone()),
                );
            }
        }
    }

    let knowledge_available =
        !archive.runtime_graph.omissions.iter().any(|omission| {
            omission.reason == WorkGraphOmissionReason::ProjectKnowledgeUnavailable
        });
    let event_available = source_available(archive, ArchiveSourceKind::EventLog);
    if knowledge_available && event_available {
        for edge in archive.runtime_graph.edges.iter().filter(|edge| {
            edge.kind == EdgeKind::Informs && edge.provenance == EdgeProvenance::Knowledge
        }) {
            let Some(definitions) = by_node.get(&edge.target) else {
                continue;
            };
            let attempted = archive.outcomes.iter().any(|outcome| {
                outcome_matches_node(outcome, &edge.target)
                    && outcome.attempt_count > 0
                    && event_backed(outcome)
            });
            for definition in definitions {
                let accumulator = accumulators.entry(definition.clone()).or_default();
                accumulator.gotcha_edges_eligible += 1;
                accumulator.gotcha_targets_attempted += usize::from(attempted);
                accumulator
                    .evidence_refs
                    .insert(format!("knowledge-edge:{}->{}", edge.source, edge.target));
            }
        }
    }

    let node_evidence_available = run.nodes.value().is_some();
    let review_evidence_available = run.reviews.value().is_some();
    let gotcha_evidence_available = knowledge_available && event_available;
    accumulators
        .into_iter()
        .map(|(definition, mut value)| {
            value
                .evidence_refs
                .insert(format!("archive:{}", archive.archive_id));
            RoleDefinitionRunMetric {
                definition,
                agent_ids: value.agent_ids.into_iter().collect(),
                node_ids: value.node_ids.into_iter().collect(),
                additional_attempts: node_evidence_available.then_some(value.additional_attempts),
                remediation_detours: node_evidence_available.then_some(value.remediation_detours),
                caught_defects: review_evidence_available.then_some(value.caught_defects),
                escaped_defects: review_evidence_available.then_some(value.escaped_defects),
                gotcha_edges_eligible: gotcha_evidence_available
                    .then_some(value.gotcha_edges_eligible),
                gotcha_targets_attempted: gotcha_evidence_available
                    .then_some(value.gotcha_targets_attempted),
                confirmed_scope_gaps: value.confirmed_scope_gaps,
                evidence_refs: value.evidence_refs.into_iter().collect(),
            }
        })
        .collect()
}

struct TaskTierMetricAccumulator {
    completion_count: usize,
    node_ids: BTreeSet<String>,
    additional_attempts: usize,
    remediation_detours: usize,
    caught_defects: usize,
    escaped_defects: usize,
    elapsed_millis: i64,
    elapsed_complete: bool,
    confirmed_scope_gaps: usize,
    evidence_refs: BTreeSet<String>,
}

impl Default for TaskTierMetricAccumulator {
    fn default() -> Self {
        Self {
            completion_count: 0,
            node_ids: BTreeSet::new(),
            additional_attempts: 0,
            remediation_detours: 0,
            caught_defects: 0,
            escaped_defects: 0,
            elapsed_millis: 0,
            elapsed_complete: true,
            confirmed_scope_gaps: 0,
            evidence_refs: BTreeSet::new(),
        }
    }
}

fn completion_fact_backed(outcome: &RuntimeOutcome) -> bool {
    outcome.executed_as.is_some()
        || outcome
            .source_refs
            .iter()
            .any(|source| source.starts_with("completion-fact:"))
}

fn task_tier_metrics(
    archive: &WorkGraphArchive,
    nodes: &EvidenceMetric<Vec<NodeExecutionMetric>>,
    reviews: &EvidenceMetric<Vec<ReviewEfficacyMetric>>,
) -> EvidenceMetric<Vec<TaskTierRunMetric>> {
    let completion_outcomes: Vec<_> = archive
        .outcomes
        .iter()
        .filter(|outcome| completion_fact_backed(outcome))
        .collect();
    if completion_outcomes.is_empty() {
        return EvidenceMetric::Unavailable {
            omissions: vec![RetroOmission::new(
                RetroOmissionReason::ExecutedAsUnavailable,
                "task_tier",
                "the archive has no completion fact with executed_as evidence",
                Vec::new(),
            )
            .for_archive(&archive.archive_id)],
        };
    }

    let additional_attempts_available = source_available(archive, ArchiveSourceKind::EventLog);
    let remediation_detours_available = source_available(archive, ArchiveSourceKind::MutationLog);
    let review_evidence_available = reviews.value().is_some();
    let remediation = remediation_counts(archive);
    let mut groups: BTreeMap<(TaskTier, String), TaskTierMetricAccumulator> = BTreeMap::new();
    let mut missing_executed_as = BTreeSet::new();
    let mut missing_timing = BTreeSet::new();

    for outcome in completion_outcomes {
        let Some(executed_as) = &outcome.executed_as else {
            missing_executed_as.insert(outcome.subject_id.clone());
            continue;
        };
        let accumulator = groups
            .entry((executed_as.tier, executed_as.provider.clone()))
            .or_default();
        accumulator.completion_count += 1;

        let mut node_ids = BTreeSet::from([outcome.subject_id.clone()]);
        if let Some(task_id) = &outcome.task_id {
            node_ids.insert(task_id.clone());
        }
        accumulator.node_ids.extend(node_ids.iter().cloned());
        // Match `node_metrics`: only event-backed outcomes contribute attempts. An
        // archive-level event log is necessary but not sufficient — a completion fact
        // carrying no `event:` source ref would otherwise inflate this tier's count
        // while leaving the per-node metric untouched.
        if additional_attempts_available && event_backed(outcome) {
            accumulator.additional_attempts += outcome.attempt_count.saturating_sub(1);
        }
        if remediation_detours_available {
            accumulator.remediation_detours += node_ids
                .iter()
                .map(|node_id| remediation.get(node_id).copied().unwrap_or(0))
                .sum::<usize>();
        }
        if let Some(review_values) = reviews.value() {
            for review in review_values
                .iter()
                .filter(|review| node_ids.contains(&review.verdict_id))
            {
                accumulator.caught_defects += review.caught_defects;
                accumulator.escaped_defects += review.escaped_defects;
                accumulator
                    .evidence_refs
                    .extend(review.evidence_refs.iter().cloned());
                accumulator.evidence_refs.extend(
                    review
                        .revisions
                        .iter()
                        .map(|revision| revision.source_ref.clone()),
                );
            }
        }
        accumulator.confirmed_scope_gaps += outcome
            .effects
            .iter()
            .filter(|effect| effect.confirmed && effect.kind == ROLE_SCOPE_GAP_EFFECT_KIND)
            .count();
        accumulator
            .evidence_refs
            .extend(outcome.source_refs.iter().cloned());
        accumulator.evidence_refs.extend(
            outcome
                .effects
                .iter()
                .filter(|effect| effect.confirmed)
                .map(|effect| effect.source_ref.clone()),
        );
        match (outcome.started_at, outcome.finished_at) {
            (Some(started), Some(finished)) if finished >= started => {
                accumulator.elapsed_millis += (finished - started).num_milliseconds();
            }
            _ => {
                accumulator.elapsed_complete = false;
                missing_timing.insert(outcome.subject_id.clone());
            }
        }
    }

    let values = groups
        .into_iter()
        .map(|((tier, provider), mut value)| {
            value
                .evidence_refs
                .insert(format!("archive:{}", archive.archive_id));
            TaskTierRunMetric {
                tier,
                provider,
                completion_count: value.completion_count,
                node_ids: value.node_ids.into_iter().collect(),
                additional_attempts: additional_attempts_available
                    .then_some(value.additional_attempts),
                remediation_detours: remediation_detours_available
                    .then_some(value.remediation_detours),
                caught_defects: review_evidence_available.then_some(value.caught_defects),
                escaped_defects: review_evidence_available.then_some(value.escaped_defects),
                elapsed_millis: value.elapsed_complete.then_some(value.elapsed_millis),
                confirmed_scope_gaps: value.confirmed_scope_gaps,
                evidence_refs: value.evidence_refs.into_iter().collect(),
            }
        })
        .collect::<Vec<_>>();

    let mut omissions = nodes
        .omissions()
        .iter()
        .chain(reviews.omissions())
        .cloned()
        .map(|mut omission| {
            omission.metric = "task_tier".to_string();
            omission
        })
        .collect::<Vec<_>>();
    if !missing_executed_as.is_empty() {
        omissions.push(
            RetroOmission::new(
                RetroOmissionReason::ExecutedAsUnavailable,
                "task_tier",
                "completion facts without executed_as were excluded from tier/provider buckets",
                missing_executed_as.into_iter().collect(),
            )
            .for_archive(&archive.archive_id),
        );
    }
    if !missing_timing.is_empty() {
        omissions.push(
            RetroOmission::new(
                RetroOmissionReason::TimingEvidenceIncomplete,
                "task_tier",
                "elapsed time is unavailable for buckets containing a completion without both timestamps",
                missing_timing.into_iter().collect(),
            )
            .for_archive(&archive.archive_id),
        );
    }
    if values.is_empty() {
        EvidenceMetric::Unavailable { omissions }
    } else if omissions.is_empty() {
        EvidenceMetric::Available { value: values }
    } else {
        EvidenceMetric::Partial {
            value: values,
            omissions,
        }
    }
}

fn edit_distance(archive: &WorkGraphArchive) -> EvidenceMetric<PlanActualEditDistance> {
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
    let mutation_omissions =
        source_report_omissions(archive, ArchiveSourceKind::MutationLog, "edit_distance");
    if mutation_omissions.is_empty() {
        EvidenceMetric::Available { value }
    } else {
        EvidenceMetric::Partial {
            value,
            omissions: mutation_omissions,
        }
    }
}

fn node_metrics(archive: &WorkGraphArchive) -> EvidenceMetric<Vec<NodeExecutionMetric>> {
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
        let before_ids: BTreeSet<_> = delta.before.nodes.iter().map(|node| &node.id).collect();
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

fn checkpoint_metrics(archive: &WorkGraphArchive) -> EvidenceMetric<Vec<CheckpointBarrierMetric>> {
    let Some(plan) = &archive.plan_graph else {
        return EvidenceMetric::Unavailable {
            omissions: vec![plan_omission(archive, "checkpoint_barrier")],
        };
    };
    let mut values = Vec::new();
    let mut omissions = Vec::new();
    for checkpoint in plan
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Checkpoint)
    {
        let mut prerequisite_ids: Vec<_> = plan
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::DependsOn && edge.target == checkpoint.id)
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

fn review_metrics(archive: &WorkGraphArchive) -> EvidenceMetric<Vec<ReviewEfficacyMetric>> {
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
            GraphMutationType::ReviewVerdictRecorded | GraphMutationType::RemediationDetour
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
            let review = reviews
                .entry(node.id.clone())
                .or_insert_with(|| ReviewEfficacyMetric {
                    verdict_id: node.id.clone(),
                    target_id: target_id.clone(),
                    state,
                    caught_defects: usize::from(state == ReviewEvidenceState::Caught),
                    escaped_defects: 0,
                    remediation_detours: remediation.get(target_id).copied().unwrap_or(0),
                    evidence_refs: Vec::new(),
                    revisions: Vec::new(),
                });
            if state == ReviewEvidenceState::Caught {
                review.state = state;
                review.caught_defects = 1;
            }
            review
                .evidence_refs
                .extend(delta.source_refs.iter().cloned());
            review
                .evidence_refs
                .push(format!("mutation:delta:{}", delta.sequence));
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

fn gotcha_hit_rate(archive: &WorkGraphArchive) -> EvidenceMetric<GotchaEdgeHitRate> {
    let knowledge_missing =
        archive.runtime_graph.omissions.iter().any(|omission| {
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
            edge.kind == EdgeKind::Informs && edge.provenance == EdgeProvenance::Knowledge
        })
        .collect();
    if eligible.is_empty() {
        return EvidenceMetric::Partial {
            value: GotchaEdgeHitRate {
                eligible_knowledge_edges: 0,
                targets_attempted: 0,
                rate_defined: false,
            },
            omissions: vec![RetroOmission::new(
                RetroOmissionReason::NoEligibleEdges,
                "gotcha_edge_hit_rate",
                "the archived graph has no eligible knowledge edges, so a hit rate is undefined",
                Vec::new(),
            )
            .for_archive(&archive.archive_id)],
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
    let mut omissions =
        source_report_omissions(archive, ArchiveSourceKind::EventLog, "gotcha_edge_hit_rate");
    if graph_resolution_incomplete(archive) {
        omissions.push(source_omission(
            archive,
            RetroOmissionReason::ResolutionIncomplete,
            "gotcha_edge_hit_rate",
            "unresolved runtime observations may undercount attempted informed targets",
        ));
    }
    if !omissions.is_empty() {
        EvidenceMetric::Partial { value, omissions }
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
        .filter_map(|run| {
            run.template
                .clone()
                .map(|key| (run.archive_id.clone(), key))
        })
        .collect();
    let mut groups: BTreeMap<(TemplateKey, String), Vec<DeviationOccurrence>> = BTreeMap::new();
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
        let repo_ids: BTreeSet<_> = occurrences
            .iter()
            .map(|item| item.repo_id.clone())
            .collect();
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
    (kinds.len() == 1)
        .then(|| kinds.into_iter().next())
        .flatten()
}

fn role_refinement_observations(runs: &[PerRunRetro]) -> Vec<RoleDefinitionRefinementObservation> {
    let mut observations = Vec::new();
    for run in runs {
        for metric in &run.role_definitions {
            let signals = [
                (
                    RoleRefinementSignal::AdditionalAttempts,
                    metric.additional_attempts.unwrap_or(0) > 0,
                ),
                (
                    RoleRefinementSignal::RemediationDetours,
                    metric.remediation_detours.unwrap_or(0) > 0,
                ),
                (
                    RoleRefinementSignal::ReviewEscapes,
                    metric.escaped_defects.unwrap_or(0) > 0,
                ),
                (
                    RoleRefinementSignal::UnusedKnowledgeScope,
                    metric.gotcha_edges_eligible.unwrap_or(0) > 0
                        && metric.gotcha_targets_attempted == Some(0),
                ),
                (
                    RoleRefinementSignal::DeclaredScopeGap,
                    metric.confirmed_scope_gaps > 0,
                ),
            ];
            for (signal, observed) in signals {
                if !observed {
                    continue;
                }
                observations.push(RoleDefinitionRefinementObservation {
                    repo_id: run.repo_id.clone(),
                    session_id: run.session_id.clone(),
                    archive_id: run.archive_id.clone(),
                    definition: metric.definition.clone(),
                    signal,
                    evidence_refs: metric.evidence_refs.clone(),
                });
            }
        }
    }
    observations
}

/// Return review-gated proposals only. The function has no definition path or
/// git handle and therefore cannot apply or commit the proposed refinement.
pub fn propose_role_definition_refinements(
    observations: &[RoleDefinitionRefinementObservation],
) -> Vec<RoleDefinitionRefinementProposal> {
    let mut groups: BTreeMap<
        (RoleDefinitionKey, RoleRefinementSignal),
        BTreeMap<(String, String), &RoleDefinitionRefinementObservation>,
    > = BTreeMap::new();
    for observation in observations {
        groups
            .entry((observation.definition.clone(), observation.signal))
            .or_default()
            .entry((observation.repo_id.clone(), observation.session_id.clone()))
            .or_insert(observation);
    }

    let mut proposals = Vec::new();
    for ((definition, signal), instances) in groups {
        let repo_ids: BTreeSet<_> = instances
            .values()
            .map(|item| item.repo_id.clone())
            .collect();
        if repo_ids.len() < 2 && instances.len() < PROMOTION_RUN_THRESHOLD {
            continue;
        }
        let tier = if repo_ids.len() >= 2 {
            PromotionTier::InstitutionalRevision
        } else {
            PromotionTier::ProjectOverride
        };
        let session_ids: BTreeSet<_> = instances
            .values()
            .map(|item| item.session_id.clone())
            .collect();
        let archive_ids: BTreeSet<_> = instances
            .values()
            .map(|item| item.archive_id.clone())
            .collect();
        let evidence_refs: BTreeSet<_> = instances
            .values()
            .flat_map(|item| item.evidence_refs.iter().cloned())
            .collect();
        let scope_impact = signal.scope_impact();
        proposals.push(RoleDefinitionRefinementProposal {
            tier,
            definition,
            change_key: signal.change_key().to_string(),
            scope_impact,
            observation_count: instances.len(),
            repo_ids: repo_ids.into_iter().collect(),
            session_ids: session_ids.into_iter().collect(),
            archive_ids: archive_ids.into_iter().collect(),
            evidence_refs: evidence_refs.into_iter().collect(),
            rationale: match (tier, scope_impact) {
                (PromotionTier::ProjectOverride, ScopeImpact::Widening) => {
                    "repeated evidence in one repository suggests a wider role scope; widening is suspect by default and requires human review as a Tier 1 override".to_string()
                }
                (PromotionTier::InstitutionalRevision, ScopeImpact::Widening) => {
                    "independent evidence across repositories suggests a wider role scope; widening is suspect by default and requires a PR-gated Tier 2 review".to_string()
                }
                (PromotionTier::ProjectOverride, _) => {
                    "repeated evidence in one repository suggests a narrower or behavior-only role refinement for human review as a Tier 1 override".to_string()
                }
                (PromotionTier::InstitutionalRevision, _) => {
                    "independent evidence across repositories suggests a role refinement for PR-gated Tier 2 review".to_string()
                }
            },
        });
    }
    proposals
}

fn role_refinement_learnings(
    proposals: &[RoleDefinitionRefinementProposal],
) -> Vec<UnreviewedLearningSubmission> {
    proposals
        .iter()
        .filter_map(|proposal| {
            let session = proposal.session_ids.last()?.clone();
            let definition = format!(
                "{}@{}",
                proposal.definition.definition_id,
                proposal.definition.definition_version
            );
            Some(UnreviewedLearningSubmission {
                session,
                task: "post-run role-definition refinement proposal".to_string(),
                outcome: UNREVIEWED_OUTCOME.to_string(),
                keywords: vec![
                    "role-definition".to_string(),
                    definition.clone(),
                    proposal.change_key.clone(),
                    match proposal.scope_impact {
                        ScopeImpact::Narrowing => "narrowing",
                        ScopeImpact::Widening => "widening",
                        ScopeImpact::NoScopeChange => "no-scope-change",
                    }
                    .to_string(),
                ],
                insight: format!(
                    "Archived evidence proposes an unreviewed {} refinement for {definition} from {} independent session instance(s). The definition was not modified; evidence: {}.",
                    proposal.change_key,
                    proposal.observation_count,
                    proposal.evidence_refs.join(", ")
                ),
                files_touched: Vec::new(),
            })
        })
        .collect()
}

#[derive(Clone)]
struct PlannerCalibrationSample {
    repo_id: String,
    session_id: String,
    archive_id: String,
    task_tier: TaskTier,
    provider: String,
    remediated: bool,
    node_ids: BTreeSet<String>,
    evidence_refs: BTreeSet<String>,
}

#[derive(Clone)]
struct LadderCalibrationSample {
    repo_id: String,
    session_id: String,
    archive_id: String,
    task_tier: TaskTier,
    provider: String,
    model: String,
    flags: Vec<String>,
    source: TierResolutionSource,
    caught_defects: usize,
    escaped_defects: usize,
    node_ids: BTreeSet<String>,
    evidence_refs: BTreeSet<String>,
}

fn next_task_tier(tier: TaskTier) -> Option<TaskTier> {
    match tier {
        TaskTier::Low => Some(TaskTier::Medium),
        TaskTier::Medium => Some(TaskTier::High),
        TaskTier::High => Some(TaskTier::Critical),
        TaskTier::Critical => None,
    }
}

fn outcome_node_ids(outcome: &RuntimeOutcome) -> BTreeSet<String> {
    let mut node_ids = BTreeSet::from([outcome.subject_id.clone()]);
    if let Some(task_id) = &outcome.task_id {
        node_ids.insert(task_id.clone());
    }
    node_ids
}

fn task_tier_calibration_observations(
    inputs: &[RetroRunInput],
    runs: &[PerRunRetro],
) -> Vec<TaskTierCalibrationObservation> {
    let mut planner_samples = Vec::new();
    let mut ladder_samples = Vec::new();
    for (input, run) in inputs.iter().zip(runs) {
        let remediation = remediation_counts(&input.archive);
        let mut planner_by_key: BTreeMap<(TaskTier, String), PlannerCalibrationSample> =
            BTreeMap::new();
        let review_values = run.reviews.value();
        for outcome in input
            .archive
            .outcomes
            .iter()
            .filter(|outcome| completion_fact_backed(outcome))
        {
            let Some(executed_as) = &outcome.executed_as else {
                continue;
            };
            let node_ids = outcome_node_ids(outcome);
            let mut evidence_refs: BTreeSet<_> = outcome.source_refs.iter().cloned().collect();
            evidence_refs.insert(format!("archive:{}", input.archive.archive_id));

            if executed_as.source == TierResolutionSource::Node
                && matches!(executed_as.tier, TaskTier::Low | TaskTier::Medium)
            {
                let sample = planner_by_key
                    .entry((executed_as.tier, executed_as.provider.clone()))
                    .or_insert_with(|| PlannerCalibrationSample {
                        repo_id: input.repo_id.clone(),
                        session_id: input.archive.session_id.clone(),
                        archive_id: input.archive.archive_id.clone(),
                        task_tier: executed_as.tier,
                        provider: executed_as.provider.clone(),
                        remediated: false,
                        node_ids: BTreeSet::new(),
                        evidence_refs: BTreeSet::new(),
                    });
                sample.remediated |= node_ids
                    .iter()
                    .any(|node_id| remediation.get(node_id).copied().unwrap_or(0) > 0);
                sample.node_ids.extend(node_ids.iter().cloned());
                sample.evidence_refs.extend(evidence_refs.iter().cloned());
            }

            if matches!(
                executed_as.source,
                TierResolutionSource::Node | TierResolutionSource::Override
            ) {
                let Some(review_values) = review_values else {
                    continue;
                };
                let mut caught_defects = 0;
                let mut escaped_defects = 0;
                for review in review_values
                    .iter()
                    .filter(|review| node_ids.contains(&review.verdict_id))
                {
                    caught_defects += review.caught_defects;
                    escaped_defects += review.escaped_defects;
                    evidence_refs.extend(review.evidence_refs.iter().cloned());
                    evidence_refs.extend(
                        review
                            .revisions
                            .iter()
                            .map(|revision| revision.source_ref.clone()),
                    );
                }
                ladder_samples.push(LadderCalibrationSample {
                    repo_id: input.repo_id.clone(),
                    session_id: input.archive.session_id.clone(),
                    archive_id: input.archive.archive_id.clone(),
                    task_tier: executed_as.tier,
                    provider: executed_as.provider.clone(),
                    model: executed_as.model.clone(),
                    flags: executed_as.flags.clone(),
                    source: executed_as.source,
                    caught_defects,
                    escaped_defects,
                    node_ids,
                    evidence_refs,
                });
            }
        }
        planner_samples.extend(planner_by_key.into_values());
    }

    let mut observations = Vec::new();
    let mut planner_groups: BTreeMap<(TaskTier, String), Vec<PlannerCalibrationSample>> =
        BTreeMap::new();
    for sample in planner_samples {
        planner_groups
            .entry((sample.task_tier, sample.provider.clone()))
            .or_default()
            .push(sample);
    }
    for ((_task_tier, _provider), samples) in planner_groups {
        let remediated = samples.iter().filter(|sample| sample.remediated).count();
        if remediated * 2 <= samples.len() {
            continue;
        }
        for sample in samples.into_iter().filter(|sample| sample.remediated) {
            observations.push(TaskTierCalibrationObservation {
                repo_id: sample.repo_id,
                session_id: sample.session_id,
                archive_id: sample.archive_id,
                target: TaskTierCalibrationTarget::Planner,
                task_tier: sample.task_tier,
                provider: sample.provider,
                candidate_tier: next_task_tier(sample.task_tier),
                candidate_model: None,
                candidate_flags: Vec::new(),
                node_ids: sample.node_ids.into_iter().collect(),
                evidence_refs: sample.evidence_refs.into_iter().collect(),
            });
        }
    }

    type LadderVariantKey = (TaskTier, String, String, Vec<String>);
    let mut baseline_groups: BTreeMap<LadderVariantKey, Vec<LadderCalibrationSample>> =
        BTreeMap::new();
    let mut candidate_groups: BTreeMap<LadderVariantKey, Vec<LadderCalibrationSample>> =
        BTreeMap::new();
    for sample in ladder_samples {
        let key = (
            sample.task_tier,
            sample.provider.clone(),
            sample.model.clone(),
            sample.flags.clone(),
        );
        match sample.source {
            TierResolutionSource::Node => baseline_groups.entry(key).or_default().push(sample),
            TierResolutionSource::Override => candidate_groups.entry(key).or_default().push(sample),
            TierResolutionSource::Fallback => {}
        }
    }
    for ((task_tier, provider, candidate_model, candidate_flags), candidates) in candidate_groups {
        if candidates.len() < PROMOTION_RUN_THRESHOLD {
            continue;
        }
        let candidate_caught: usize = candidates.iter().map(|sample| sample.caught_defects).sum();
        let candidate_escaped: usize = candidates.iter().map(|sample| sample.escaped_defects).sum();
        let candidate_total = candidate_caught + candidate_escaped;
        if candidate_total == 0 {
            continue;
        }
        for ((baseline_tier, baseline_provider, baseline_model, baseline_flags), baselines) in
            &baseline_groups
        {
            if *baseline_tier != task_tier
                || baseline_provider != &provider
                || baselines.len() < PROMOTION_RUN_THRESHOLD
                || (baseline_model == &candidate_model && baseline_flags == &candidate_flags)
            {
                continue;
            }
            let baseline_caught: usize = baselines.iter().map(|sample| sample.caught_defects).sum();
            let baseline_escaped: usize =
                baselines.iter().map(|sample| sample.escaped_defects).sum();
            let baseline_total = baseline_caught + baseline_escaped;
            if baseline_total == 0
                || candidate_escaped * baseline_total != baseline_escaped * candidate_total
            {
                continue;
            }
            let baseline_node_ids: BTreeSet<_> = baselines
                .iter()
                .flat_map(|sample| sample.node_ids.iter().cloned())
                .collect();
            let baseline_evidence_refs: BTreeSet<_> = baselines
                .iter()
                .flat_map(|sample| sample.evidence_refs.iter().cloned())
                .collect();
            for candidate in &candidates {
                let mut node_ids = candidate.node_ids.clone();
                node_ids.extend(baseline_node_ids.iter().cloned());
                let mut evidence_refs = candidate.evidence_refs.clone();
                evidence_refs.extend(baseline_evidence_refs.iter().cloned());
                observations.push(TaskTierCalibrationObservation {
                    repo_id: candidate.repo_id.clone(),
                    session_id: candidate.session_id.clone(),
                    archive_id: candidate.archive_id.clone(),
                    target: TaskTierCalibrationTarget::Ladder,
                    task_tier,
                    provider: provider.clone(),
                    candidate_tier: None,
                    candidate_model: Some(candidate_model.clone()),
                    candidate_flags: candidate_flags.clone(),
                    node_ids: node_ids.into_iter().collect(),
                    evidence_refs: evidence_refs.into_iter().collect(),
                });
            }
            break;
        }
    }
    observations
}

fn task_tier_calibration_learnings<'a>(
    proposals: impl IntoIterator<Item = &'a TaskTierCalibrationProposal>,
) -> Vec<UnreviewedLearningSubmission> {
    proposals
        .into_iter()
        .filter_map(|proposal| {
            let session = proposal.session_ids.last()?.clone();
            let target = match proposal.target {
                TaskTierCalibrationTarget::Planner => "planner",
                TaskTierCalibrationTarget::Ladder => "ladder",
            };
            Some(UnreviewedLearningSubmission {
                session,
                task: format!("post-run task-tier {target} calibration proposal"),
                outcome: UNREVIEWED_OUTCOME.to_string(),
                keywords: vec![
                    "task-tier".to_string(),
                    format!("{target}-calibration"),
                    proposal.provider.clone(),
                    format!("{:?}", proposal.task_tier).to_lowercase(),
                ],
                insight: format!(
                    "Archived evidence produced an unreviewed {target} calibration proposal from {} independent run instance(s). No planner rubric or ladder cell was modified; evidence: {}.",
                    proposal.observation_count,
                    proposal.evidence_refs.join(", ")
                ),
                files_touched: Vec::new(),
            })
        })
        .collect()
}

fn add_optional(total: &mut Option<usize>, contributing_runs: &mut usize, value: Option<usize>) {
    if let Some(value) = value {
        *total.get_or_insert(0) += value;
        *contributing_runs += 1;
    }
}

fn aggregate_role_definitions(runs: &[PerRunRetro]) -> Vec<RoleDefinitionAggregate> {
    let mut groups: BTreeMap<RoleDefinitionKey, RoleDefinitionAggregate> = BTreeMap::new();
    for run in runs {
        for metric in &run.role_definitions {
            let aggregate = groups.entry(metric.definition.clone()).or_insert_with(|| {
                RoleDefinitionAggregate {
                    definition: metric.definition.clone(),
                    run_count: 0,
                    repo_ids: Vec::new(),
                    session_ids: Vec::new(),
                    archive_ids: Vec::new(),
                    additional_attempts: None,
                    additional_attempts_contributing_runs: 0,
                    remediation_detours: None,
                    remediation_detours_contributing_runs: 0,
                    caught_defects: None,
                    caught_defects_contributing_runs: 0,
                    escaped_defects: None,
                    escaped_defects_contributing_runs: 0,
                    gotcha_edges_eligible: None,
                    gotcha_edges_eligible_contributing_runs: 0,
                    gotcha_targets_attempted: None,
                    gotcha_targets_attempted_contributing_runs: 0,
                    confirmed_scope_gaps: 0,
                }
            });
            aggregate.run_count += 1;
            aggregate.repo_ids.push(run.repo_id.clone());
            aggregate.session_ids.push(run.session_id.clone());
            aggregate.archive_ids.push(run.archive_id.clone());
            add_optional(
                &mut aggregate.additional_attempts,
                &mut aggregate.additional_attempts_contributing_runs,
                metric.additional_attempts,
            );
            add_optional(
                &mut aggregate.remediation_detours,
                &mut aggregate.remediation_detours_contributing_runs,
                metric.remediation_detours,
            );
            add_optional(
                &mut aggregate.caught_defects,
                &mut aggregate.caught_defects_contributing_runs,
                metric.caught_defects,
            );
            add_optional(
                &mut aggregate.escaped_defects,
                &mut aggregate.escaped_defects_contributing_runs,
                metric.escaped_defects,
            );
            add_optional(
                &mut aggregate.gotcha_edges_eligible,
                &mut aggregate.gotcha_edges_eligible_contributing_runs,
                metric.gotcha_edges_eligible,
            );
            add_optional(
                &mut aggregate.gotcha_targets_attempted,
                &mut aggregate.gotcha_targets_attempted_contributing_runs,
                metric.gotcha_targets_attempted,
            );
            aggregate.confirmed_scope_gaps += metric.confirmed_scope_gaps;
        }
    }
    groups
        .into_values()
        .map(|mut aggregate| {
            aggregate.repo_ids.sort();
            aggregate.repo_ids.dedup();
            aggregate.session_ids.sort();
            aggregate.session_ids.dedup();
            aggregate.archive_ids.sort();
            aggregate.archive_ids.dedup();
            aggregate
        })
        .collect()
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
                *aggregate.gotcha_targets_attempted.get_or_insert(0) += gotcha.targets_attempted;
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
    RetroOmission::new(reason, metric, detail, Vec::new()).for_archive(&archive.archive_id)
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

#[cfg(test)]
mod task_tier_tests {
    use super::*;
    use std::fs;

    use chrono::TimeZone;
    use tempfile::TempDir;

    use crate::domain::run_journal::Confidence;
    use crate::http::handlers::workers::{ExecutedAs, ExecutionChannel};
    use crate::orchestrator::work_graph::archive::{
        ArchiveSourceReport, WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
    };
    use crate::orchestrator::work_graph::divergence::DivergenceSummary;
    use crate::orchestrator::work_graph::runtime::{
        GraphMutationDelta, RuntimeEffect, RuntimeOutcomeStatus,
    };
    use crate::orchestrator::work_graph::{BindingRef, CompositeExpansion, NodeContract};

    fn instant(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_900_000_000 + seconds, 0)
            .single()
            .expect("fixture timestamp")
    }

    fn source(kind: ArchiveSourceKind) -> ArchiveSourceReport {
        ArchiveSourceReport {
            kind,
            location: format!("fixture/{kind:?}"),
            available: true,
            record_count: 1,
            omissions: Vec::new(),
        }
    }

    fn sources() -> Vec<ArchiveSourceReport> {
        vec![
            source(ArchiveSourceKind::PlanGraph),
            source(ArchiveSourceKind::EventLog),
            source(ArchiveSourceKind::RunJournal),
            source(ArchiveSourceKind::RunLedger),
            source(ArchiveSourceKind::MutationLog),
        ]
    }

    fn task_node(id: &str, tier: TaskTier) -> WorkNode {
        let mut node = WorkNode::new(
            id,
            NodeKind::Task,
            format!("Node {id}"),
            NodeContract::default(),
            BindingRef::Role("worker".to_string()),
            NodeStatus::Pending,
        );
        node.tier = tier;
        node
    }

    fn executed_as(
        provider: &str,
        tier: TaskTier,
        model: &str,
        source: TierResolutionSource,
    ) -> ExecutedAs {
        ExecutedAs {
            provider: provider.to_string(),
            tier,
            model: model.to_string(),
            flags: vec![format!("effort={tier:?}").to_lowercase()],
            channel: ExecutionChannel::Hive,
            source,
        }
    }

    fn completion_outcome(
        id: &str,
        execution: Option<ExecutedAs>,
        effects: Vec<RuntimeEffect>,
    ) -> RuntimeOutcome {
        RuntimeOutcome {
            subject_id: id.to_string(),
            task_id: Some(id.to_string()),
            agent_ids: vec![format!("agent-{id}")],
            executed_as: execution,
            status: RuntimeOutcomeStatus::Completed,
            started_at: Some(instant(1)),
            finished_at: Some(instant(11)),
            attempt_count: 1,
            effects,
            source_refs: vec![format!("completion-fact:{id}")],
            completion_evidence: None,
        }
    }

    fn archive(
        archive_id: &str,
        session_id: &str,
        plan_graph: TaskGraph,
        runtime_graph: TaskGraph,
        deltas: Vec<GraphMutationDelta>,
        outcomes: Vec<RuntimeOutcome>,
    ) -> WorkGraphArchive {
        WorkGraphArchive {
            schema_version: WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
            archive_id: archive_id.to_string(),
            session_id: session_id.to_string(),
            archived_at: instant(20),
            plan_graph: Some(plan_graph),
            runtime_graph,
            deltas,
            outcomes,
            divergence: DivergenceSummary::default(),
            sources: sources(),
        }
    }

    fn evaluator() -> IndependentEvaluator {
        IndependentEvaluator::new(
            "independent-tier-retro",
            vec!["planner".to_string()],
            vec!["supervisor".to_string()],
        )
        .expect("independent evaluator")
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn visit(root: &Path, current: &Path, snapshot: &mut BTreeMap<String, Vec<u8>>) {
            let mut entries = fs::read_dir(current)
                .expect("read fixture tree")
                .map(|entry| entry.expect("fixture entry").path())
                .collect::<Vec<_>>();
            entries.sort();
            for path in entries {
                if path.is_dir() {
                    visit(root, &path, snapshot);
                } else {
                    let relative = path
                        .strip_prefix(root)
                        .expect("fixture path below root")
                        .to_string_lossy()
                        .replace('\\', "/");
                    snapshot.insert(relative, fs::read(path).expect("read fixture file"));
                }
            }
        }

        let mut snapshot = BTreeMap::new();
        visit(root, root, &mut snapshot);
        snapshot
    }

    #[test]
    fn task_tier_metrics_are_keyed_by_tier_and_provider_and_report_partial_legacy_evidence() {
        let specs = [
            ("low-codex-a", TaskTier::Low, "codex", Some("terra")),
            ("low-codex-b", TaskTier::Low, "codex", Some("terra")),
            ("low-claude", TaskTier::Low, "claude", Some("haiku")),
            ("high-codex", TaskTier::High, "codex", Some("sol")),
            ("high-claude", TaskTier::High, "claude", Some("opus")),
            ("legacy", TaskTier::Low, "codex", None),
        ];
        let nodes = specs
            .iter()
            .map(|(id, tier, _, _)| task_node(id, *tier))
            .collect::<Vec<_>>();
        let graph = TaskGraph::new(nodes, Vec::new());
        let outcomes = specs
            .iter()
            .map(|(id, tier, provider, model)| {
                completion_outcome(
                    id,
                    model.map(|model| {
                        executed_as(provider, *tier, model, TierResolutionSource::Node)
                    }),
                    Vec::new(),
                )
            })
            .collect();
        let report = evaluate_archives(
            &evaluator(),
            &[RetroRunInput {
                repo_id: "repo-a".to_string(),
                archive: archive(
                    "tier-archive",
                    "tier-session",
                    graph.clone(),
                    graph,
                    Vec::new(),
                    outcomes,
                ),
            }],
        )
        .expect("tier retro evaluates");

        let EvidenceMetric::Partial { value, omissions } = &report.runs[0].task_tiers else {
            panic!("one legacy completion must make the tier metric partial");
        };
        let keys: BTreeSet<_> = value
            .iter()
            .map(|metric| (metric.tier, metric.provider.as_str()))
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                (TaskTier::Low, "claude"),
                (TaskTier::Low, "codex"),
                (TaskTier::High, "claude"),
                (TaskTier::High, "codex"),
            ])
        );
        assert_eq!(value.len(), 4);
        assert!(omissions.iter().any(|omission| {
            omission.reason == RetroOmissionReason::ExecutedAsUnavailable
                && omission.examples == vec!["legacy"]
        }));
        assert_eq!(
            serde_json::to_string(&RetroOmissionReason::ExecutedAsUnavailable).unwrap(),
            "\"executed_as_unavailable\""
        );
    }

    fn planner_archive(index: usize, remediated: bool) -> WorkGraphArchive {
        let task_id = format!("planner-task-{index}");
        let plan = TaskGraph::new(vec![task_node(&task_id, TaskTier::Low)], Vec::new());
        let mut runtime = plan.clone();
        let deltas = if remediated {
            let mut remediation = task_node(&format!("remediation-{index}"), TaskTier::Medium);
            remediation.expansion = Some(CompositeExpansion {
                template: JUDGE_PRINCE_REMEDIATION_TEMPLATE.to_string(),
                parameters: BTreeMap::from([("target".to_string(), task_id.clone())]),
            });
            runtime.nodes.push(remediation);
            vec![GraphMutationDelta {
                sequence: 1,
                observed_at: instant(12),
                mutation_type: GraphMutationType::RemediationDetour,
                before: plan.clone(),
                after: runtime.clone(),
                source_refs: vec![format!("mutation:planner-{index}")],
            }]
        } else {
            Vec::new()
        };
        archive(
            &format!("planner-archive-{index}"),
            &format!("planner-session-{index}"),
            plan,
            runtime,
            deltas,
            vec![completion_outcome(
                &task_id,
                Some(executed_as(
                    "codex",
                    TaskTier::Low,
                    "gpt-5.6-terra",
                    TierResolutionSource::Node,
                )),
                Vec::new(),
            )],
        )
    }

    fn ladder_archive(index: usize, source: TierResolutionSource, model: &str) -> WorkGraphArchive {
        let verdict_id = format!("review-{index}");
        let mut planned_review = task_node(&verdict_id, TaskTier::High);
        planned_review.kind = NodeKind::Join;
        planned_review.expansion = Some(CompositeExpansion {
            template: MULTI_LENS_REVIEW_TEMPLATE.to_string(),
            parameters: BTreeMap::from([("target".to_string(), format!("target-{index}"))]),
        });
        let plan = TaskGraph::new(vec![planned_review], Vec::new());
        let mut runtime = plan.clone();
        runtime.nodes[0].status = NodeStatus::Failed;
        let delta = GraphMutationDelta {
            sequence: 1,
            observed_at: instant(12),
            mutation_type: GraphMutationType::ReviewVerdictRecorded,
            before: plan.clone(),
            after: runtime.clone(),
            source_refs: vec![format!("mutation:review-{index}")],
        };
        archive(
            &format!("ladder-archive-{index}"),
            &format!("ladder-session-{index}"),
            plan,
            runtime,
            vec![delta],
            vec![completion_outcome(
                &verdict_id,
                Some(executed_as("codex", TaskTier::High, model, source)),
                vec![RuntimeEffect {
                    kind: "review_observed".to_string(),
                    reference: None,
                    confirmed: true,
                    confidence: Confidence::High,
                    source_ref: format!("effect:review-{index}"),
                }],
            )],
        )
    }

    #[test]
    fn retro_emits_propose_only_planner_and_ladder_calibrations() {
        let fixture = TempDir::new().expect("temp tree");
        let project = fixture.path().join("project");
        let wiki = fixture.path().join("wiki");
        fs::create_dir_all(project.join(".ai-docs")).unwrap();
        fs::create_dir_all(wiki.join("tools")).unwrap();
        fs::write(project.join(".ai-docs").join("sentinel"), b"project").unwrap();
        fs::write(wiki.join("tools").join("sentinel"), b"wiki").unwrap();
        let before_project = snapshot_tree(&project);
        let before_wiki = snapshot_tree(&wiki);

        let mut inputs = (0..5)
            .map(|index| RetroRunInput {
                repo_id: "repo-planner".to_string(),
                archive: planner_archive(index, index < 3),
            })
            .collect::<Vec<_>>();
        inputs.extend((0..2).map(|index| RetroRunInput {
            repo_id: "repo-ladder".to_string(),
            archive: ladder_archive(index, TierResolutionSource::Node, "gpt-5.6-sol"),
        }));
        inputs.extend((2..4).map(|index| RetroRunInput {
            repo_id: "repo-ladder".to_string(),
            archive: ladder_archive(index, TierResolutionSource::Override, "gpt-5.6-terra"),
        }));

        let report = evaluate_archives(&evaluator(), &inputs).expect("calibration retro evaluates");

        assert!(report.planner_calibration_proposals.iter().any(|proposal| {
            proposal.target == TaskTierCalibrationTarget::Planner
                && proposal.task_tier == TaskTier::Low
                && proposal.provider == "codex"
                && proposal.candidate_tier == Some(TaskTier::Medium)
                && proposal.observation_count == 3
        }));
        assert!(report.ladder_calibration_proposals.iter().any(|proposal| {
            proposal.target == TaskTierCalibrationTarget::Ladder
                && proposal.task_tier == TaskTier::High
                && proposal.provider == "codex"
                && proposal.candidate_model.as_deref() == Some("gpt-5.6-terra")
                && proposal.observation_count == 2
        }));
        assert_eq!(snapshot_tree(&project), before_project);
        assert_eq!(snapshot_tree(&wiki), before_wiki);
    }
}
