use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::{validate_cli, validate_session_id};
use crate::cli::CliRegistry;
use crate::coordination::queue_manager::{ClaimOutcome, GuardedRelease};
use crate::coordination::{ReleaseAfterFailure, ReleaseOutcome, StateManager, WorkerStateInfo};
use crate::domain::WorkspaceStrategy;
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::orchestrator::org_graph::definitions::role_prompt_template;
use crate::pty::{AgentConfig, AgentRole, WorkerRole};
use crate::session::{AddWorkerError, AddWorkerRejectionReason, SessionController};
use crate::storage::queue::{
    QueueConflictAction, QueueConflictCoverage, QueueConflictRow, QueueResolutionUpdate,
};

struct QueueSchedulingFacts {
    prerequisite_task_ids: Vec<String>,
    resolution: QueueResolutionUpdate,
    conflicts: Vec<QueueConflictRow>,
    conflict_coverage: Option<QueueConflictCoverage>,
    reconcile_conflict_task_id: Option<String>,
}

fn queue_scheduling_facts(
    state: &AppState,
    session_id: &str,
    task_id: Option<&str>,
    workspace_strategy: WorkspaceStrategy,
) -> Result<QueueSchedulingFacts, ApiError> {
    let Some(task_id) = task_id else {
        return Ok(QueueSchedulingFacts {
            prerequisite_task_ids: Vec::new(),
            resolution: QueueResolutionUpdate::Preserve,
            conflicts: Vec::new(),
            conflict_coverage: None,
            reconcile_conflict_task_id: None,
        });
    };
    let state_manager = StateManager::new(state.storage.session_dir(session_id));
    let composition = state_manager
        .read_graph_composition_state()
        .map_err(|error| {
            ApiError::internal(format!(
                "Could not read the authoritative graph composition for task {task_id}: {error}"
            ))
        })?;
    let legacy_graph = if composition.is_none() {
        state_manager.read_work_graph().map_err(|error| {
            ApiError::internal(format!(
                "Could not read the authoritative work graph for task {task_id}: {error}"
            ))
        })?
    } else {
        None
    };
    let graph = composition
        .as_ref()
        .map(|state| &state.graph)
        .or(legacy_graph.as_ref());

    let (prerequisite_task_ids, resolution) = if let Some(graph) = graph {
        if graph.nodes.iter().any(|node| node.id == task_id) {
            (
                graph
                    .edges
                    .iter()
                    .filter(|edge| {
                        edge.kind == crate::orchestrator::work_graph::EdgeKind::DependsOn
                            && edge.target == task_id
                    })
                    .map(|edge| edge.source.clone())
                    .collect(),
                QueueResolutionUpdate::Resolved,
            )
        } else if graph
            .edges
            .iter()
            .any(|edge| edge.kind == crate::orchestrator::work_graph::EdgeKind::DependsOn)
        {
            (
                Vec::new(),
                QueueResolutionUpdate::ResolutionIncomplete {
                    task_id: task_id.to_string(),
                    reason: format!(
                        "explicit task {task_id} is absent from the authoritative dependency-constrained work graph"
                    ),
                },
            )
        } else {
            // No graph, a complete edgeless graph, or a degraded empty graph retains legacy
            // FIFO and any existing omission. It cannot truthfully clear an earlier resolution
            // issue; only a later authoritative graph containing the task does that.
            (Vec::new(), QueueResolutionUpdate::Preserve)
        }
    } else {
        (Vec::new(), QueueResolutionUpdate::Preserve)
    };

    let (conflicts, conflict_coverage, reconcile_conflict_task_id) = if let Some(composition) = composition.as_ref() {
        use crate::orchestrator::work_graph::codegraph::{
            conflicting_ready_tasks, ConflictDetectionState, ParallelConflictAction,
        };
        let mut projected = state
            .queue_manager
            .project_queue_statuses(session_id, &composition.graph)
            .map_err(|error| ApiError::internal(error.to_string()))?;
        // Conflict materialization covers both tasks claimable now and peers already running.
        // Mapping Running -> Ready is analytical only; it cannot affect the SQL claim decision.
        for node in &mut projected.nodes {
            if node.status == crate::orchestrator::work_graph::NodeStatus::Running {
                node.status = crate::orchestrator::work_graph::NodeStatus::Ready;
            }
        }
        let active_task_ids =
            crate::orchestrator::work_graph::review::checkpoint_aware_claimable_nodes(&projected);
        let report = conflicting_ready_tasks(
            &projected,
            &composition.codegraph,
            workspace_strategy,
        );
        let globally_unresolved = &composition.codegraph.unresolved_task_ids;
        let coverage = QueueConflictCoverage {
            state: match (report.state, globally_unresolved.is_empty()) {
                (ConflictDetectionState::Disabled, _) => "disabled",
                (_, false) => "partial",
                (ConflictDetectionState::Complete, true) => "complete",
                (ConflictDetectionState::Partial, true) => "partial",
            }
            .to_string(),
            unresolved_task_ids: {
                let mut ids = globally_unresolved.clone();
                ids.extend(report.unresolved_ready_task_ids);
                ids.sort();
                ids.dedup();
                ids
            },
        };
        let conflicts = report
            .decisions
            .into_iter()
            .flat_map(|decision| {
                let action = match decision.action {
                    ParallelConflictAction::Serialize => QueueConflictAction::Serialize,
                    ParallelConflictAction::WorktreeIsolate => {
                        QueueConflictAction::WorktreeIsolate
                    }
                };
                let forward = QueueConflictRow {
                    session_id: session_id.to_string(),
                    task_id: decision.first_task_id.clone(),
                    conflicting_task_id: decision.second_task_id.clone(),
                    action,
                    reason: decision.reason.clone(),
                };
                let reverse = QueueConflictRow {
                    session_id: session_id.to_string(),
                    task_id: decision.second_task_id,
                    conflicting_task_id: decision.first_task_id,
                    action,
                    reason: decision.reason,
                };
                [forward, reverse]
            })
            .collect();
        let reconcile_conflict_task_id = (coverage.state == "complete"
            && active_task_ids.iter().any(|active| active == task_id))
        .then(|| task_id.to_string());
        (conflicts, Some(coverage), reconcile_conflict_task_id)
    } else {
        (Vec::new(), None, None)
    };

    Ok(QueueSchedulingFacts {
        prerequisite_task_ids,
        resolution,
        conflicts,
        conflict_coverage,
        reconcile_conflict_task_id,
    })
}

