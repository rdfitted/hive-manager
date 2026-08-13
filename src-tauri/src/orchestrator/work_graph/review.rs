//! Review subgraphs for issue #213, owned by WS-6.
//!
//! Reviews are expanded from node-class templates rather than handwritten into
//! every plan. Remediation is bounded and unrolled into later review rounds, so
//! the conceptual verdict -> remediation -> review loop never introduces a
//! dependency back-edge into the schedulable graph.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::{
    BindingRef, CompositeExpansion, EdgeKind, EdgeProvenance, NodeContract, NodeKind,
    NodeStatus, TaskGraph, TaskId, WorkEdge, WorkNode,
};

pub const MULTI_LENS_REVIEW_TEMPLATE: &str = "multi-lens-review";
pub const JUDGE_PRINCE_REMEDIATION_TEMPLATE: &str = "judge-prince-remediation";
const MAX_REMEDIATION_ITERATIONS: u8 = 8;

/// One deliberately distinct way to inspect a target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewLens {
    pub id: String,
    pub focus: String,
    #[serde(default)]
    pub acceptance: Vec<String>,
    pub binding: BindingRef,
}

impl ReviewLens {
    pub fn new(id: impl Into<String>, focus: impl Into<String>) -> Self {
        let focus = focus.into();
        Self {
            id: id.into(),
            acceptance: vec![focus.clone()],
            focus,
            binding: BindingRef::Role("evaluator".to_string()),
        }
    }
}

/// A plan-level declaration applied automatically to every matching node.
///
/// `target_kind` selects the node class. `required_output` can narrow that
/// class, for example to `Task` nodes whose contracts produce `code`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewTemplate {
    pub id: String,
    pub target_kind: NodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_output: Option<String>,
    pub lenses: Vec<ReviewLens>,
    #[serde(default)]
    pub rubric: Vec<String>,
    pub verdict_binding: BindingRef,
    pub remediation_binding: BindingRef,
    #[serde(default = "default_remediation_iterations")]
    pub remediation_iterations: u8,
}

const fn default_remediation_iterations() -> u8 {
    1
}

impl ReviewTemplate {
    /// The default code-review policy uses genuinely different lenses rather
    /// than repeating the same reviewer N times.
    pub fn code_tasks(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            target_kind: NodeKind::Task,
            required_output: Some("code".to_string()),
            lenses: vec![
                ReviewLens::new(
                    "correctness",
                    "verify behavior and evidence against the target acceptance contract",
                ),
                ReviewLens::new(
                    "security",
                    "look for trust-boundary, authorization, and unsafe-input failures",
                ),
                ReviewLens::new(
                    "regression",
                    "look for compatibility breaks and behavior lost outside the happy path",
                ),
            ],
            rubric: vec![
                "cite evidence for every lens finding".to_string(),
                "emit pass or the exact failing rubric items".to_string(),
            ],
            verdict_binding: BindingRef::Role("evaluator".to_string()),
            remediation_binding: BindingRef::Role("prince".to_string()),
            remediation_iterations: default_remediation_iterations(),
        }
    }
}

/// Generated identifiers for one target's bounded review expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewExpansion {
    pub template_id: String,
    pub target_id: TaskId,
    pub rounds: Vec<ReviewRoundExpansion>,
    pub remediation_ids: Vec<TaskId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRoundExpansion {
    pub lens_ids: Vec<TaskId>,
    pub verdict_id: TaskId,
}

/// An optional wave boundary. The false default is load-bearing: ordinary
/// readiness gating remains the default because barriers make fast siblings
/// wait for the slowest sibling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointWave {
    pub id: TaskId,
    pub title: String,
    #[serde(default)]
    pub checkpoint: bool,
    #[serde(default)]
    pub prerequisites: Vec<TaskId>,
    #[serde(default)]
    pub downstream: Vec<TaskId>,
    pub binding: BindingRef,
    #[serde(default)]
    pub acceptance: Vec<String>,
}

