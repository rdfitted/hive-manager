use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::pty::{AgentConfig, AgentRole};
use super::{validate_session_id, validate_cli};

/// Request to add a planner to a Swarm session (spawned sequentially by Queen)
#[derive(Debug, Clone, Deserialize)]
pub struct AddPlannerRequest {
    /// Domain for this planner (e.g., "backend", "frontend", "testing")
    pub domain: String,
    /// Optional custom label for the planner
    pub label: Option<String>,
    /// CLI to use: claude, gemini, etc. Defaults to "claude"
    pub cli: Option<String>,
    /// Model to use (optional)
    pub model: Option<String>,
    /// Number of workers this planner will manage (for sequential spawning)
    pub worker_count: Option<u8>,
    /// Worker configurations this planner will manage (optional, for pre-defined roles)
    pub workers: Option<Vec<WorkerConfigRequest>>,
}

/// Worker configuration in planner request
#[derive(Debug, Clone, Deserialize)]
pub struct WorkerConfigRequest {
    pub role_type: String,
    pub label: Option<String>,
    pub cli: Option<String>,
}

/// Response after adding a planner
#[derive(Debug, Clone, Serialize)]
pub struct AddPlannerResponse {
    pub planner_id: String,
    pub planner_index: u8,
    pub domain: String,
    pub cli: String,
    pub status: String,
    pub worker_count: usize,
    pub prompt_file: String,
    pub tools_dir: String,
}

/// POST /api/sessions/{id}/planners - Add a new planner to a Swarm session (sequential)
///
/// Queen calls this to spawn planners one at a time. Each planner gets:
/// - Its own terminal window
/// - Tool documentation for spawning workers
/// - Knowledge of how many workers it should spawn
pub async fn add_planner(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<AddPlannerRequest>,
) -> Result<(StatusCode, Json<AddPlannerResponse>), ApiError> {
    validate_session_id(&session_id)?;

    let session_default_cli = {
        let controller = state.session_controller.read();
        controller.get_session_default_cli(&session_id)
            .unwrap_or_else(|| "claude".to_string())
    };
    let cli = req.cli.unwrap_or(session_default_cli);
    validate_cli(&cli)?;
    let model = req.model;

    // Build planner config
    let config = AgentConfig {
        cli: cli.clone(),
        model,
        flags: vec![],
        label: req.label.clone().or_else(|| Some(format!("{} Planner", req.domain))),
        role: None,
        initial_prompt: None,
    };

    // Convert worker configs (or create default based on worker_count)
    // Reuse session_default_cli already fetched above (avoid redundant lock acquisitions)
    let workers: Vec<AgentConfig> = if let Some(worker_configs) = req.workers {
        worker_configs.iter().map(|w| {
            AgentConfig {
                cli: w.cli.clone().unwrap_or(cli.clone()),
                model: None,
                flags: vec![],
                label: w.label.clone(),
                role: Some(crate::pty::WorkerRole {
                    role_type: w.role_type.clone(),
                    label: w.label.clone().unwrap_or_else(|| w.role_type.clone()),
                    default_cli: w.cli.clone().unwrap_or(cli.clone()),
                    prompt_template: None,
                }),
                initial_prompt: None,
            }
        }).collect()
    } else {
        // Create default workers based on worker_count
        let count = req.worker_count.unwrap_or(1) as usize;
        (0..count).map(|i| {
            AgentConfig {
                cli: cli.clone(),
                model: None,
                flags: vec![],
                label: Some(format!("Worker {}", i + 1)),
                role: Some(crate::pty::WorkerRole {
                    role_type: "general".to_string(),
                    label: format!("Worker {}", i + 1),
                    default_cli: cli.clone(),
                    prompt_template: None,
                }),
                initial_prompt: None,
            }
        }).collect()
    };

    let worker_count = workers.len();

    // Add planner through session controller
    let (planner_id, planner_index) = {
        let controller = state.session_controller.write();

        let agent_info = controller
            .add_planner(&session_id, config, req.domain.clone(), workers)
            .map_err(|e| ApiError::internal(e.to_string()))?;

        // Extract planner index from ID (format: session-id-planner-N)
        let index = agent_info.id
            .rsplit('-')
            .next()
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(1);

        (agent_info.id, index)
    };

    let prompt_file = format!(".hive-manager/{}/prompts/planner-{}-prompt.md", session_id, planner_index);
    let tools_dir = format!(".hive-manager/{}/tools/", session_id);

    Ok((
        StatusCode::CREATED,
        Json(AddPlannerResponse {
            planner_id,
            planner_index,
            domain: req.domain,
            cli,
            status: "Running".to_string(),
            worker_count,
            prompt_file,
            tools_dir,
        }),
    ))
}

/// GET /api/sessions/{id}/planners - List planners in a Swarm session
pub async fn list_planners(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let controller = state.session_controller.read();

    let session = controller
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    let planners: Vec<Value> = session
        .agents
        .iter()
        .filter(|a| matches!(a.role, AgentRole::Planner { .. }))
        .map(|a| {
            let index = match a.role {
                AgentRole::Planner { index } => index,
                _ => 0,
            };
            json!({
                "id": a.id,
                "index": index,
                "cli": a.config.cli,
                "label": a.config.label,
                "status": format!("{:?}", a.status),
                "prompt_file": format!(".hive-manager/{}/prompts/planner-{}-prompt.md", session_id, index)
            })
        })
        .collect();

    Ok(Json(json!({
        "session_id": session_id,
        "planners": planners,
        "count": planners.len()
    })))
}