/// Map a pre-spawn rejection to an accurate HTTP status (#175d).
///
/// A session that simply is not accepting workers right now is a conflict the
/// caller can act on, not an internal error — the old blanket 500 gave the Queen
/// nothing to work with.
/// Last portion of a CLI's captured output, sized for an API error payload (#207).
fn tail_for_api(output: &str) -> String {
    const MAX_BYTES: usize = 2000;
    let trimmed = output.trim();
    if trimmed.len() <= MAX_BYTES {
        return trimmed.to_string();
    }
    let mut start = trimmed.len() - MAX_BYTES;
    while !trimmed.is_char_boundary(start) {
        start += 1;
    }
    trimmed[start..].to_string()
}

fn add_worker_error_to_api(err: AddWorkerError) -> ApiError {
    match err {
        AddWorkerError::SessionNotFound(id) => {
            ApiError::not_found(format!("Session {} not found", id))
        }
        AddWorkerError::Rejected(rejection) => match rejection.reason {
            AddWorkerRejectionReason::StateNotAcceptingWorkers => {
                let mut details: HashMap<String, Value> = HashMap::new();
                details.insert("reason".to_string(), json!("session_state"));
                details.insert(
                    "current_state".to_string(),
                    json!(rejection.current_state.clone()),
                );
                ApiError::conflict_with_details(rejection.error, details)
            }
            _ => ApiError::bad_request(rejection.error),
        },
    }
}

fn deserialize_optional_trimmed_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }))
}