impl CheckpointWave {
    pub fn new(
        id: impl Into<TaskId>,
        prerequisites: Vec<TaskId>,
        downstream: Vec<TaskId>,
    ) -> Self {
        let id = id.into();
        Self {
            title: format!("Checkpoint {id}"),
            id,
            checkpoint: false,
            prerequisites,
            downstream,
            binding: BindingRef::Zone("checkpoint".to_string()),
            acceptance: vec!["the checkpoint gate passes".to_string()],
        }
    }

    pub fn enabled(mut self) -> Self {
        self.checkpoint = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewGraphError {
    EmptyTemplateId,
    TooFewLenses { template: String },
    DuplicateLensId { template: String, lens: String },
    DuplicateLensFocus { template: String, focus: String },
    TooManyRemediationIterations { template: String, requested: u8 },
    RemediationLimitReached { target: TaskId, limit: u8 },
    TemplateMismatch { expected: String, actual: String },
    DuplicateNodeId(TaskId),
    UnknownNode(TaskId),
}

impl fmt::Display for ReviewGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTemplateId => write!(formatter, "review template id must not be empty"),
            Self::TooFewLenses { template } => write!(
                formatter,
                "review template {template} needs at least two distinct lenses"
            ),
            Self::DuplicateLensId { template, lens } => write!(
                formatter,
                "review template {template} repeats lens id {lens}"
            ),
            Self::DuplicateLensFocus { template, focus } => write!(
                formatter,
                "review template {template} repeats lens focus {focus}"
            ),
            Self::TooManyRemediationIterations {
                template,
                requested,
            } => write!(
                formatter,
                "review template {template} requests {requested} remediation iterations; maximum is {MAX_REMEDIATION_ITERATIONS}"
            ),
            Self::RemediationLimitReached { target, limit } => write!(
                formatter,
                "review target {target} already reached its {limit}-iteration remediation limit"
            ),
            Self::TemplateMismatch { expected, actual } => write!(
                formatter,
                "review expansion belongs to template {expected}, not {actual}"
            ),
            Self::DuplicateNodeId(id) => write!(formatter, "work graph already contains node {id}"),
            Self::UnknownNode(id) => write!(formatter, "work graph does not contain node {id}"),
        }
    }
}

impl Error for ReviewGraphError {}

/// Apply plan-declared templates to all matching nodes.
///
/// An empty template list is a strict no-op, preserving pre-review-template
/// plans. Expansion is atomic: an invalid template or identifier collision
/// leaves the caller's graph unchanged.
pub fn instantiate_review_templates(
    graph: &mut TaskGraph,
    templates: &[ReviewTemplate],
) -> Result<Vec<ReviewExpansion>, ReviewGraphError> {
    if templates.is_empty() {
        return Ok(Vec::new());
    }
    for template in templates {
        validate_template(template)?;
    }

    let targets = graph.nodes.clone();
    let mut expanded_graph = graph.clone();
    let mut known_ids: BTreeSet<TaskId> = expanded_graph
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect();
    let mut expansions = Vec::new();

    for template in templates {
        for target in targets
            .iter()
            .filter(|node| template_matches(template, node))
        {
            expansions.push(expand_target(
                &mut expanded_graph,
                &mut known_ids,
                template,
                target,
            )?);
        }
    }

    *graph = expanded_graph;
    Ok(expansions)
}

fn validate_template(template: &ReviewTemplate) -> Result<(), ReviewGraphError> {
    if template.id.trim().is_empty() {
        return Err(ReviewGraphError::EmptyTemplateId);
    }
    if template.lenses.len() < 2 {
        return Err(ReviewGraphError::TooFewLenses {
            template: template.id.clone(),
        });
    }
    if template.remediation_iterations > MAX_REMEDIATION_ITERATIONS {
        return Err(ReviewGraphError::TooManyRemediationIterations {
            template: template.id.clone(),
            requested: template.remediation_iterations,
        });
    }

    let mut lens_ids = BTreeSet::new();
    let mut focuses = BTreeSet::new();
    for lens in &template.lenses {
        let id = lens.id.trim().to_lowercase();
        if id.is_empty() || !lens_ids.insert(id) {
            return Err(ReviewGraphError::DuplicateLensId {
                template: template.id.clone(),
                lens: lens.id.clone(),
            });
        }
        let focus = lens.focus.trim().to_lowercase();
        if focus.is_empty() || !focuses.insert(focus) {
            return Err(ReviewGraphError::DuplicateLensFocus {
                template: template.id.clone(),
                focus: lens.focus.clone(),
            });
        }
    }
    Ok(())
}

