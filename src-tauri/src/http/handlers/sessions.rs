use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::pty::AgentConfig;
use crate::storage::SessionTypeInfo;
use super::{validate_session_id, validate_cli};

#[derive(Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub session_type: String,
    pub status: String,
    pub project_path: String,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionInfo>,
}

#[derive(Deserialize)]
pub struct LaunchHiveRequest {
    #[allow(dead_code)]
    pub issue_url: Option<String>,
    pub task_description: Option<String>,
    pub worker_count: Option<u8>,
    pub project_path: String,
    pub command: Option<String>,
}

#[derive(Deserialize)]
pub struct LaunchSwarmRequest {
    #[allow(dead_code)]
    pub issue_url: Option<String>,
    pub task_description: Option<String>,
    pub planner_count: Option<u8>,
    pub project_path: String,
}

#[derive(Serialize)]
pub struct LaunchResponse {
    pub session_id: String,
    pub message: String,
}

/// GET /api/sessions - List all sessions
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SessionListResponse>, ApiError> {
    let summaries = state.storage.list_sessions()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    
    let sessions = summaries.into_iter().map(|s| SessionInfo {
        id: s.id,
        session_type: s.session_type,
        status: s.state,
        project_path: s.project_path,
        created_at: s.created_at.to_rfc3339(),
    }).collect();

    Ok(Json(SessionListResponse { sessions }))
}

/// GET /api/sessions/{id} - Get session details
pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SessionInfo>, ApiError> {
    validate_session_id(&id)?;

    let controller = state.session_controller.read();
    if let Some(session) = controller.get_session(&id) {
        return Ok(Json(SessionInfo {
            id: session.id.clone(),
            session_type: match &session.session_type {
                crate::session::SessionType::Hive { worker_count } => format!("Hive ({})", worker_count),
                crate::session::SessionType::Swarm { planner_count } => format!("Swarm ({})", planner_count),
                crate::session::SessionType::Fusion { variants } => format!("Fusion ({})", variants.len()),
            },
            status: format!("{:?}", session.state),
            project_path: session.project_path.to_string_lossy().to_string(),
            created_at: session.created_at.to_rfc3339(),
        }));
    }

    // Try loading from storage if not active
    let persisted = state.storage.load_session(&id)
        .map_err(|_| ApiError::not_found(format!("Session {} not found", id)))?;

    Ok(Json(SessionInfo {
        id: persisted.id,
        session_type: match &persisted.session_type {
            SessionTypeInfo::Hive { worker_count } => format!("Hive ({})", worker_count),
            SessionTypeInfo::Swarm { planner_count } => format!("Swarm ({})", planner_count),
            SessionTypeInfo::Fusion { variants } => format!("Fusion ({})", variants.len()),
        },
        status: persisted.state,
        project_path: persisted.project_path,
        created_at: persisted.created_at.to_rfc3339(),
    }))
}

/// POST /api/sessions/hive - Launch a new Hive session
pub async fn launch_hive(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LaunchHiveRequest>,
) -> Result<(StatusCode, Json<LaunchResponse>), ApiError> {
    let controller = state.session_controller.write();
    let project_path = std::path::PathBuf::from(req.project_path);

    let command = req.command.unwrap_or_else(|| "claude".to_string());
    validate_cli(&command)?;

    let session = controller.launch_hive(
        project_path,
        req.worker_count.unwrap_or(3),
        &command,
        req.task_description,
    ).map_err(|e| ApiError::internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(LaunchResponse {
            session_id: session.id,
            message: "Hive session launched".to_string(),
        }),
    ))
}

/// POST /api/sessions/swarm - Launch a new Swarm session
pub async fn launch_swarm(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LaunchSwarmRequest>,
) -> Result<(StatusCode, Json<LaunchResponse>), ApiError> {
    let controller = state.session_controller.write();

    let default_cli = "claude".to_string();
    let default_config = AgentConfig {
        cli: default_cli.clone(),
        model: None,
        flags: vec![],
        label: None,
        role: None,
        initial_prompt: None,
    };

    let config = crate::session::SwarmLaunchConfig {
        project_path: req.project_path,
        queen_config: default_config.clone(),
        planner_count: req.planner_count.unwrap_or(2),
        planner_config: default_config.clone(),
        workers_per_planner: vec![default_config.clone(); 2],
        prompt: req.task_description,
        with_planning: false,
        smoke_test: false,
        planners: vec![],
    };

    let session = controller.launch_swarm(config)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(LaunchResponse {
            session_id: session.id,
            message: "Swarm session launched".to_string(),
        }),
    ))
}

/// POST /api/sessions/{id}/stop - Stop a session
pub async fn stop_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_session_id(&id)?;

    let controller = state.session_controller.write();
    controller.stop_session(&id)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "message": format!("Session {} stopped", id)
    })))
}