/// Request to add a worker to a session
#[derive(Debug, Clone, Deserialize)]
pub struct AddWorkerRequest {
    /// Role type: backend, frontend, coherence, simplify, reviewer, resolver, tester, etc.
    pub role_type: String,
    /// Optional custom label for the worker
    pub label: Option<String>,
    /// Stable worker name
    #[serde(default, deserialize_with = "deserialize_optional_trimmed_string")]
    pub name: Option<String>,
    /// One-line task summary used for deterministic labels
    #[serde(default, deserialize_with = "deserialize_optional_trimmed_string")]
    pub description: Option<String>,
    /// CLI to use. Defaults to the session's configured principal CLI.
    pub cli: Option<String>,
    /// Model to use (optional)
    pub model: Option<String>,
    /// Additional CLI flags. Omit to inherit the session principal flags; use [] to clear them.
    pub flags: Option<Vec<String>>,
    /// Initial task/prompt for the worker
    pub initial_task: Option<String>,
    /// Stable work-graph node ID. Explicit or null; never inferred from free-text task prose.
    #[serde(default)]
    pub task_id: Option<String>,
    /// Parent agent ID (defaults to Queen)
    pub parent_id: Option<String>,
}

/// Response after adding a worker
#[derive(Debug, Clone, Serialize)]
pub struct AddWorkerResponse {
    pub worker_id: String,
    pub role: String,
    pub cli: String,
    pub status: String,
    pub task_file: String,
}

