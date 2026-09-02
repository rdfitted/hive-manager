use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::coordination::queue_manager::CompletionProvenance;
use crate::coordination::{HierarchyNode, StateManager};
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::orchestrator::work_graph::archive::{list_archives, read_archive, WorkGraphArchive};
use crate::orchestrator::work_graph::completion_ledger::{
    read_node_completion_facts, NodeCompletionProvenance,
};
use crate::orchestrator::work_graph::divergence::{compute_divergence, DivergenceSummary};
use crate::orchestrator::work_graph::runtime::{
    mutation_log_snapshot, CompletionEvidenceClass, RuntimeOutcome,
};
use crate::orchestrator::work_graph::{
    topological_sort, BindingRef, CompositeExpansion, EdgeKind, EdgeProvenance, NodeContract,
    NodeKind, NodeStatus, TaskGraph, TaskId, WorkGraphOmission, WorkGraphOmissionReason,
};

use super::validate_session_id;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkGraphView {
    Plan,
    #[default]
    Runtime,
    Divergence,
}

#[derive(Debug, Default, Deserialize)]
pub struct WorkGraphQuery {
    #[serde(default)]
    view: WorkGraphView,
    #[serde(default)]
    source: WorkGraphSourceSelector,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkGraphSourceSelector {
    #[default]
    Auto,
    Live,
    Archive,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkGraphSource {
    Live,
    Archive,
}

#[derive(Debug, Serialize)]
pub struct ContractSummary {
    pub input_count: usize,
    pub output_count: usize,
    pub acceptance_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkGraphNodeProgress {
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub attempts: usize,
    pub agent_id: Option<String>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct WorkGraphNodeResponse {
    pub id: TaskId,
    pub title: String,
    pub kind: NodeKind,
    pub status: NodeStatus,
    pub lane: BindingRef,
    pub contract: NodeContract,
    pub contract_summary: ContractSummary,
    pub expansion: Option<CompositeExpansion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<WorkGraphNodeProgress>,
}

#[derive(Debug, Serialize)]
pub struct WorkGraphEdgeResponse {
    pub source: TaskId,
    pub target: TaskId,
    pub kind: EdgeKind,
    pub provenance: EdgeProvenance,
}

#[derive(Debug, Serialize)]
pub struct EdgeProvenanceResponse {
    pub source: TaskId,
    pub target: TaskId,
    pub kind: EdgeKind,
    pub provenance: EdgeProvenance,
}

#[derive(Debug, Serialize)]
pub struct WorkGraphResponse {
    pub view: WorkGraphView,
    pub source: WorkGraphSource,
    pub nodes: Vec<WorkGraphNodeResponse>,
    pub edges: Vec<WorkGraphEdgeResponse>,
    pub waves: Vec<Vec<TaskId>>,
    pub status_by_node: BTreeMap<TaskId, NodeStatus>,
    pub completion_provenance: BTreeMap<TaskId, CompletionProvenance>,
    pub completion_source_refs: BTreeMap<TaskId, Vec<String>>,
    pub lane_assignment: BTreeMap<TaskId, BindingRef>,
    pub agents_by_lane: BTreeMap<String, Vec<String>>,
    pub critical_path: Vec<TaskId>,
    pub provenance_by_edge: Vec<EdgeProvenanceResponse>,
    pub divergence: Option<DivergenceSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub omissions: Vec<WorkGraphOmission>,
}

/// GET /api/sessions/{id}/work-graph?view=plan|runtime|divergence&source=auto|live|archive
///
/// This handler only projects existing state. It never creates, archives, or mutates a graph.
pub async fn get_work_graph(
    State(state): State<Arc<AppState>>,
    AxumPath(session_id): AxumPath<String>,
    Query(query): Query<WorkGraphQuery>,
) -> Result<Json<WorkGraphResponse>, ApiError> {
    validate_session_id(&session_id)?;
    let session_dir = state.storage.session_dir(&session_id);
    if !session_dir.is_dir() {
        return Err(ApiError::not_found(format!(
            "Session not found: {session_id}"
        )));
    }

    let (
        source,
        graph,
        divergence,
        progress_by_node,
        completion_provenance,
        completion_source_refs,
        omissions,
    ) = match query.source {
        WorkGraphSourceSelector::Live => {
            let plan = read_live_graph(&session_dir)?.ok_or_else(|| {
                ApiError::not_found(format!("Live work graph not found: {session_id}"))
            })?;
            graph_from_live_state(&state, &session_id, query.view, plan)?
        }
        WorkGraphSourceSelector::Archive => {
            let archive = latest_archive(&session_dir, &session_id)?.ok_or_else(|| {
                ApiError::not_found(format!("Work graph archive not found: {session_id}"))
            })?;
            graph_from_archive(&archive, query.view)
        }
        WorkGraphSourceSelector::Auto => {
            let is_terminal = state
                .session_controller
                .read()
                .get_session(&session_id)
                .is_some_and(|session| session.state.is_terminal());
            if is_terminal {
                if let Some(archive) = latest_archive(&session_dir, &session_id)? {
                    graph_from_archive(&archive, query.view)
                } else {
                    let plan = read_live_graph(&session_dir)?.ok_or_else(|| {
                        ApiError::not_found(format!("Work graph not found: {session_id}"))
                    })?;
                    let mut live = graph_from_live_state(&state, &session_id, query.view, plan)?;
                    live.6.push(WorkGraphOmission::new(
                        WorkGraphOmissionReason::SourceUnreadable,
                        1,
                        vec!["archive:missing".to_string()],
                    ));
                    live
                }
            } else if let Some(plan) = read_live_graph(&session_dir)? {
                graph_from_live_state(&state, &session_id, query.view, plan)?
            } else {
                let archive = latest_archive(&session_dir, &session_id)?.ok_or_else(|| {
                    ApiError::not_found(format!("Work graph not found: {session_id}"))
                })?;
                graph_from_archive(&archive, query.view)
            }
        }
    };

    let hierarchy = StateManager::new(session_dir)
        .read_hierarchy()
        .map_err(|error| ApiError::internal(format!("Failed to read agent hierarchy: {error}")))?;

    let response = project_graph(
        query.view,
        source,
        graph,
        divergence,
        &progress_by_node,
        completion_provenance,
        completion_source_refs,
        &hierarchy,
        omissions,
    )?;
    Ok(Json(response))
}

fn read_live_graph(session_dir: &Path) -> Result<Option<TaskGraph>, ApiError> {
    StateManager::new(session_dir.to_path_buf())
        .read_work_graph()
        .map_err(|error| ApiError::internal(format!("Failed to read live work graph: {error}")))
}

fn latest_archive(
    session_dir: &Path,
    session_id: &str,
) -> Result<Option<WorkGraphArchive>, ApiError> {
    let paths = list_archives(session_dir).map_err(|error| {
        ApiError::internal(format!("Failed to list work-graph archives: {error}"))
    })?;
    let mut latest: Option<WorkGraphArchive> = None;

    for path in paths {
        let archive = read_archive(&path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to read work-graph archive {}: {error}",
                path.display()
            ))
        })?;
        if archive.session_id != session_id {
            continue;
        }
        if latest
            .as_ref()
            .is_none_or(|current| archive.archived_at > current.archived_at)
        {
            latest = Some(archive);
        }
    }

    Ok(latest)
}

fn graph_from_archive(
    archive: &WorkGraphArchive,
    view: WorkGraphView,
) -> (
    WorkGraphSource,
    TaskGraph,
    Option<DivergenceSummary>,
    BTreeMap<TaskId, WorkGraphNodeProgress>,
    BTreeMap<TaskId, CompletionProvenance>,
    BTreeMap<TaskId, Vec<String>>,
    Vec<WorkGraphOmission>,
) {
    match view {
        WorkGraphView::Plan => (
            WorkGraphSource::Archive,
            archive
                .plan_graph
                .clone()
                .unwrap_or_else(|| archive.runtime_graph.clone()),
            None,
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            Vec::new(),
        ),
        WorkGraphView::Runtime => (
            WorkGraphSource::Archive,
            archive.runtime_graph.clone(),
            None,
            archive_progress_by_node(archive),
            archive_completion_provenance(archive),
            archive_completion_source_refs(archive),
            Vec::new(),
        ),
        WorkGraphView::Divergence => (
            WorkGraphSource::Archive,
            archive.runtime_graph.clone(),
            Some(archive.divergence.clone()),
            archive_progress_by_node(archive),
            archive_completion_provenance(archive),
            archive_completion_source_refs(archive),
            Vec::new(),
        ),
    }
}

fn archive_completion_source_refs(archive: &WorkGraphArchive) -> BTreeMap<TaskId, Vec<String>> {
    let structural_node_ids: BTreeSet<&str> = archive
        .runtime_graph
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect();
    let mut refs_by_node = BTreeMap::<TaskId, Vec<String>>::new();
    for outcome in archive
        .outcomes
        .iter()
        .filter(|outcome| outcome.completion_evidence.is_some())
    {
        let task_id = if structural_node_ids.contains(outcome.subject_id.as_str()) {
            Some(outcome.subject_id.clone())
        } else {
            outcome
                .task_id
                .clone()
                .filter(|task_id| structural_node_ids.contains(task_id.as_str()))
        };
        let Some(task_id) = task_id else {
            continue;
        };
        refs_by_node
            .entry(task_id)
            .or_default()
            .extend(outcome.source_refs.iter().cloned());
    }
    for source_refs in refs_by_node.values_mut() {
        source_refs.sort();
        source_refs.dedup();
    }
    refs_by_node
}

fn archive_completion_provenance(
    archive: &WorkGraphArchive,
) -> BTreeMap<TaskId, CompletionProvenance> {
    let structural_node_ids: BTreeSet<&str> = archive
        .runtime_graph
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect();
    let mut provenance = BTreeMap::new();
    for outcome in &archive.outcomes {
        let Some(class) = outcome.completion_evidence else {
            continue;
        };
        let task_id = if structural_node_ids.contains(outcome.subject_id.as_str()) {
            Some(outcome.subject_id.clone())
        } else {
            outcome
                .task_id
                .clone()
                .filter(|task_id| structural_node_ids.contains(task_id.as_str()))
        };
        let Some(task_id) = task_id else {
            continue;
        };
        let candidate = match class {
            CompletionEvidenceClass::Observed => CompletionProvenance::Observed,
            CompletionEvidenceClass::Inferred => CompletionProvenance::Inferred,
        };
        let existing = provenance.get(&task_id).copied();
        if existing != Some(CompletionProvenance::Observed) {
            provenance.insert(task_id, candidate);
        }
    }
    provenance
}

fn archive_progress_by_node(archive: &WorkGraphArchive) -> BTreeMap<TaskId, WorkGraphNodeProgress> {
    let structural_node_ids: BTreeSet<&str> = archive
        .runtime_graph
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect();
    let mut progress = BTreeMap::new();

    for outcome in archive
        .outcomes
        .iter()
        .filter(|outcome| structural_node_ids.contains(outcome.subject_id.as_str()))
    {
        progress.insert(
            outcome.subject_id.clone(),
            node_progress_from_outcome(outcome),
        );
    }

    for outcome in archive
        .outcomes
        .iter()
        .filter(|outcome| !structural_node_ids.contains(outcome.subject_id.as_str()))
    {
        let Some(task_id) = outcome
            .task_id
            .as_ref()
            .filter(|task_id| structural_node_ids.contains(task_id.as_str()))
        else {
            continue;
        };
        progress
            .entry(task_id.clone())
            .or_insert_with(|| node_progress_from_outcome(outcome));
    }
    progress
}

fn node_progress_from_outcome(outcome: &RuntimeOutcome) -> WorkGraphNodeProgress {
    WorkGraphNodeProgress {
        started_at: outcome.started_at,
        finished_at: outcome.finished_at,
        attempts: outcome.attempt_count,
        agent_id: outcome.agent_ids.last().cloned(),
        last_heartbeat_at: None,
    }
}

fn graph_from_live_state(
    state: &AppState,
    session_id: &str,
    view: WorkGraphView,
    plan: TaskGraph,
) -> Result<
    (
        WorkGraphSource,
        TaskGraph,
        Option<DivergenceSummary>,
        BTreeMap<TaskId, WorkGraphNodeProgress>,
        BTreeMap<TaskId, CompletionProvenance>,
        BTreeMap<TaskId, Vec<String>>,
        Vec<WorkGraphOmission>,
    ),
    ApiError,
> {
    if matches!(view, WorkGraphView::Plan) {
        return Ok((
            WorkGraphSource::Live,
            plan,
            None,
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            Vec::new(),
        ));
    }

    let (mut runtime, mut completion_provenance) = state
        .queue_manager
        .project_queue_statuses_for_view(session_id, &plan)
        .map_err(|error| {
            ApiError::internal(format!("Failed to project live work-graph status: {error}"))
        })?;
    let (completion_facts, mut ledger_source_omissions) =
        match read_node_completion_facts(&state.storage.session_dir(session_id)) {
            Ok(facts) => (facts, Vec::new()),
            Err(error) => {
                let mut omission = WorkGraphOmission::new(
                    WorkGraphOmissionReason::SourceUnreadable,
                    1,
                    vec!["state/work-graph-completions.jsonl".to_string()],
                );
                omission.detail = format!(
                    "declared node completions could not be read and were omitted: {error}"
                );
                (Vec::new(), vec![omission])
            }
        };
    let mut completion_source_refs = BTreeMap::<TaskId, Vec<String>>::new();
    let mut declared_progress = BTreeMap::<TaskId, WorkGraphNodeProgress>::new();
    let mut declared_omissions = Vec::new();
    for fact in completion_facts {
        if let Some(node) = runtime
            .nodes
            .iter_mut()
            .find(|node| node.id == fact.task_id)
        {
            node.status = NodeStatus::Completed;
            completion_provenance.insert(fact.task_id.clone(), CompletionProvenance::Declared);
            completion_source_refs
                .entry(fact.task_id.clone())
                .or_default()
                .push(fact.source_ref());
            declared_progress.insert(
                fact.task_id,
                WorkGraphNodeProgress {
                    started_at: None,
                    finished_at: Some(fact.completed_at),
                    attempts: 1,
                    agent_id: Some(fact.agent_id),
                    last_heartbeat_at: (fact.provenance == NodeCompletionProvenance::Heartbeat)
                        .then_some(fact.completed_at),
                },
            );
        } else {
            declared_omissions.push(format!("{}:task:{}", fact.source_ref(), fact.task_id));
        }
    }
    let (divergence, mut omissions) = if matches!(view, WorkGraphView::Divergence) {
        let mutation_snapshot = mutation_log_snapshot(session_id);
        let omissions = (!mutation_snapshot.tracked)
            .then(|| {
                let mut omission = WorkGraphOmission::new(
                    WorkGraphOmissionReason::ResolutionIncomplete,
                    1,
                    vec!["mutation-log:not-observed-in-this-process".to_string()],
                );
                omission.detail = "the process did not observe a mutation boundary for this session; zero deltas cannot prove that no earlier structural mutations occurred".to_string();
                omission
            })
            .into_iter()
            .collect();
        (
            Some(compute_divergence(
                Some(&plan),
                &runtime,
                &mutation_snapshot.deltas,
            )),
            omissions,
        )
    } else {
        (None, Vec::new())
    };
    omissions.append(&mut ledger_source_omissions);
    if !declared_omissions.is_empty() {
        let mut omission = WorkGraphOmission::new(
            WorkGraphOmissionReason::CompletionUnresolved,
            declared_omissions.len(),
            declared_omissions,
        );
        omission.detail =
            "declared completion referenced a node absent from the current live work graph"
                .to_string();
        omissions.push(omission);
    }
    let mut progress = live_progress_by_node(state, session_id)?;
    progress.extend(declared_progress);
    Ok((
        WorkGraphSource::Live,
        runtime,
        divergence,
        progress,
        completion_provenance,
        completion_source_refs,
        omissions,
    ))
}

fn live_progress_by_node(
    state: &AppState,
    session_id: &str,
) -> Result<BTreeMap<TaskId, WorkGraphNodeProgress>, ApiError> {
    let rows = state
        .queue_manager
        .queue_snapshot(session_id)
        .map_err(|error| {
            ApiError::internal(format!("Failed to read live work-graph progress: {error}"))
        })?
        .rows;
    let mut latest: BTreeMap<TaskId, crate::storage::queue::QueueRow> = BTreeMap::new();
    for row in rows {
        let Some(task_id) = row.task_id.clone() else {
            continue;
        };
        let replace = latest.get(&task_id).is_none_or(|existing| {
            (row.updated_at, row.created_at, row.id.as_str())
                > (
                    existing.updated_at,
                    existing.created_at,
                    existing.id.as_str(),
                )
        });
        if replace {
            latest.insert(task_id, row);
        }
    }

    latest
        .into_iter()
        .map(|(task_id, row)| {
            let attempts = usize::try_from(row.attempts).map_err(|_| {
                ApiError::internal(format!(
                    "Queue row {} has a negative attempt count: {}",
                    row.id, row.attempts
                ))
            })?;
            let last_heartbeat_at = row
                .heartbeat_at
                .map(|millis| {
                    DateTime::<Utc>::from_timestamp_millis(millis).ok_or_else(|| {
                        ApiError::internal(format!(
                            "Queue row {} has an invalid heartbeat timestamp: {millis}",
                            row.id
                        ))
                    })
                })
                .transpose()?;
            let agent_id = (!row.worker_id.starts_with("pending:")).then_some(row.worker_id);
            Ok((
                task_id,
                WorkGraphNodeProgress {
                    started_at: None,
                    finished_at: None,
                    attempts,
                    agent_id,
                    last_heartbeat_at,
                },
            ))
        })
        .collect()
}

fn project_graph(
    view: WorkGraphView,
    source: WorkGraphSource,
    graph: TaskGraph,
    divergence: Option<DivergenceSummary>,
    progress_by_node: &BTreeMap<TaskId, WorkGraphNodeProgress>,
    completion_provenance: BTreeMap<TaskId, CompletionProvenance>,
    completion_source_refs: BTreeMap<TaskId, Vec<String>>,
    hierarchy: &[HierarchyNode],
    supplemental_omissions: Vec<WorkGraphOmission>,
) -> Result<WorkGraphResponse, ApiError> {
    let order = topological_sort(&graph).map_err(|error| {
        ApiError::internal(format!("Persisted work graph is not schedulable: {error}"))
    })?;
    let waves = topological_waves(&graph, &order);
    let critical_path = critical_path(&graph, &order);
    let mut omissions = graph.omissions.clone();
    omissions.extend(supplemental_omissions);

    let nodes = graph
        .nodes
        .iter()
        .map(|node| WorkGraphNodeResponse {
            id: node.id.clone(),
            title: node.title.clone(),
            kind: node.kind,
            status: node.status,
            lane: node.binding.clone(),
            contract: node.contract.clone(),
            contract_summary: ContractSummary {
                input_count: node.contract.inputs.len(),
                output_count: node.contract.outputs.len(),
                acceptance_count: node.contract.acceptance.len(),
            },
            expansion: node.expansion.clone(),
            progress: progress_by_node.get(&node.id).cloned(),
        })
        .collect();
    let edges = graph
        .edges
        .iter()
        .map(|edge| WorkGraphEdgeResponse {
            source: edge.source.clone(),
            target: edge.target.clone(),
            kind: edge.kind,
            provenance: edge.provenance,
        })
        .collect();
    let status_by_node = graph
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.status))
        .collect();
    let principal_by_agent: BTreeMap<&str, &str> = hierarchy
        .iter()
        .filter_map(|agent| {
            agent
                .principal
                .as_deref()
                .map(|principal| (agent.id.as_str(), principal))
        })
        .collect();
    let mut agents_by_lane = BTreeMap::<String, Vec<String>>::new();
    for agent in hierarchy {
        if let Some(principal) = agent.principal.as_deref() {
            agents_by_lane
                .entry(principal.to_string())
                .or_default()
                .push(agent.id.clone());
        }
    }
    for agents in agents_by_lane.values_mut() {
        agents.sort();
        agents.dedup();
    }
    let lane_assignment = graph
        .nodes
        .iter()
        .map(|node| {
            let principal = binding_value(&node.binding);
            let progress_agent = progress_by_node
                .get(&node.id)
                .and_then(|progress| progress.agent_id.as_deref())
                .filter(|agent_id| principal_by_agent.get(agent_id).copied() == Some(principal));
            let observed_agent = progress_agent.or_else(|| {
                agents_by_lane
                    .get(principal)
                    .and_then(|agents| agents.first())
                    .map(String::as_str)
            });
            let assignment = observed_agent
                .map(|agent_id| BindingRef::Role(agent_id.to_string()))
                .unwrap_or_else(|| node.binding.clone());
            (node.id.clone(), assignment)
        })
        .collect();
    let provenance_by_edge = graph
        .edges
        .iter()
        .map(|edge| EdgeProvenanceResponse {
            source: edge.source.clone(),
            target: edge.target.clone(),
            kind: edge.kind,
            provenance: edge.provenance,
        })
        .collect();

    Ok(WorkGraphResponse {
        view,
        source,
        nodes,
        edges,
        waves,
        status_by_node,
        completion_provenance,
        completion_source_refs,
        lane_assignment,
        agents_by_lane,
        critical_path,
        provenance_by_edge,
        divergence,
        omissions,
    })
}