fn template_matches(template: &ReviewTemplate, node: &WorkNode) -> bool {
    node.kind == template.target_kind
        && template.required_output.as_ref().map_or(true, |required| {
            node.contract.outputs.iter().any(|output| output == required)
        })
}

fn expand_target(
    graph: &mut TaskGraph,
    known_ids: &mut BTreeSet<TaskId>,
    template: &ReviewTemplate,
    target: &WorkNode,
) -> Result<ReviewExpansion, ReviewGraphError> {
    let first_round = append_review_round(
        graph,
        known_ids,
        template,
        target,
        &target.id,
        0,
    )?;

    Ok(ReviewExpansion {
        template_id: template.id.clone(),
        target_id: target.id.clone(),
        rounds: vec![first_round],
        remediation_ids: Vec::new(),
    })
}

/// Append the standard Prince-remediation delta after a failing verdict.
///
/// Nothing is pre-created for a passing verdict. Each failure adds fresh,
/// forward-only identifiers, which expresses re-entry without a dependency
/// back-edge and stops after the template's bounded iteration limit.
pub fn route_failed_verdict(
    graph: &mut TaskGraph,
    template: &ReviewTemplate,
    expansion: &mut ReviewExpansion,
) -> Result<ReviewRoundExpansion, ReviewGraphError> {
    validate_template(template)?;
    if expansion.template_id != template.id {
        return Err(ReviewGraphError::TemplateMismatch {
            expected: expansion.template_id.clone(),
            actual: template.id.clone(),
        });
    }
    if expansion.remediation_ids.len() >= usize::from(template.remediation_iterations) {
        return Err(ReviewGraphError::RemediationLimitReached {
            target: expansion.target_id.clone(),
            limit: template.remediation_iterations,
        });
    }

    let target = graph
        .nodes
        .iter()
        .find(|node| node.id == expansion.target_id)
        .cloned()
        .ok_or_else(|| ReviewGraphError::UnknownNode(expansion.target_id.clone()))?;
    let verdict_id = expansion
        .rounds
        .last()
        .map(|round| round.verdict_id.clone())
        .ok_or_else(|| ReviewGraphError::UnknownNode(expansion.target_id.clone()))?;
    let round = u8::try_from(expansion.rounds.len()).map_err(|_| {
        ReviewGraphError::TooManyRemediationIterations {
            template: template.id.clone(),
            requested: u8::MAX,
        }
    })?;

    let mut expanded_graph = graph.clone();
    let mut known_ids: BTreeSet<_> = expanded_graph
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect();
    let remediation_id = append_remediation(
        &mut expanded_graph,
        &mut known_ids,
        template,
        &target,
        &verdict_id,
        round,
    )?;
    let next_round = append_review_round(
        &mut expanded_graph,
        &mut known_ids,
        template,
        &target,
        &remediation_id,
        round,
    )?;

    *graph = expanded_graph;
    expansion.remediation_ids.push(remediation_id);
    expansion.rounds.push(next_round.clone());
    Ok(next_round)
}