/// POST /api/sessions/{id}/workers - Add a new worker to a session
pub async fn add_worker(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<AddWorkerRequest>,
) -> Result<(StatusCode, Json<AddWorkerResponse>), ApiError> {
    validate_session_id(&session_id)?;

    let AddWorkerRequest {
        role_type,
        label,
        name,
        description,
        cli: requested_cli,
        model: requested_model,
        flags: requested_flags,
        initial_task,
        task_id,
        parent_id,
    } = req;

    let principal_defaults = {
        let controller = state.session_controller.read();
        controller.get_session_principal_defaults(&session_id)
    }
    .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    let inherits_principal_defaults = match requested_cli.as_deref() {
        None => true,
        Some(requested) => requested == principal_defaults.cli.as_str(),
    };
    let cli = requested_cli.unwrap_or_else(|| principal_defaults.cli.clone());
    validate_cli(&cli)?;
    let model = requested_model.or_else(|| {
        if inherits_principal_defaults {
            principal_defaults.model.clone()
        } else {
            CliRegistry::default_model(&cli).map(ToString::to_string)
        }
    });
    let flags = requested_flags.unwrap_or_else(|| {
        if inherits_principal_defaults {
            principal_defaults.flags.clone()
        } else {
            Vec::new()
        }
    });

    // Build role
    let role_label = label.unwrap_or_else(|| {
        // Capitalize first letter of role_type
        let mut chars = role_type.chars();
        match chars.next() {
            None => role_type.clone(),
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        }
    });

    let role = WorkerRole {
        role_type: role_type.clone(),
        label: role_label.clone(),
        default_cli: cli.clone(),
        prompt_template: Some(role_prompt_template(&role_type)),
        resolved_definition: None,
    };

    // Build config
    let config = AgentConfig {
        cli: cli.clone(),
        model,
        flags,
        label: Some(role_label.clone()),
        name,
        description,
        role: Some(role.clone()),
        initial_prompt: initial_task.clone(),
    };

    // #175(d): VALIDATE BEFORE CLAIMING. The queue row used to be enqueued and
    // claimed before the controller checked whether the session could accept a
    // worker at all, and no failure path released it. A single rejected spawn
    // therefore stranded the worker index permanently.
    //
    // The pre-check shares one implementation with the real check inside
    // `add_worker`, so they cannot drift, and the common rejection now creates no
    // queue row at all.
    let (reservation, workspace_strategy) = {
        let controller = state.session_controller.read();
        let workspace_strategy = controller
            .get_session(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?
            .execution_policy
            .workspace_strategy;
        let reservation = controller
            .reserve_add_worker(&session_id, &role, parent_id.as_deref())
            .map_err(add_worker_error_to_api)?;
        (reservation, workspace_strategy)
    };

    // #126/#212: enqueue + atomically claim the task BEFORE spawning. Explicit task IDs get a
    // stable queue identity independent of the current roster slot: a dependency-pending task
    // must not monopolize `worker-1` or let a later task claim its row with different config.
    // Legacy null task IDs retain the historical worker-id key. The winning claim atomically
    // rebinds a task-backed row to the current reservation's worker ID.
    let predicted_index = reservation.index;
    let predicted_worker_id = reservation.worker_id.clone();
    let queue_id = task_id.as_deref().map_or_else(
        || predicted_worker_id.clone(),
        |task_id| {
            format!(
                "task:{}",
                uuid::Uuid::new_v5(
                    &uuid::Uuid::NAMESPACE_URL,
                    format!("hive-manager:queue-task:{session_id}:{task_id}").as_bytes(),
                )
            )
        },
    );
    let queued_worker_id = if task_id.is_some() {
        format!("pending:{queue_id}")
    } else {
        predicted_worker_id.clone()
    };
    let payload = json!({
        "role_type": role_type,
        "cli": cli,
        "model": config.model,
        "flags": config.flags,
        "parent_id": parent_id,
        "initial_task": initial_task,
        "task_id": task_id,
    });

    // Persisted graph state is the only dependency/conflict source. These facts are derived
    // before enqueue but never authorize a claim: the row and all materialized facts commit in
    // one transaction, then the single SQL UPDATE remains the sole decision boundary.
    let scheduling = queue_scheduling_facts(
        &state,
        &session_id,
        task_id.as_deref(),
        workspace_strategy,
    )?;

    state
        .queue_manager
        .enqueue_worker_with_scheduling(
            &queue_id,
            &session_id,
            &queued_worker_id,
            &role_type,
            &cli,
            payload,
            task_id.clone(),
            &scheduling.prerequisite_task_ids,
            &scheduling.resolution,
            &scheduling.conflicts,
            scheduling.conflict_coverage.as_ref(),
            scheduling.reconcile_conflict_task_id.as_deref(),
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let epoch = match state
        .queue_manager
        .claim_and_reserve_spawn(
            &queue_id,
            &session_id,
            &predicted_worker_id,
            task_id.as_deref(),
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
    {
        ClaimOutcome::Claimed { epoch } => epoch,
        ClaimOutcome::ResolutionIncomplete { task_id, reason } => {
            let mut details: HashMap<String, Value> = HashMap::new();
            details.insert("worker_id".to_string(), json!(predicted_worker_id));
            details.insert("session_id".to_string(), json!(session_id));
            details.insert("reason".to_string(), json!("resolution_incomplete"));
            details.insert("task_id".to_string(), json!(task_id));
            details.insert("detail".to_string(), json!(reason));
            details.insert("retryable".to_string(), json!(true));
            return Err(ApiError::conflict_with_details(
                "The explicit plan task is not yet resolvable in the authoritative dependency graph",
                details,
            ));
        }
        ClaimOutcome::ConflictsPending { conflicts } => {
            let mut details: HashMap<String, Value> = HashMap::new();
            details.insert("worker_id".to_string(), json!(predicted_worker_id));
            details.insert("session_id".to_string(), json!(session_id));
            details.insert("reason".to_string(), json!("conflicts_pending"));
            details.insert(
                "blocking_task_ids".to_string(),
                json!(conflicts
                    .iter()
                    .map(|conflict| conflict.task_id.clone())
                    .collect::<Vec<_>>()),
            );
            details.insert(
                "conflict_reasons".to_string(),
                json!(conflicts
                    .iter()
                    .map(|conflict| conflict.reason.clone())
                    .collect::<Vec<_>>()),
            );
            details.insert("retryable".to_string(), json!(true));
            return Err(ApiError::conflict_with_details(
                "Worker is waiting for a conflicting serialized task to finish",
                details,
            ));
        }
        ClaimOutcome::DependenciesPending { task_ids } => {
            let mut details: HashMap<String, Value> = HashMap::new();
            details.insert("worker_id".to_string(), json!(predicted_worker_id));
            details.insert("session_id".to_string(), json!(session_id));
            details.insert("reason".to_string(), json!("dependencies_pending"));
            details.insert("blocking_task_ids".to_string(), json!(task_ids));
            details.insert("retryable".to_string(), json!(true));
            return Err(ApiError::conflict_with_details(
                format!(
                    "Worker {} is waiting for queue readiness",
                    predicted_worker_id
                ),
                details,
            ));
        }
        ClaimOutcome::AlreadyClaimed => {
            let mut details: HashMap<String, Value> = HashMap::new();
            details.insert("worker_id".to_string(), json!(predicted_worker_id));
            details.insert("session_id".to_string(), json!(session_id));
            details.insert("reason".to_string(), json!("already_claimed"));
            return Err(ApiError::conflict_with_details(
                format!(
                    "Worker {} is already claimed and running",
                    predicted_worker_id
                ),
                details,
            ));
        }
    };

    // Add worker through session controller. The parking_lot guard is scoped out
    // before the release await below — it is !Send and cannot cross an await.
    let add_result = {
        let controller = state.session_controller.write();
        controller.add_worker_for_plan_task(
            &session_id,
            config,
            role.clone(),
            parent_id,
            Some(reservation.index),
            task_id.as_deref(),
        )
    };

    let agent_info = match add_result {
        Ok(agent_info) => {
            // The expensive worktree/PTY creation finished. Refresh and hand off only this
            // exact epoch/worker, then clear its in-process spawn reservation. A failed handoff
            // retains the marker (fail closed); returning the already-live worker is safer than
            // turning a diagnostic/storage race into an operator retry and double spawn.
            match state.queue_manager.complete_spawn_handoff(
                &queue_id,
                epoch,
                &predicted_worker_id,
            ) {
                Ok(true) => {}
                Ok(false) => tracing::error!(
                    queue_id = %queue_id,
                    worker_id = %predicted_worker_id,
                    epoch,
                    "spawn succeeded but its fenced queue handoff did not match; retaining the fail-closed reservation"
                ),
                Err(error) => tracing::error!(
                    queue_id = %queue_id,
                    worker_id = %predicted_worker_id,
                    epoch,
                    %error,
                    "spawn succeeded but its fenced queue handoff failed; retaining the fail-closed reservation"
                ),
            }
            agent_info
        }
        Err(err) => {
            // #175(d): EVERY failure after a won claim must release it. Otherwise
            // the index is stranded and no further worker can ever be spawned.
            match state
                .queue_manager
                .release_after_failed_spawn(&session_id, &predicted_worker_id, &queue_id, epoch)
                .await
            {
                Ok(ReleaseAfterFailure::Exhausted { attempts }) => {
                    let mut details: HashMap<String, Value> = HashMap::new();
                    details.insert("worker_id".to_string(), json!(predicted_worker_id));
                    details.insert("session_id".to_string(), json!(session_id));
                    details.insert("reason".to_string(), json!("spawn_failed"));
                    details.insert("attempts".to_string(), json!(attempts));
                    details.insert(
                        "recovery".to_string(),
                        json!(format!(
                            "POST /api/sessions/{}/workers/{}/release",
                            session_id, predicted_worker_id
                        )),
                    );
                    return Err(ApiError::conflict_with_details(
                        format!(
                            "Worker {} failed to spawn {} times and has been retired: {}",
                            predicted_worker_id, attempts, err
                        ),
                        details,
                    ));
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(
                    worker_id = %predicted_worker_id,
                    error = %e,
                    "failed to release the queue claim after a failed spawn"
                ),
            }

            let mut details: HashMap<String, Value> = HashMap::new();
            details.insert("worker_id".to_string(), json!(predicted_worker_id));
            details.insert("session_id".to_string(), json!(session_id));
            if err.starts_with("Worker index race") {
                details.insert("reason".to_string(), json!("index_raced"));
                return Err(ApiError::conflict_with_details(err, details));
            }
            return Err(ApiError::internal_with_details(err, details));
        }
    };

    // #207 fix 2: the spawn call returning is not evidence the worker is running. In the
    // field, codex processes died within moments of starting ("database is locked") while
    // the roster reported them Running for ~50 minutes. Hold the response until the
    // process has survived a short startup grace window; if it dies inside the window,
    // fail loudly with the CLI's own output and recover the slot in place.
    let startup_grace = Duration::from_millis(if cfg!(test) { 250 } else { 1500 });
    let startup_poll = Duration::from_millis(50);
    let started_at = std::time::Instant::now();
    let died_during_startup = loop {
        let alive = { state.pty_manager.read().is_alive(&agent_info.id) };
        if !alive {
            break true;
        }
        if started_at.elapsed() >= startup_grace {
            break false;
        }
        tokio::time::sleep(startup_poll).await;
    };

    if died_during_startup {
        // Give the reader thread a beat to drain the PTY's final bytes, then capture the
        // output BEFORE killing: kill() drops the session, and with it the only record of
        // why the CLI died.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let cli_output = { state.pty_manager.read().recent_output(&agent_info.id) }
            .unwrap_or_default();
        {
            let _ = state.pty_manager.read().kill(&agent_info.id);
        }

        // Free the slot so a retry respawns worker-N in place instead of advancing the
        // index and orphaning the original branch/task-file paths.
        let slot_freed = {
            let controller = state.session_controller.read();
            match controller.discard_worker_slot(&session_id, &agent_info.id) {
                Ok(_) => true,
                Err(error) => {
                    tracing::warn!(
                        worker_id = %agent_info.id,
                        %error,
                        "failed to discard the dead worker's roster slot"
                    );
                    false
                }
            }
        };

        let mut details: HashMap<String, Value> = HashMap::new();
        details.insert("worker_id".to_string(), json!(predicted_worker_id));
        details.insert("session_id".to_string(), json!(session_id));
        details.insert("reason".to_string(), json!("failed_to_start"));
        details.insert("slot_freed".to_string(), json!(slot_freed));
        details.insert("cli_output".to_string(), json!(tail_for_api(&cli_output)));

        match state
            .queue_manager
            .release_after_failed_spawn(&session_id, &predicted_worker_id, &queue_id, epoch)
            .await
        {
            Ok(ReleaseAfterFailure::Exhausted { attempts }) => {
                details.insert("attempts".to_string(), json!(attempts));
                return Err(ApiError::conflict_with_details(
                    format!(
                        "Worker {} failed to start {} times and has been retired",
                        predicted_worker_id, attempts
                    ),
                    details,
                ));
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(
                worker_id = %predicted_worker_id,
                error = %e,
                "failed to release the queue claim after a startup death"
            ),
        }

        return Err(ApiError::internal_with_details(
            if slot_freed {
                format!(
                    "Worker {} spawned but its process died during startup; the slot has \
                     been freed for an in-place retry",
                    predicted_worker_id
                )
            } else {
                format!(
                    "Worker {} spawned but its process died during startup; its roster \
                     slot could NOT be freed, so an in-place retry is not yet possible",
                    predicted_worker_id
                )
            },
            details,
        ));
    }

    // From here to the response there must be NO fallible early return: the PTY
    // is live, and releasing the claim past this point is a double-spawn vector.
    let (worker_id, worker_index) = {
        // Extract worker index from ID (format: session-id-worker-N)
        let index = agent_info
            .id
            .rsplit('-')
            .next()
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(predicted_index);
        (agent_info.id, index)
    };

    // Update workers.md file
    let session_path = state.storage.session_dir(&session_id);
    let state_manager = StateManager::new(session_path.clone());

    // Get all current workers and update the file
    {
        let controller = state.session_controller.read();
        if let Some(session) = controller.get_session(&session_id) {
            let workers: Vec<WorkerStateInfo> = session
                .agents
                .iter()
                .filter(|a| matches!(a.role, AgentRole::Worker { .. }))
                .map(|a| WorkerStateInfo {
                    id: a.id.clone(),
                    role: a.config.role.clone().unwrap_or_default(),
                    cli: a.config.cli.clone(),
                    status: format!("{:?}", a.status),
                    current_task: None,
                    last_update: chrono::Utc::now(),
                    last_heartbeat: None,
                })
                .collect();

            let _ = state_manager.update_workers_file(&workers);
        }
    }

    // Notify Queen about new worker
    let queen_id = format!("{}-queen", session_id);
    let worker_state = WorkerStateInfo {
        id: worker_id.clone(),
        role: role.clone(),
        cli: cli.clone(),
        status: "Running".to_string(),
        current_task: None,
        last_update: chrono::Utc::now(),
        last_heartbeat: None,
    };

    let _ = state.injection_manager.read().notify_queen_worker_added(
        &session_id,
        &queen_id,
        &worker_state,
    );

    let task_file = {
        let controller = state.session_controller.read();
        let session = controller
            .get_session(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
        SessionController::task_file_path_for_session_worker(&session, worker_index as usize)
            .map_err(ApiError::internal)?
            .to_string_lossy()
            .to_string()
    };

    Ok((
        StatusCode::CREATED,
        Json(AddWorkerResponse {
            worker_id,
            role: role_label,
            cli,
            status: "Running".to_string(),
            task_file,
        }),
    ))
}

/// POST /api/sessions/{id}/workers/{worker_id}/release
///
/// #175(e): recover a durable queue claim whose worker never made it onto the
/// roster. Before this there was no API at all — a leaked claim capped the
/// session permanently and only a backend restart cleared it.
///
/// Deliberately narrow: it releases the CLAIM, never the worker. It does not
/// touch the roster (which keeps the worker-index allocator monotone), does not
/// kill a PTY, and refuses outright if the worker is actually rostered — that
/// case belongs to `DELETE /api/sessions/{id}/agents/{agent_id}`.
pub async fn release_worker(
    State(state): State<Arc<AppState>>,
    Path((session_id, worker_id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    validate_session_id(&session_id)?;
    super::validate_agent_id(&worker_id)?;

    // Claim-and-register and this entire roster-preparation + durable-release sequence share
    // one QueueManager mutex. A deny-only precheck would be TOCTOU: a new spawn reservation
    // could appear after the check but before the dead roster slot was discarded.
    let guarded = state
        .queue_manager
        .release_claim_with_preparation(&session_id, &worker_id, || {
            let (rostered, is_worker_slot, live_pty) = {
                let controller = state.session_controller.read();
                let session = controller.get_session(&session_id).ok_or_else(|| {
                    ApiError::not_found(format!("Session {} not found", session_id))
                })?;
                let rostered_agent = session.agents.iter().find(|a| a.id == worker_id);
                let rostered = rostered_agent.is_some();
                let is_worker_slot = rostered_agent
                    .map(|a| matches!(a.role, AgentRole::Worker { .. }))
                    .unwrap_or(false);
                let live_pty = state.pty_manager.read().is_alive(&worker_id);
                (rostered, is_worker_slot, live_pty)
            };

            // #207 fix 4: for worker slots, refusal requires a live process rather than mere
            // roster membership. Non-worker roster members keep unconditional refusal.
            if rostered && (live_pty || !is_worker_slot) {
                let mut details: HashMap<String, Value> = HashMap::new();
                details.insert("reason".to_string(), json!("rostered"));
                details.insert("worker_id".to_string(), json!(worker_id));
                return Err(ApiError::conflict_with_details(
                    format!(
                        "Worker {} is a live member of this session. Releasing its claim would strand \
                         the queue row. Use DELETE /api/sessions/{}/agents/{} to stop the agent instead.",
                        worker_id, session_id, worker_id
                    ),
                    details,
                ));
            }
            if live_pty {
                let mut details: HashMap<String, Value> = HashMap::new();
                details.insert("reason".to_string(), json!("live_pty"));
                details.insert("worker_id".to_string(), json!(worker_id));
                return Err(ApiError::conflict_with_details(
                    format!(
                        "Worker {} still has a live PTY; refusing to release its claim",
                        worker_id
                    ),
                    details,
                ));
            }

            // Rostered but dead: discard its slot before releasing the queue row. The manager
            // guard prevents a replacement claim from registering until both actions finish.
            if rostered {
                let discard = {
                    let controller = state.session_controller.read();
                    controller.discard_worker_slot(&session_id, &worker_id)
                };
                if let Err(error) = discard {
                    let mut details: HashMap<String, Value> = HashMap::new();
                    details.insert("reason".to_string(), json!("slot_has_work"));
                    details.insert("worker_id".to_string(), json!(worker_id));
                    return Err(ApiError::conflict_with_details(error, details));
                }
            }
            Ok(rostered)
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let (discarded_rostered_slot, outcome) = match guarded {
        GuardedRelease::SpawnInFlight(reservation) => {
            let mut details: HashMap<String, Value> = HashMap::new();
            details.insert("reason".to_string(), json!("spawn_in_flight"));
            details.insert("worker_id".to_string(), json!(worker_id));
            details.insert("queue_id".to_string(), json!(reservation.queue_id));
            details.insert("epoch".to_string(), json!(reservation.epoch));
            details.insert("retryable".to_string(), json!(true));
            return Err(ApiError::conflict_with_details(
                "The worker claim is still constructing its worktree/PTY; release is denied until handoff",
                details,
            ));
        }
        GuardedRelease::PreparationFailed(error) => return Err(error),
        GuardedRelease::Complete { prepared, outcome } => (prepared, outcome),
    };

    match outcome {
        ReleaseOutcome::Released { previous } => Ok((
            StatusCode::OK,
            Json(json!({
                "session_id": session_id,
                "worker_id": worker_id,
                "released": true,
                "previous_status": previous.as_tag(),
                "new_status": "queued",
                "discarded_rostered_slot": discarded_rostered_slot,
            })),
        )),
        ReleaseOutcome::AlreadyQueued => Ok((
            StatusCode::OK,
            Json(json!({
                "session_id": session_id,
                "worker_id": worker_id,
                "released": false,
                "previous_status": "queued",
                "new_status": "queued",
                "discarded_rostered_slot": discarded_rostered_slot,
            })),
        )),
        ReleaseOutcome::SpawnInFlight { epoch } => {
            let mut details: HashMap<String, Value> = HashMap::new();
            details.insert("reason".to_string(), json!("spawn_in_flight"));
            details.insert("worker_id".to_string(), json!(worker_id));
            details.insert("epoch".to_string(), json!(epoch));
            details.insert("retryable".to_string(), json!(true));
            Err(ApiError::conflict_with_details(
                "The worker claim is still constructing its worktree/PTY; release is denied until handoff",
                details,
            ))
        }
        ReleaseOutcome::Terminal { status } => Ok((
            StatusCode::OK,
            Json(json!({
                "session_id": session_id,
                "worker_id": worker_id,
                "released": false,
                "previous_status": status.as_tag(),
                "new_status": status.as_tag(),
                "discarded_rostered_slot": discarded_rostered_slot,
            })),
        )),
        // Launch-time workers predate the durable queue and have no row; if we just
        // discarded such a worker's dead slot, that recovery is the meaningful outcome
        // and deserves a 200, not a 404 (#207).
        ReleaseOutcome::NoRow if discarded_rostered_slot => Ok((
            StatusCode::OK,
            Json(json!({
                "session_id": session_id,
                "worker_id": worker_id,
                "released": false,
                "previous_status": "none",
                "new_status": "none",
                "discarded_rostered_slot": true,
            })),
        )),
        ReleaseOutcome::NoRow => Err(ApiError::not_found(format!(
            "No queue row for worker {} in session {}",
            worker_id, session_id
        ))),
    }
}

/// GET /api/sessions/{id}/workers - List workers in a session
pub async fn list_workers(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let controller = state.session_controller.read();

    let session = controller
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    let workers: Vec<Value> = session
        .agents
        .iter()
        .filter(|a| matches!(a.role, AgentRole::Worker { .. }))
        .map(|a| -> Result<Value, ApiError> {
            let index = a.id.rsplit('-').next().unwrap_or("0");
            let task_file = SessionController::task_file_path_for_session_worker(
                &session,
                index.parse::<usize>().unwrap_or(0),
            )
            .map_err(ApiError::internal)?
            .to_string_lossy()
            .to_string();
            Ok(json!({
                "id": a.id,
                "role": a.config.role.as_ref().map(|r| &r.label).unwrap_or(&"Worker".to_string()),
                "cli": a.config.cli,
                "status": format!("{:?}", a.status),
                "task_file": task_file
            }))
        })
        .collect::<Result<_, _>>()?;

    Ok(Json(json!({
        "session_id": session_id,
        "workers": workers,
        "count": workers.len()
    })))
}
