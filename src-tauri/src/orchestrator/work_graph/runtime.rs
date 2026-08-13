//! Runtime graph reconstruction for issue #214, owned by WS-5.
//!
//! Execution evidence is replayed from the existing event log and run journal.
//! Structural changes are captured separately as in-memory, append-only deltas;
//! no claim or spawn path gains another persistent write.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::event::{Event, EventType};
use crate::domain::run_journal::{
    Confidence, LedgerEntry, RunJournalEntry, StepKind, StepStatus,
};

use super::review::{
    instantiate_checkpoint_wave, instantiate_review_templates, route_failed_verdict,
    CheckpointWave, ReviewExpansion, ReviewGraphError, ReviewRoundExpansion,
    ReviewTemplate,
};
use super::schema::{
    BindingRef, EdgeKind, EdgeProvenance, NodeContract, NodeKind, NodeStatus,
    TaskGraph, TaskId, WorkEdge, WorkGraph, WorkGraphOmission,
    WorkGraphOmissionReason, WorkNode,
};

pub const RUNTIME_OBSERVATION_PREFIX: &str = "runtime:";
const MAX_OMISSION_EXAMPLES: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphMutationType {
    Split,
    Merge,
    Reorder,
    CompositeExpanded,
    ReviewRoundAdded,
    ReviewVerdictRecorded,
    RemediationDetour,
    CheckpointInserted,
    Other,
}

/// An exact, invertible graph edit. Full before/after snapshots intentionally
/// preserve ordering and duplicate edges, which a fragment-only format cannot
/// reconstruct reliably because `WorkEdge` has no identity field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphMutationDelta {
    pub sequence: u64,
    pub observed_at: DateTime<Utc>,
    pub mutation_type: GraphMutationType,
    pub before: WorkGraph,
    pub after: WorkGraph,
    #[serde(default)]
    pub source_refs: Vec<String>,
}

static MUTATION_LOGS: OnceLock<Mutex<HashMap<String, Vec<GraphMutationDelta>>>> =
    OnceLock::new();

fn mutation_logs() -> &'static Mutex<HashMap<String, Vec<GraphMutationDelta>>> {
    MUTATION_LOGS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationLogError {
    pub session_id: String,
    pub previous_sequence: u64,
}

impl fmt::Display for MutationLogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "mutation for session {} does not continue delta {}",
            self.session_id, self.previous_sequence
        )
    }
}

impl Error for MutationLogError {}

#[derive(Debug)]
pub enum GraphMutationError<E> {
    Mutation(E),
    Log(MutationLogError),
}

impl<E: fmt::Display> fmt::Display for GraphMutationError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mutation(error) => write!(formatter, "graph mutation failed: {error}"),
            Self::Log(error) => write!(formatter, "graph mutation was not recorded: {error}"),
        }
    }
}

impl<E: Error + 'static> Error for GraphMutationError<E> {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationLogSnapshot {
    /// False means this process never observed a mutation boundary for the
    /// session. It must not be interpreted as a proven zero-mutation run.
    pub tracked: bool,
    pub deltas: Vec<GraphMutationDelta>,
}

fn ensure_mutation_tracking(session_id: &str) {
    mutation_logs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(session_id.to_string())
        .or_default();
}

/// Append one structural change to the session's in-memory mutation log.
/// Returns `None` for a true no-op.
pub fn record_graph_change(
    session_id: &str,
    mutation_type: GraphMutationType,
    before: &WorkGraph,
    after: &WorkGraph,
    source_refs: Vec<String>,
) -> Result<Option<GraphMutationDelta>, MutationLogError> {
    let mut logs = mutation_logs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let log = logs.entry(session_id.to_string()).or_default();
    if before == after {
        return Ok(None);
    }
    if let Some(previous) = log.last() {
        if previous.after != *before {
            return Err(MutationLogError {
                session_id: session_id.to_string(),
                previous_sequence: previous.sequence,
            });
        }
    }
    let delta = GraphMutationDelta {
        sequence: log
            .last()
            .map(|previous| previous.sequence.saturating_add(1))
            .unwrap_or(1),
        observed_at: Utc::now(),
        mutation_type,
        before: before.clone(),
        after: after.clone(),
        source_refs,
    };
    log.push(delta.clone());
    Ok(Some(delta))
}

