use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::coordination::StateManager;
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::orchestrator::work_graph::archive::{list_archives, read_archive, WorkGraphArchive};
use crate::orchestrator::work_graph::divergence::{compute_divergence, DivergenceSummary};
use crate::orchestrator::work_graph::runtime::{mutation_log_snapshot, RuntimeOutcome};
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
    pub lane_assignment: BTreeMap<TaskId, BindingRef>,
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

    let (source, graph, divergence, progress_by_node, omissions) = match query.source {
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
            if let Some(plan) = read_live_graph(&session_dir)? {
                graph_from_live_state(&state, &session_id, query.view, plan)?
            } else {
                let archive = latest_archive(&session_dir, &session_id)?.ok_or_else(|| {
                    ApiError::not_found(format!("Work graph not found: {session_id}"))
                })?;
                graph_from_archive(&archive, query.view)
            }
        }
    };

    let response = project_graph(
        query.view,
        source,
        graph,
        divergence,
        &progress_by_node,
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
            Vec::new(),
        ),
        WorkGraphView::Runtime => (
            WorkGraphSource::Archive,
            archive.runtime_graph.clone(),
            None,
            archive_progress_by_node(archive),
            Vec::new(),
        ),
        WorkGraphView::Divergence => (
            WorkGraphSource::Archive,
            archive.runtime_graph.clone(),
            Some(archive.divergence.clone()),
            archive_progress_by_node(archive),
            Vec::new(),
        ),
    }
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
            Vec::new(),
        ));
    }

    let runtime = state
        .queue_manager
        .project_queue_statuses(session_id, &plan)
        .map_err(|error| {
            ApiError::internal(format!("Failed to project live work-graph status: {error}"))
        })?;
    let (divergence, omissions) = if matches!(view, WorkGraphView::Divergence) {
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
    let progress = live_progress_by_node(state, session_id)?;
    Ok((
        WorkGraphSource::Live,
        runtime,
        divergence,
        progress,
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
    let lane_assignment = graph
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.binding.clone()))
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
        lane_assignment,
        critical_path,
        provenance_by_edge,
        divergence,
        omissions,
    })
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