fn append_review_round(
    graph: &mut TaskGraph,
    known_ids: &mut BTreeSet<TaskId>,
    template: &ReviewTemplate,
    target: &WorkNode,
    review_source: &str,
    round: u8,
) -> Result<ReviewRoundExpansion, ReviewGraphError> {
    let mut lens_ids = Vec::new();
    for lens in &template.lenses {
        let lens_id = format!(
            "{}::review::{}::round-{round}::{}",
            target.id, template.id, lens.id
        );
        let evidence = format!("{lens_id}:evidence");
        let mut acceptance = template.rubric.clone();
        acceptance.extend(lens.acceptance.clone());
        let mut lens_node = WorkNode::new(
            &lens_id,
            NodeKind::Review,
            format!("{} review of {}", lens.id, target.title),
            NodeContract {
                inputs: vec![format!("{review_source}:outputs")],
                outputs: vec![evidence],
                acceptance,
            },
            lens.binding.clone(),
            NodeStatus::Pending,
        );
        lens_node.expansion = Some(expansion_metadata(
            MULTI_LENS_REVIEW_TEMPLATE,
            &target.id,
            round,
            Some((&lens.id, &lens.focus)),
        ));
        push_node(graph, known_ids, lens_node)?;
        graph.edges.push(
            WorkEdge::new(
                review_source,
                &lens_id,
                EdgeKind::Reviews,
                EdgeProvenance::Planner,
            )
            .with_rationale(format!(
                "{} supplies the input reviewed through the {} lens",
                review_source, lens.id
            )),
        );
        graph.edges.push(
            WorkEdge::new(
                review_source,
                &lens_id,
                EdgeKind::DependsOn,
                EdgeProvenance::Planner,
            )
            .with_rationale("schedule this bounded review round after its input exists"),
        );
        lens_ids.push(lens_id);
    }

    let verdict_id = format!(
        "{}::review::{}::round-{round}::verdict",
        target.id, template.id
    );
    let mut verdict_node = WorkNode::new(
        &verdict_id,
        NodeKind::Join,
        format!("Review verdict for {} (round {})", target.title, round + 1),
        NodeContract {
            inputs: lens_ids
                .iter()
                .map(|lens_id| format!("{lens_id}:evidence"))
                .collect(),
            outputs: vec![format!("{verdict_id}:outcome")],
            acceptance: template.rubric.clone(),
        },
        template.verdict_binding.clone(),
        NodeStatus::Pending,
    );
    verdict_node.expansion = Some(expansion_metadata(
        MULTI_LENS_REVIEW_TEMPLATE,
        &target.id,
        round,
        None,
    ));
    push_node(graph, known_ids, verdict_node)?;
    for lens_id in &lens_ids {
        graph.edges.push(
            WorkEdge::new(
                lens_id,
                &verdict_id,
                EdgeKind::DependsOn,
                EdgeProvenance::Planner,
            )
            .with_rationale("the verdict joins every distinct lens result"),
        );
    }
    Ok(ReviewRoundExpansion {
        lens_ids,
        verdict_id,
    })
}

fn append_remediation(
    graph: &mut TaskGraph,
    known_ids: &mut BTreeSet<TaskId>,
    template: &ReviewTemplate,
    target: &WorkNode,
    verdict_id: &str,
    next_round: u8,
) -> Result<TaskId, ReviewGraphError> {
    let remediation_id = format!(
        "{}::review::{}::after-round-{}::remediation",
        target.id,
        template.id,
        next_round - 1
    );
    let mut remediation_node = WorkNode::new(
        &remediation_id,
        NodeKind::Task,
        format!("Prince remediation for {}", target.title),
        NodeContract {
            inputs: vec![format!("{verdict_id}:outcome=fail")],
            outputs: vec![format!("{remediation_id}:delta")],
            acceptance: vec![
                "address every failing verdict item with a reviewable delta".to_string(),
            ],
        },
        template.remediation_binding.clone(),
        NodeStatus::Pending,
    );
    let mut metadata = expansion_metadata(
        JUDGE_PRINCE_REMEDIATION_TEMPLATE,
        &target.id,
        next_round - 1,
        None,
    );
    metadata
        .parameters
        .insert("activation".to_string(), "verdict_fail".to_string());
    metadata
        .parameters
        .insert("next_review_round".to_string(), next_round.to_string());
    remediation_node.expansion = Some(metadata);
    push_node(graph, known_ids, remediation_node)?;
    graph.edges.push(
        WorkEdge::new(
            verdict_id,
            &remediation_id,
            EdgeKind::DependsOn,
            EdgeProvenance::Planner,
        )
        .with_rationale("a failing verdict activates the Prince remediation delta"),
    );
    Ok(remediation_id)
}