/// Run a graph mutation and record its exact before/after state if it changed.
/// Review expansion/remediation callers can use this wrapper without changing
/// the frozen review implementation.
pub fn mutate_and_record<T, E, F>(
    session_id: &str,
    graph: &mut WorkGraph,
    mutation_type: GraphMutationType,
    source_refs: Vec<String>,
    mutate: F,
) -> Result<(T, Option<GraphMutationDelta>), GraphMutationError<E>>
where
    F: FnOnce(&mut WorkGraph) -> Result<T, E>,
{
    ensure_mutation_tracking(session_id);
    let before = graph.clone();
    let value = match mutate(graph) {
        Ok(value) => value,
        Err(error) => {
            *graph = before;
            return Err(GraphMutationError::Mutation(error));
        }
    };
    mark_new_edges_runtime(&before, graph);
    let delta = match record_graph_change(
        session_id,
        mutation_type,
        &before,
        graph,
        source_refs,
    ) {
        Ok(delta) => delta,
        Err(error) => {
            *graph = before;
            return Err(GraphMutationError::Log(error));
        }
    };
    Ok((value, delta))
}

fn mark_new_edges_runtime(before: &WorkGraph, after: &mut WorkGraph) {
    let mut consumed = vec![false; before.edges.len()];
    for edge in &mut after.edges {
        let prior = before.edges.iter().enumerate().position(|(index, candidate)| {
            !consumed[index]
                && candidate.source == edge.source
                && candidate.target == edge.target
                && candidate.kind == edge.kind
        });
        if let Some(index) = prior {
            consumed[index] = true;
        } else {
            edge.provenance = EdgeProvenance::Runtime;
        }
    }
}

/// Review-template boundary that guarantees the graph edit and its delta are
/// atomic from the caller's perspective while leaving the frozen producer
/// implementation unchanged.
pub fn instantiate_review_templates_and_record(
    session_id: &str,
    graph: &mut WorkGraph,
    templates: &[ReviewTemplate],
) -> Result<
    (Vec<ReviewExpansion>, Option<GraphMutationDelta>),
    GraphMutationError<ReviewGraphError>,
> {
    let refs = templates
        .iter()
        .map(|template| format!("review-template:{}", template.id))
        .collect();
    mutate_and_record(
        session_id,
        graph,
        GraphMutationType::ReviewRoundAdded,
        refs,
        |graph| instantiate_review_templates(graph, templates),
    )
}

pub fn route_failed_verdict_and_record(
    session_id: &str,
    graph: &mut WorkGraph,
    template: &ReviewTemplate,
    expansion: &mut ReviewExpansion,
) -> Result<
    (ReviewRoundExpansion, Option<GraphMutationDelta>),
    GraphMutationError<ReviewGraphError>,
> {
    let before_expansion = expansion.clone();
    let verdict = expansion
        .rounds
        .last()
        .map(|round| round.verdict_id.clone())
        .unwrap_or_else(|| expansion.target_id.clone());
    let result = mutate_and_record(
        session_id,
        graph,
        GraphMutationType::RemediationDetour,
        vec![format!("verdict:{verdict}")],
        |graph| {
            graph
                .nodes
                .iter_mut()
                .find(|node| node.id == verdict)
                .ok_or_else(|| ReviewGraphError::UnknownNode(verdict.clone()))?
                .status = NodeStatus::Failed;
            route_failed_verdict(graph, template, expansion)
        },
    );
    if result.is_err() {
        *expansion = before_expansion;
    }
    result
}

