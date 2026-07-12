use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use super::{validate_cli, validate_session_id};
use crate::cli::CliRegistry;
use crate::coordination::{StateManager, WorkerStateInfo};
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::pty::{AgentConfig, AgentRole, WorkerRole};
use crate::session::SessionController;

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

    // #126: enqueue + atomically claim the worker BEFORE spawning. The queue table is the
    // source of truth, so we compute the deterministic worker_id the same way the controller
    // does (`{session}-worker-{index}`, index = existing worker count + 1), enqueue a
    // `queued` row, then try to claim it. A duplicate POST for the same worker hits an
    // already-`running` row, loses the claim, and is turned away with 409 — no double spawn.
    let predicted_index = {
        let controller = state.session_controller.read();
        let existing = controller
            .get_session(&session_id)
            .map(|s| {
                s.agents
                    .iter()
                    .filter(|a| matches!(a.role, AgentRole::Worker { .. }))
                    .count()
            })
            .unwrap_or(0);
        (existing + 1) as u8
    };
    let predicted_worker_id = format!("{}-worker-{}", session_id, predicted_index);
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

    let claimed = state
        .queue_manager
        .claim_and_spawn(&queue_id, &session_id, &predicted_worker_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if !claimed {
        let mut details: HashMap<String, Value> = HashMap::new();
        details.insert("worker_id".to_string(), json!(predicted_worker_id));
        details.insert("session_id".to_string(), json!(session_id));
        return Err(ApiError::conflict_with_details(
            format!(
                "Worker {} is already claimed and running",
                predicted_worker_id
            ),
            details,
        ));
    }

    // Add worker through session controller
    let (worker_id, worker_index) = {
        let controller = state.session_controller.write();

        let agent_info = controller
            .add_worker(&session_id, config, role.clone(), parent_id)
            .map_err(|e| ApiError::internal(e.to_string()))?;

        // Extract worker index from ID (format: session-id-worker-N)
        let index = agent_info
            .id
            .rsplit('-')
            .next()
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(1);

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
