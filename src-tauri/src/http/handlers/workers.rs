use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::coordination::{StateManager, WorkerStateInfo};
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::pty::{AgentConfig, AgentRole, WorkerRole};
use super::{validate_session_id, validate_cli};

/// Request to add a worker to a session
#[derive(Debug, Clone, Deserialize)]
pub struct AddWorkerRequest {
    /// Role type: backend, frontend, coherence, simplify, reviewer, resolver, tester, etc.
    pub role_type: String,
    /// Optional custom label for the worker
    pub label: Option<String>,
    /// CLI to use: claude, gemini, cursor, droid, qwen, etc. Defaults to "claude"
    pub cli: Option<String>,
    /// Model to use (optional)
    pub model: Option<String>,
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

    let session_default_cli = {
        let controller = state.session_controller.read();
        controller.get_session_default_cli(&session_id)
            .unwrap_or_else(|| "claude".to_string())
    };
    let cli = req.cli.unwrap_or(session_default_cli);
    validate_cli(&cli)?;

    // Build role
    let role_label = req.label.unwrap_or_else(|| {
        // Capitalize first letter of role_type
        let mut chars = req.role_type.chars();
        match chars.next() {
            None => req.role_type.clone(),
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        }
    });

    let role = WorkerRole {
        role_type: req.role_type.clone(),
        label: role_label.clone(),
        default_cli: cli.clone(),
        prompt_template: None,
    };

    // Build config
    let config = AgentConfig {
        cli: cli.clone(),
        model: req.model,
        flags: vec![],
        label: Some(role_label.clone()),
        role: Some(role.clone()),
        initial_prompt: req.initial_task.clone(),
    };

    // Add worker through session controller
    let (worker_id, worker_index) = {
        let controller = state.session_controller.write();

        let agent_info = controller
            .add_worker(&session_id, config, role.clone(), req.parent_id)
            .map_err(|e| ApiError::internal(e.to_string()))?;

        // Extract worker index from ID (format: session-id-worker-N)
        let index = agent_info.id
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

    let task_file = format!(".hive-manager/{}/tasks/worker-{}-task.md", session_id, worker_index);

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
        .map(|a| {
            let index = a.id.rsplit('-').next().unwrap_or("0");
            json!({
                "id": a.id,
                "role": a.config.role.as_ref().map(|r| &r.label).unwrap_or(&"Worker".to_string()),
                "cli": a.config.cli,
                "status": format!("{:?}", a.status),
                "task_file": format!(".hive-manager/{}/tasks/worker-{}-task.md", session_id, index)
            })
        })
        .collect();

    Ok(Json(json!({
        "session_id": session_id,
        "workers": workers,
        "count": workers.len()
    })))
}