pub fn record_review_verdict_and_record(
    session_id: &str,
    graph: &mut WorkGraph,
    verdict_id: &str,
    verdict: ReviewVerdict,
) -> Result<Option<GraphMutationDelta>, GraphMutationError<ReviewGraphError>> {
    let (_, delta) = mutate_and_record(
        session_id,
        graph,
        GraphMutationType::ReviewVerdictRecorded,
        vec![format!("verdict:{verdict_id}")],
        |graph| {
            graph
                .nodes
                .iter_mut()
                .find(|node| node.id == verdict_id)
                .ok_or_else(|| ReviewGraphError::UnknownNode(verdict_id.to_string()))?
                .status = match verdict {
                ReviewVerdict::Passed => NodeStatus::Completed,
                ReviewVerdict::Failed => NodeStatus::Failed,
            };
            Ok(())
        },
    )?;
    Ok(delta)
}

pub fn instantiate_checkpoint_wave_and_record(
    session_id: &str,
    graph: &mut WorkGraph,
    wave: &CheckpointWave,
) -> Result<
    (Option<TaskId>, Option<GraphMutationDelta>),
    GraphMutationError<ReviewGraphError>,
> {
    mutate_and_record(
        session_id,
        graph,
        GraphMutationType::CheckpointInserted,
        vec![format!("checkpoint-wave:{}", wave.id)],
        |graph| instantiate_checkpoint_wave(graph, wave),
    )
}

/// Snapshot the append-only log. The archive path deliberately does not drain
/// it, so repeated terminal calls are safe and observe the same history.
pub fn mutation_log(session_id: &str) -> Vec<GraphMutationDelta> {
    mutation_log_snapshot(session_id).deltas
}

pub fn mutation_log_snapshot(session_id: &str) -> MutationLogSnapshot {
    mutation_logs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(session_id)
        .map(|deltas| MutationLogSnapshot {
            tracked: true,
            deltas: deltas.clone(),
        })
        .unwrap_or_else(|| MutationLogSnapshot {
            tracked: false,
            deltas: Vec::new(),
        })
}

/// Reconstruct every structural graph state from the delta chain. The final
/// graph is used as an integrity check, not as a substitute for a missing delta.
pub fn reconstruct_structural_history(
    final_runtime_graph: &WorkGraph,
    deltas: &[GraphMutationDelta],
) -> Result<Vec<WorkGraph>, String> {
    if deltas.is_empty() {
        return Ok(vec![structural_projection(final_runtime_graph)]);
    }

    if deltas[0].sequence != 1 {
        return Err("mutation delta sequence must begin at 1".to_string());
    }

    for pair in deltas.windows(2) {
        if pair[1].sequence != pair[0].sequence.saturating_add(1) {
            return Err("mutation delta sequence is not contiguous".to_string());
        }
        if pair[0].after != pair[1].before {
            return Err(format!(
                "mutation delta {} does not connect to delta {}",
                pair[0].sequence, pair[1].sequence
            ));
        }
    }

    let final_structural = structural_projection(final_runtime_graph);
    let last = deltas.last().expect("non-empty deltas have a last item");
    if !same_structural_state(&last.after, &final_structural) {
        return Err(format!(
            "final runtime graph does not match mutation delta {}",
            last.sequence
        ));
    }

    let mut history = Vec::with_capacity(deltas.len() + 1);
    history.push(deltas[0].before.clone());
    history.extend(deltas.iter().map(|delta| delta.after.clone()));
    Ok(history)
}