fn binding_value(binding: &BindingRef) -> &str {
    match binding {
        BindingRef::Role(value) | BindingRef::Zone(value) => value,
    }
}

fn topological_waves(graph: &TaskGraph, order: &[TaskId]) -> Vec<Vec<TaskId>> {
    let known: BTreeSet<&str> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in graph
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::DependsOn)
    {
        if known.contains(edge.source.as_str()) && known.contains(edge.target.as_str()) {
            dependents
                .entry(edge.source.as_str())
                .or_default()
                .push(edge.target.as_str());
        }
    }
    for targets in dependents.values_mut() {
        targets.sort_unstable();
        targets.dedup();
    }

    let mut levels: BTreeMap<&str, usize> = known.iter().copied().map(|id| (id, 0)).collect();
    for id in order {
        let level = levels.get(id.as_str()).copied().unwrap_or(0);
        for target in dependents.get(id.as_str()).into_iter().flatten() {
            let target_level = levels.entry(target).or_insert(0);
            *target_level = (*target_level).max(level.saturating_add(1));
        }
    }

    let wave_count = levels.values().copied().max().map_or(0, |level| level + 1);
    let mut waves = vec![Vec::new(); wave_count];
    for (id, level) in levels {
        waves[level].push(id.to_string());
    }
    for wave in &mut waves {
        wave.sort();
    }
    waves
}

