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
use crate::coordination::{ReleaseAfterFailure, ReleaseOutcome, StateManager, WorkerStateInfo};
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::pty::{AgentConfig, AgentRole, WorkerRole};
use crate::session::{AddWorkerError, AddWorkerRejectionReason, SessionController};

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
        prompt_template: None,
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
    let reservation = {
        let controller = state.session_controller.read();
        controller.reserve_add_worker(&session_id, &role, parent_id.as_deref())
    }
    .map_err(add_worker_error_to_api)?;

    // #126: enqueue + atomically claim the worker BEFORE spawning. The queue table is the
    // source of truth, so we compute the deterministic worker_id the same way the controller
    // does (`{session}-worker-{index}`, index = existing worker count + 1), enqueue a
    // `queued` row, then try to claim it. A duplicate POST for the same worker hits an
    // already-`running` row, loses the claim, and is turned away with 409 — no double spawn.
    let predicted_index = reservation.index;
    let predicted_worker_id = reservation.worker_id.clone();
    let queue_id = predicted_worker_id.clone();
    let payload = json!({
        "role_type": role_type,
        "cli": cli,
        "model": config.model,
        "flags": config.flags,
        "parent_id": parent_id,
        "initial_task": initial_task,
    });

    state
        .queue_manager
        .enqueue_worker(
            &queue_id,
            &session_id,
            &predicted_worker_id,
            &role_type,
            &cli,
            payload,
            None,
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let epoch = match state
        .queue_manager
        .claim_and_spawn(&queue_id, &session_id, &predicted_worker_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
    {
        Some(epoch) => epoch,
        None => {
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
        controller.add_worker(
            &session_id,
            config,
            role.clone(),
            parent_id,
            Some(reservation.index),
        )
    };

    let agent_info = match add_result {
        Ok(agent_info) => agent_info,
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
        let discard = {
            let controller = state.session_controller.read();
            controller.discard_worker_slot(&session_id, &agent_info.id)
        };
        if let Err(error) = discard {
            tracing::warn!(
                worker_id = %agent_info.id,
                %error,
                "failed to discard the dead worker's roster slot"
            );
        }

        let mut details: HashMap<String, Value> = HashMap::new();
        details.insert("worker_id".to_string(), json!(predicted_worker_id));
        details.insert("session_id".to_string(), json!(session_id));
        details.insert("reason".to_string(), json!("failed_to_start"));
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
            format!(
                "Worker {} spawned but its process died during startup; the slot has \
                 been freed for an in-place retry",
                predicted_worker_id
            ),
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

    // Scope every !Send guard before the await below.
    let (rostered, is_worker_slot, live_pty) = {
        let controller = state.session_controller.read();
        let session = controller
            .get_session(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
        let rostered_agent = session.agents.iter().find(|a| a.id == worker_id);
        let rostered = rostered_agent.is_some();
        let is_worker_slot = rostered_agent
            .map(|a| matches!(a.role, AgentRole::Worker { .. }))
            .unwrap_or(false);
        let live_pty = state.pty_manager.read().is_alive(&worker_id);
        (rostered, is_worker_slot, live_pty)
    };

    // #207 fix 4: for WORKER slots, refusal now requires a live process, not mere roster
    // membership. Under the old rules a worker whose CLI died at startup — still
    // rostered, reported Running, no process behind it — could never be recovered in
    // place: release refused with "rostered", DELETE moved it to Completed but kept the
    // entry, and a re-spawn advanced the index, orphaning the original task-file paths.
    // Non-worker roster members (queen, evaluator, prince, QA) keep the unconditional
    // refusal: this route recovers worker slots, nothing else.
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
            format!("Worker {} still has a live PTY; refusing to release its claim", worker_id),
            details,
        ));
    }

    // Rostered but dead: discard the roster entry (and its launch artifacts) so the
    // index becomes reusable, then release the claim below as usual.
    let discarded_rostered_slot = rostered;
    if rostered {
        let discard = {
            let controller = state.session_controller.read();
            controller.discard_worker_slot(&session_id, &worker_id)
        };
        if let Err(error) = discard {
            return Err(ApiError::internal(format!(
                "Worker {} has no live process but its roster slot could not be discarded: {}",
                worker_id, error
            )));
        }
    }

    let outcome = state
        .queue_manager
        .release_claim(&session_id, &worker_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

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