fn same_structural_state(left: &WorkGraph, right: &WorkGraph) -> bool {
    left.nodes == right.nodes && left.edges == right.edges
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeObservationKind {
    Claim,
    ClaimFailed,
    Spawn,
    Retry,
    Completion,
    Failure,
    Finalization,
    Artifact,
    JournalStep,
    LedgerEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOutcomeStatus {
    Running,
    Completed,
    Failed,
    Interrupted,
    Unknown,
    Skipped,
    Finalized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEffect {
    pub kind: String,
    pub reference: Option<String>,
    pub confirmed: bool,
    pub confidence: Confidence,
    pub source_ref: String,
}

/// Structured archive evidence for one task, agent, or journal step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeOutcome {
    pub subject_id: String,
    pub task_id: Option<TaskId>,
    #[serde(default)]
    pub agent_ids: Vec<String>,
    pub status: RuntimeOutcomeStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub attempt_count: usize,
    #[serde(default)]
    pub effects: Vec<RuntimeEffect>,
    #[serde(default)]
    pub source_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDerivation {
    pub runtime_graph: WorkGraph,
    pub outcomes: Vec<RuntimeOutcome>,
}

/// Replay immutable sources into an evidence graph. The function is pure: it
/// cannot append to events.jsonl, update SQLite, or persist graph state.
pub fn derive_runtime_graph(
    plan_graph: Option<&TaskGraph>,
    events: &[Event],
    journal: &[RunJournalEntry],
    ledger: &[LedgerEntry],
    deltas: &[GraphMutationDelta],
) -> RuntimeDerivation {
    let mut graph = plan_graph.cloned().unwrap_or_default();
    if plan_graph.is_none() {
        record_omission(
            &mut graph,
            WorkGraphOmissionReason::SourceUnreadable,
            "state/work-graph.json",
        );
    }

    for delta in deltas {
        graph = delta.after.clone();
    }

    let structural_ids: std::collections::BTreeSet<_> = graph
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect();
    let mut last_agent_observation = BTreeMap::<String, String>::new();
    let mut current_task = BTreeMap::<String, TaskId>::new();
    let mut outcomes = BTreeMap::<String, RuntimeOutcome>::new();
    for delta in deltas {
        update_mutation_outcomes(&mut outcomes, delta);
    }

    for event in events {
        let kind = match observation_kind(&event.event_type) {
            Some(kind) => kind,
            None => continue,
        };
        let observation_id = format!("{RUNTIME_OBSERVATION_PREFIX}event:{}", event.id);
        let agent_id = event
            .agent_id
            .clone()
            .or_else(|| event.payload.get("worker_id").and_then(|v| v.as_str()).map(str::to_string));
        let task_id = event
            .payload
            .get("task_id")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let status = observation_status(kind);

        graph.nodes.push(observation_node(
            &observation_id,
            kind,
            event.timestamp,
            status,
            agent_id.as_deref(),
        ));

        if let Some(agent_id) = agent_id.as_deref() {
            if let Some(previous) = last_agent_observation.get(agent_id) {
                graph.edges.push(
                    WorkEdge::new(
                        previous,
                        &observation_id,
                        EdgeKind::DependsOn,
                        EdgeProvenance::Runtime,
                    )
                    .with_rationale(format!(
                        "ordered runtime observations for agent {agent_id}"
                    )),
                );
            }
            last_agent_observation.insert(agent_id.to_string(), observation_id.clone());
        }

        match event.event_type {
            EventType::WorkerClaimed | EventType::WorkerClaimFailed => {
                let resolved = task_id
                    .as_ref()
                    .filter(|task_id| structural_ids.contains(*task_id));
                if let Some(task_id) = resolved {
                    graph.edges.push(
                        WorkEdge::new(
                            task_id,
                            &observation_id,
                            EdgeKind::Consumes,
                            EdgeProvenance::Runtime,
                        )
                        .with_rationale(format!(
                            "{} claimed planned task {task_id}",
                            agent_id.as_deref().unwrap_or("unknown worker")
                        )),
                    );
                    if matches!(event.event_type, EventType::WorkerClaimed) {
                        if let Some(agent_id) = agent_id.as_deref() {
                            current_task.insert(agent_id.to_string(), task_id.clone());
                        }
                    }
                } else {
                    record_omission(
                        &mut graph,
                        WorkGraphOmissionReason::ResolutionIncomplete,
                        &format!(
                            "claim:{}:{}",
                            agent_id.as_deref().unwrap_or("unknown"),
                            task_id.as_deref().unwrap_or("null-task-id")
                        ),
                    );
                    if let Some(agent_id) = agent_id.as_deref() {
                        current_task.remove(agent_id);
                    }
                }
            }
            EventType::WorkerReclaimed => {
                if let Some(task_id) = task_id
                    .as_ref()
                    .filter(|task_id| structural_ids.contains(*task_id))
                {
                    graph.edges.push(
                        WorkEdge::new(
                            task_id,
                            &observation_id,
                            EdgeKind::Informs,
                            EdgeProvenance::Runtime,
                        )
                        .with_rationale(format!(
                            "{} requeued planned task {task_id} for retry",
                            agent_id.as_deref().unwrap_or("unknown worker")
                        )),
                    );
                    if let Some(agent_id) = agent_id.as_deref() {
                        current_task.insert(agent_id.to_string(), task_id.clone());
                    }
                } else {
                    record_omission(
                        &mut graph,
                        WorkGraphOmissionReason::ResolutionIncomplete,
                        &format!(
                            "retry:{}:{}",
                            agent_id.as_deref().unwrap_or("unknown"),
                            task_id.as_deref().unwrap_or("null-task-id")
                        ),
                    );
                }
            }
            EventType::ArtifactUpdated => {
                if let Some(agent_id) = agent_id.as_deref() {
                    if let Some(task_id) = current_task.get(agent_id) {
                        graph.edges.push(
                            WorkEdge::new(
                                task_id,
                                &observation_id,
                                EdgeKind::Produces,
                                EdgeProvenance::Runtime,
                            )
                            .with_rationale("artifact observed after the task claim"),
                        );
                    }
                }
            }
            _ => {}
        }

        if let Some(agent_id) = agent_id {
            let resolved_event_task = task_id
                .filter(|task_id| structural_ids.contains(task_id))
                .or_else(|| current_task.get(&agent_id).cloned());
            update_event_outcome(
                &mut outcomes,
                &agent_id,
                resolved_event_task,
                kind,
                event,
            );
        }
    }

    let journal_step_ids: std::collections::BTreeSet<_> = journal
        .iter()
        .map(|entry| entry.step_id.as_str())
        .collect();
    for entry in journal {
        let node_id = format!("{RUNTIME_OBSERVATION_PREFIX}journal:{}", entry.step_id);
        graph.nodes.push(journal_node(&node_id, entry));
        let matched_effects: Vec<_> = ledger
            .iter()
            .filter(|effect| effect.step_id == entry.step_id)
            .map(|effect| {
                let effect_node_id = format!(
                    "{RUNTIME_OBSERVATION_PREFIX}ledger:{}:{}",
                    effect.step_id, effect.effect_kind
                );
                graph.nodes.push(ledger_node(&effect_node_id, effect));
                graph.edges.push(
                    WorkEdge::new(
                        &node_id,
                        effect_node_id,
                        EdgeKind::Produces,
                        EdgeProvenance::Runtime,
                    )
                    .with_rationale("journaled step produced this recorded side effect"),
                );
                runtime_effect(effect)
            })
            .collect();
        outcomes.insert(
            node_id.clone(),
            RuntimeOutcome {
                subject_id: node_id,
                task_id: None,
                agent_ids: if entry.kind == StepKind::GitCommit {
                    entry.detail.clone().into_iter().collect()
                } else {
                    Vec::new()
                },
                status: journal_outcome_status(entry.status),
                started_at: Some(entry.started_at),
                finished_at: entry.finished_at,
                attempt_count: 1,
                effects: matched_effects,
                source_refs: vec![format!("journal:step:{}", entry.step_id)],
            },
        );
    }
    for effect in ledger
        .iter()
        .filter(|effect| !journal_step_ids.contains(effect.step_id.as_str()))
    {
        let effect_node_id = format!(
            "{RUNTIME_OBSERVATION_PREFIX}ledger:{}:{}",
            effect.step_id, effect.effect_kind
        );
        graph.nodes.push(ledger_node(&effect_node_id, effect));
        record_omission(
            &mut graph,
            WorkGraphOmissionReason::ResolutionIncomplete,
            &format!("orphan-ledger-step:{}", effect.step_id),
        );
        outcomes.insert(
            effect_node_id.clone(),
            RuntimeOutcome {
                subject_id: effect_node_id,
                task_id: None,
                agent_ids: Vec::new(),
                status: if effect.confirmed {
                    RuntimeOutcomeStatus::Completed
                } else {
                    RuntimeOutcomeStatus::Unknown
                },
                started_at: Some(effect.recorded_at),
                finished_at: Some(effect.recorded_at),
                attempt_count: 1,
                effects: vec![runtime_effect(effect)],
                source_refs: vec![format!("ledger:step:{}", effect.step_id)],
            },
        );
    }

    RuntimeDerivation {
        runtime_graph: graph,
        outcomes: outcomes.into_values().collect(),
    }
}

/// Remove execution-observation nodes and their incident edges, leaving only
/// the graph structure used for plan-vs-actual divergence and mutation replay.
pub fn structural_projection(graph: &WorkGraph) -> WorkGraph {
    let nodes: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| !is_runtime_observation_node(node))
        .cloned()
        .collect();
    let ids: std::collections::BTreeSet<_> =
        nodes.iter().map(|node| node.id.as_str()).collect();
    let edges = graph
        .edges
        .iter()
        .filter(|edge| {
            ids.contains(edge.source.as_str()) && ids.contains(edge.target.as_str())
        })
        .cloned()
        .collect();
    WorkGraph {
        nodes,
        edges,
        omissions: graph.omissions.clone(),
    }
}

fn is_runtime_observation_node(node: &WorkNode) -> bool {
    node.id.starts_with(RUNTIME_OBSERVATION_PREFIX)
        && matches!(
            &node.binding,
            BindingRef::Role(role)
                if role == "runtime_observer" || role == "run_journal" || role == "run_ledger"
        )
}

pub fn record_omission(
    graph: &mut WorkGraph,
    reason: WorkGraphOmissionReason,
    example: &str,
) {
    let omission = if let Some(existing) = graph
        .omissions
        .iter_mut()
        .find(|omission| omission.reason == reason)
    {
        existing
    } else {
        graph
            .omissions
            .push(WorkGraphOmission::new(reason, 0, Vec::new()));
        graph
            .omissions
            .last_mut()
            .expect("an omission was just appended")
    };
    omission.count = omission.count.saturating_add(1);
    if omission.examples.len() < MAX_OMISSION_EXAMPLES
        && !omission.examples.iter().any(|seen| seen == example)
    {
        omission.examples.push(example.to_string());
    }
}

fn observation_kind(event_type: &EventType) -> Option<RuntimeObservationKind> {
    match event_type {
        EventType::WorkerClaimed => Some(RuntimeObservationKind::Claim),
        EventType::WorkerClaimFailed => Some(RuntimeObservationKind::ClaimFailed),
        EventType::AgentLaunched => Some(RuntimeObservationKind::Spawn),
        EventType::WorkerReclaimed => Some(RuntimeObservationKind::Retry),
        EventType::AgentCompleted => Some(RuntimeObservationKind::Completion),
        EventType::AgentFailed => Some(RuntimeObservationKind::Failure),
        EventType::WorkerFinalized => Some(RuntimeObservationKind::Finalization),
        EventType::ArtifactUpdated => Some(RuntimeObservationKind::Artifact),
        _ => None,
    }
}

fn observation_status(kind: RuntimeObservationKind) -> NodeStatus {
    match kind {
        RuntimeObservationKind::Claim
        | RuntimeObservationKind::Spawn
        | RuntimeObservationKind::JournalStep => NodeStatus::Running,
        RuntimeObservationKind::ClaimFailed | RuntimeObservationKind::Failure => {
            NodeStatus::Failed
        }
        RuntimeObservationKind::Retry => NodeStatus::Ready,
        RuntimeObservationKind::Completion
        | RuntimeObservationKind::Finalization
        | RuntimeObservationKind::Artifact
        | RuntimeObservationKind::LedgerEffect => NodeStatus::Completed,
    }
}

fn observation_node(
    id: &str,
    kind: RuntimeObservationKind,
    timestamp: DateTime<Utc>,
    status: NodeStatus,
    agent_id: Option<&str>,
) -> WorkNode {
    WorkNode::new(
        id,
        NodeKind::Context,
        format!("Runtime {kind:?} at {timestamp}"),
        NodeContract {
            inputs: agent_id.into_iter().map(str::to_string).collect(),
            outputs: vec![format!("{id}:observed")],
            acceptance: vec!["observation exists in the immutable event log".to_string()],
        },
        BindingRef::Role("runtime_observer".to_string()),
        status,
    )
}

fn journal_node(id: &str, entry: &RunJournalEntry) -> WorkNode {
    WorkNode::new(
        id,
        NodeKind::Context,
        format!("Journaled {:?} step", entry.kind),
        NodeContract {
            inputs: entry.detail.clone().into_iter().collect(),
            outputs: vec![format!("{id}:status={}", entry.status.as_tag())],
            acceptance: vec!["step exists in the run journal".to_string()],
        },
        BindingRef::Role("run_journal".to_string()),
        match entry.status {
            StepStatus::Started => NodeStatus::Running,
            StepStatus::Completed | StepStatus::Skipped => NodeStatus::Completed,
            StepStatus::Failed => NodeStatus::Failed,
            StepStatus::Interrupted | StepStatus::Unknown => NodeStatus::Blocked,
        },
    )
}

fn ledger_node(id: &str, entry: &LedgerEntry) -> WorkNode {
    WorkNode::new(
        id,
        NodeKind::Context,
        format!("Ledger effect {}", entry.effect_kind),
        NodeContract {
            inputs: vec![entry.step_id.clone()],
            outputs: entry.effect_ref.clone().into_iter().collect(),
            acceptance: vec![format!("confirmed={}", entry.confirmed)],
        },
        BindingRef::Role("run_ledger".to_string()),
        if entry.confirmed {
            NodeStatus::Completed
        } else {
            NodeStatus::Blocked
        },
    )
}

fn runtime_effect(entry: &LedgerEntry) -> RuntimeEffect {
    RuntimeEffect {
        kind: entry.effect_kind.clone(),
        reference: entry.effect_ref.clone(),
        confirmed: entry.confirmed,
        confidence: entry.confidence,
        source_ref: format!("ledger:step:{}", entry.step_id),
    }
}

fn journal_outcome_status(status: StepStatus) -> RuntimeOutcomeStatus {
    match status {
        StepStatus::Started => RuntimeOutcomeStatus::Running,
        StepStatus::Completed => RuntimeOutcomeStatus::Completed,
        StepStatus::Failed => RuntimeOutcomeStatus::Failed,
        StepStatus::Interrupted => RuntimeOutcomeStatus::Interrupted,
        StepStatus::Unknown => RuntimeOutcomeStatus::Unknown,
        StepStatus::Skipped => RuntimeOutcomeStatus::Skipped,
    }
}

fn update_mutation_outcomes(
    outcomes: &mut BTreeMap<String, RuntimeOutcome>,
    delta: &GraphMutationDelta,
) {
    for node in &delta.after.nodes {
        let prior_status = delta
            .before
            .nodes
            .iter()
            .find(|prior| prior.id == node.id)
            .map(|prior| prior.status);
        if prior_status.is_none() || prior_status == Some(node.status) {
            continue;
        }
        let task_id = node
            .expansion
            .as_ref()
            .and_then(|expansion| expansion.parameters.get("target"))
            .cloned()
            .or_else(|| (node.kind == NodeKind::Task).then(|| node.id.clone()));
        let status = match node.status {
            NodeStatus::Running => RuntimeOutcomeStatus::Running,
            NodeStatus::Completed => RuntimeOutcomeStatus::Completed,
            NodeStatus::Failed => RuntimeOutcomeStatus::Failed,
            NodeStatus::Cancelled => RuntimeOutcomeStatus::Interrupted,
            NodeStatus::Pending | NodeStatus::Ready | NodeStatus::Blocked => {
                RuntimeOutcomeStatus::Unknown
            }
        };
        let outcome = outcomes
            .entry(node.id.clone())
            .or_insert_with(|| RuntimeOutcome {
                subject_id: node.id.clone(),
                task_id,
                agent_ids: Vec::new(),
                status,
                started_at: None,
                finished_at: None,
                attempt_count: 1,
                effects: Vec::new(),
                source_refs: Vec::new(),
            });
        outcome.status = status;
        if status == RuntimeOutcomeStatus::Running {
            outcome.started_at.get_or_insert(delta.observed_at);
        } else {
            outcome.finished_at = Some(delta.observed_at);
        }
        outcome
            .source_refs
            .push(format!("mutation:delta:{}", delta.sequence));
        outcome.source_refs.extend(delta.source_refs.iter().cloned());
    }
}

fn update_event_outcome(
    outcomes: &mut BTreeMap<String, RuntimeOutcome>,
    agent_id: &str,
    task_id: Option<TaskId>,
    kind: RuntimeObservationKind,
    event: &Event,
) {
    let subject_id = task_id
        .clone()
        .unwrap_or_else(|| format!("agent:{agent_id}"));
    let outcome = outcomes
        .entry(subject_id.clone())
        .or_insert_with(|| RuntimeOutcome {
            subject_id,
            task_id: task_id.clone(),
            agent_ids: vec![agent_id.to_string()],
            status: RuntimeOutcomeStatus::Running,
            started_at: Some(event.timestamp),
            finished_at: None,
            attempt_count: 0,
            effects: Vec::new(),
            source_refs: Vec::new(),
        });
    if !outcome.agent_ids.iter().any(|known| known == agent_id) {
        outcome.agent_ids.push(agent_id.to_string());
    }
    if outcome.task_id.is_none() {
        outcome.task_id = task_id;
    }
    outcome.source_refs.push(format!("event:{}", event.id));
    match kind {
        RuntimeObservationKind::Claim => {
            outcome.attempt_count = outcome.attempt_count.saturating_add(1);
            outcome.status = RuntimeOutcomeStatus::Running;
        }
        RuntimeObservationKind::ClaimFailed => {
            outcome.attempt_count = outcome.attempt_count.saturating_add(1);
            outcome.status = RuntimeOutcomeStatus::Failed;
            outcome.finished_at = Some(event.timestamp);
        }
        RuntimeObservationKind::Spawn => {
            outcome.started_at.get_or_insert(event.timestamp);
        }
        RuntimeObservationKind::Retry => {
            outcome.status = RuntimeOutcomeStatus::Running;
        }
        RuntimeObservationKind::Completion => {
            outcome.status = RuntimeOutcomeStatus::Completed;
            outcome.finished_at = Some(event.timestamp);
        }
        RuntimeObservationKind::Failure => {
            outcome.status = RuntimeOutcomeStatus::Failed;
            outcome.finished_at = Some(event.timestamp);
        }
        RuntimeObservationKind::Finalization => {
            outcome.status = RuntimeOutcomeStatus::Finalized;
            outcome.finished_at = Some(event.timestamp);
        }
        RuntimeObservationKind::Artifact => {
            outcome.effects.push(RuntimeEffect {
                kind: "artifact".to_string(),
                reference: event
                    .payload
                    .get("path")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                confirmed: true,
                confidence: Confidence::High,
                source_ref: format!("event:{}", event.id),
            });
        }
        RuntimeObservationKind::JournalStep
        | RuntimeObservationKind::LedgerEffect => {}
    }
}