fn expansion_metadata(
    template: &str,
    target: &str,
    round: u8,
    lens: Option<(&str, &str)>,
) -> CompositeExpansion {
    let mut parameters = BTreeMap::new();
    parameters.insert("target".to_string(), target.to_string());
    parameters.insert("round".to_string(), round.to_string());
    if let Some((id, focus)) = lens {
        parameters.insert("lens".to_string(), id.to_string());
        parameters.insert("focus".to_string(), focus.to_string());
    }
    CompositeExpansion {
        template: template.to_string(),
        parameters,
    }
}

fn push_node(
    graph: &mut TaskGraph,
    known_ids: &mut BTreeSet<TaskId>,
    node: WorkNode,
) -> Result<(), ReviewGraphError> {
    if !known_ids.insert(node.id.clone()) {
        return Err(ReviewGraphError::DuplicateNodeId(node.id));
    }
    graph.nodes.push(node);
    Ok(())
}

/// Insert an explicit checkpoint barrier. Disabled declarations are no-ops.
pub fn instantiate_checkpoint_wave(
    graph: &mut TaskGraph,
    wave: &CheckpointWave,
) -> Result<Option<TaskId>, ReviewGraphError> {
    if !wave.checkpoint {
        return Ok(None);
    }

    let existing: BTreeSet<_> = graph.nodes.iter().map(|node| node.id.clone()).collect();
    if existing.contains(&wave.id) {
        return Err(ReviewGraphError::DuplicateNodeId(wave.id.clone()));
    }
    for id in wave.prerequisites.iter().chain(&wave.downstream) {
        if !existing.contains(id) {
            return Err(ReviewGraphError::UnknownNode(id.clone()));
        }
    }

    let checkpoint_node = WorkNode::new(
        &wave.id,
        NodeKind::Checkpoint,
        &wave.title,
        NodeContract {
            inputs: wave
                .prerequisites
                .iter()
                .map(|id| format!("{id}:completed"))
                .collect(),
            outputs: vec![format!("{}:passed", wave.id)],
            acceptance: wave.acceptance.clone(),
        },
        wave.binding.clone(),
        NodeStatus::Ready,
    );
    graph.nodes.push(checkpoint_node);
    for prerequisite in &wave.prerequisites {
        graph.edges.push(
            WorkEdge::new(
                prerequisite,
                &wave.id,
                EdgeKind::DependsOn,
                EdgeProvenance::Planner,
            )
            .with_rationale("checkpoint waits for every upstream sibling"),
        );
    }
    for downstream in &wave.downstream {
        graph.edges.push(
            WorkEdge::new(
                &wave.id,
                downstream,
                EdgeKind::DependsOn,
                EdgeProvenance::Planner,
            )
            .with_rationale("downstream claims wait for the checkpoint gate"),
        );
    }
    Ok(Some(wave.id.clone()))
}

/// Pure claim-time projection for the graph's current statuses.
///
/// Ordinary readiness remains the default. An opt-in checkpoint adds ordinary
/// dependency edges, so downstream `Ready` nodes are withheld until the
/// checkpoint itself reaches `Completed`.
pub fn checkpoint_aware_claimable_nodes(graph: &TaskGraph) -> Vec<TaskId> {
    let statuses: BTreeMap<_, _> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.status))
        .collect();
    let mut claimable: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| node.status == NodeStatus::Ready)
        .filter(|node| {
            graph
                .edges
                .iter()
                .filter(|edge| edge.kind == EdgeKind::DependsOn && edge.target == node.id)
                .all(|edge| {
                    matches!(statuses.get(edge.source.as_str()), Some(NodeStatus::Completed))
                })
        })
        .map(|node| node.id.clone())
        .collect();
    claimable.sort();
    claimable
}
