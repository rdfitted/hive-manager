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
use crate::session::{FusionLaunchConfig, FusionVariantConfig, FusionVariantStatus};
use crate::storage::SessionTypeInfo;
use super::{validate_session_id, validate_cli, validate_project_path};

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

#[derive(Deserialize)]
pub struct LaunchFusionVariantRequest {
    pub name: String,
    pub cli: Option<String>,
    pub model: Option<String>,
}

#[derive(Deserialize)]
pub struct LaunchFusionRequest {
    pub project_path: String,
    pub task_description: String,
    pub variants: Vec<LaunchFusionVariantRequest>,
    pub judge_cli: Option<String>,
    pub judge_model: Option<String>,
    pub with_planning: Option<bool>,
    pub default_cli: Option<String>,
    pub default_model: Option<String>,
}

#[derive(Deserialize)]
pub struct SelectFusionWinnerRequest {
    pub variant: String,
}

#[derive(Serialize)]
pub struct LaunchResponse {
    pub session_id: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct FusionStatusResponse {
    pub session_id: String,
    pub state: String,
    pub variants: Vec<FusionVariantStatus>,
}

#[derive(Serialize)]
pub struct FusionEvaluationResponse {
    pub session_id: String,
    pub state: String,
    pub report_path: String,
    pub report: Option<String>,
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

/// POST /api/sessions/fusion - Launch a new Fusion session
pub async fn launch_fusion(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LaunchFusionRequest>,
) -> Result<(StatusCode, Json<LaunchResponse>), ApiError> {
    if req.variants.is_empty() {
        return Err(ApiError::bad_request("Fusion launch requires at least one variant"));
    }
    if req.task_description.trim().is_empty() {
        return Err(ApiError::bad_request("task_description cannot be empty"));
    }

    // Validate project path for security (prevent path traversal)
    validate_project_path(&req.project_path)?;

    let default_cli = req.default_cli.unwrap_or_else(|| "claude".to_string());
    validate_cli(&default_cli)?;

    let judge_cli = req.judge_cli.unwrap_or_else(|| default_cli.clone());
    validate_cli(&judge_cli)?;

    let variants = req
        .variants
        .into_iter()
        .map(|v| {
            let cli = v.cli.unwrap_or_else(|| default_cli.clone());
            validate_cli(&cli)?;
            Ok(FusionVariantConfig {
                name: v.name,
                cli,
                model: v.model,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;

    let judge_config = AgentConfig {
        cli: judge_cli,
        model: req.judge_model.or(req.default_model.clone()),
        flags: vec![],
        label: Some("Fusion Judge".to_string()),
        role: None,
        initial_prompt: None,
    };

    let config = FusionLaunchConfig {
        project_path: req.project_path,
        variants,
        task_description: req.task_description,
        judge_config,
        queen_config: None,
        with_planning: req.with_planning.unwrap_or(false),
        default_cli,
        default_model: req.default_model,
    };

    let controller = state.session_controller.write();
    let session = controller
        .launch_fusion(config)
        .map_err(ApiError::internal)?;

    Ok((
        StatusCode::CREATED,
        Json(LaunchResponse {
            session_id: session.id,
            message: "Fusion session launched".to_string(),
        }),
    ))
}

/// POST /api/sessions/{id}/fusion/select-winner - Select and squash-merge winner
pub async fn select_fusion_winner(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SelectFusionWinnerRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_session_id(&id)?;
    if req.variant.trim().is_empty() {
        return Err(ApiError::bad_request("variant cannot be empty"));
    }

    let controller = state.session_controller.write();
    controller
        .select_fusion_winner(&id, &req.variant)
        .map_err(ApiError::internal)?;

    Ok(Json(serde_json::json!({
        "session_id": id,
        "message": format!("Selected '{}' as fusion winner", req.variant)
    })))
}

/// GET /api/sessions/{id}/fusion/status - Get fusion variant statuses
pub async fn get_fusion_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<FusionStatusResponse>, ApiError> {
    validate_session_id(&id)?;

    let controller = state.session_controller.read();
    if controller.get_session(&id).is_none() {
        return Err(ApiError::not_found(format!("Session {} not found", id)));
    }

    let variants = controller
        .get_fusion_variant_statuses(&id)
        .map_err(ApiError::internal)?;
    let state_str = controller
        .get_session(&id)
        .map(|s| format!("{:?}", s.state))
        .unwrap_or_else(|| "Unknown".to_string());

    Ok(Json(FusionStatusResponse {
        session_id: id,
        state: state_str,
        variants,
    }))
}

/// GET /api/sessions/{id}/fusion/evaluation - Get judge report
pub async fn get_fusion_evaluation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<FusionEvaluationResponse>, ApiError> {
    validate_session_id(&id)?;

    let controller = state.session_controller.read();
    if controller.get_session(&id).is_none() {
        return Err(ApiError::not_found(format!("Session {} not found", id)));
    }

    let (report_path, report) = controller
        .get_fusion_evaluation(&id)
        .map_err(ApiError::internal)?;
    let state_str = controller
        .get_session(&id)
        .map(|s| format!("{:?}", s.state))
        .unwrap_or_else(|| "Unknown".to_string());

    Ok(Json(FusionEvaluationResponse {
        session_id: id,
        state: state_str,
        report_path,
        report,
    }))
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