/// Deterministic longest path over dependency edges. Non-dependency edges never affect it.
fn critical_path(graph: &TaskGraph, order: &[TaskId]) -> Vec<TaskId> {
    let known: BTreeSet<&str> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
    let mut predecessors: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in graph
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::DependsOn)
    {
        if known.contains(edge.source.as_str()) && known.contains(edge.target.as_str()) {
            predecessors
                .entry(edge.target.as_str())
                .or_default()
                .push(edge.source.as_str());
        }
    }
    for sources in predecessors.values_mut() {
        sources.sort_unstable();
        sources.dedup();
    }

    let mut paths: BTreeMap<&str, Vec<TaskId>> = BTreeMap::new();
    let mut longest = Vec::new();
    for id in order {
        let mut best = vec![id.clone()];
        for predecessor in predecessors.get(id.as_str()).into_iter().flatten() {
            let Some(prefix) = paths.get(predecessor) else {
                continue;
            };
            let mut candidate = prefix.clone();
            candidate.push(id.clone());
            if candidate.len() > best.len() || (candidate.len() == best.len() && candidate < best) {
                best = candidate;
            }
        }
        if best.len() > longest.len() || (best.len() == longest.len() && best < longest) {
            longest = best.clone();
        }
        paths.insert(id.as_str(), best);
    }
    longest
}
