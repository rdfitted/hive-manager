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
use crate::session::{FusionLaunchConfig, FusionVariantConfig, FusionVariantStatus, HiveLaunchConfig, QaWorkerConfig};
use crate::storage::SessionTypeInfo;
use super::{validate_session_id, validate_cli, validate_project_path};

const SESSION_COLOR_ALLOWLIST: &[&str] = &[
    "#7aa2f7",
    "#bb9af7",
    "#9ece6a",
    "#e0af68",
    "#7dcfff",
    "#f7768e",
    "#ff9e64",
    "#f7b1d1",
];

fn validate_session_name(name: Option<&str>) -> Result<(), ApiError> {
    let Some(name) = name else {
        return Ok(());
    };

    if name.trim().is_empty() {
        return Err(ApiError::bad_request(
            "Invalid session name: must not be empty or whitespace",
        ));
    }

    if name.chars().count() > 64 {
        return Err(ApiError::bad_request(
            "Invalid session name: must be 64 characters or fewer",
        ));
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err(ApiError::bad_request(
            "Invalid session name: must not contain '..', '/', or '\\'",
        ));
    }

    Ok(())
}

fn validate_session_color(color: Option<&str>) -> Result<(), ApiError> {
    let Some(color) = color else {
        return Ok(());
    };

    if !SESSION_COLOR_ALLOWLIST.contains(&color) && !is_valid_hex_session_color(color) {
        return Err(ApiError::bad_request(format!(
            "Invalid session color '{}'. Valid options: {} or any #RRGGBB hex color",
            color,
            SESSION_COLOR_ALLOWLIST.join(", ")
        )));
    }

    Ok(())
}

fn is_valid_hex_session_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color.chars().skip(1).all(|c| c.is_ascii_hexdigit())
}

#[derive(Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: Option<String>,
    pub color: Option<String>,
    pub session_type: String,
    pub status: String,
    pub project_path: String,
    pub created_at: String,
    pub last_activity_at: String,
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
    // NOTE: evaluator_cli/model intentionally omitted - /api/sessions/hive does not
    // support evaluator launches; use POST /api/sessions with with_evaluator=true instead.
    pub name: Option<String>,
    pub color: Option<String>,
}

#[derive(Deserialize)]
pub struct LaunchSwarmRequest {
    #[allow(dead_code)]
    pub issue_url: Option<String>,
    pub task_description: Option<String>,
    pub planner_count: Option<u8>,
    pub project_path: String,
    pub evaluator_cli: Option<String>,
    pub evaluator_model: Option<String>,
    pub qa_workers: Option<Vec<QaWorkerConfig>>,
    pub name: Option<String>,
    pub color: Option<String>,
}

#[derive(Deserialize)]
pub struct LaunchFusionVariantRequest {
    pub name: String,
    pub cli: Option<String>,
    pub model: Option<String>,
    pub flags: Option<Vec<String>>,
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
    pub name: Option<String>,
    pub color: Option<String>,
}

#[derive(Deserialize)]
pub struct LaunchSoloRequest {
    pub project_path: String,
    pub task_description: Option<String>,
    pub cli: String,
    pub model: Option<String>,
    pub flags: Option<Vec<String>>,
    pub evaluator_cli: Option<String>,
    pub evaluator_model: Option<String>,
    pub name: Option<String>,
    pub color: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub project_path: String,
    pub mode: String,
    pub objective: Option<String>,
    pub default_cli: Option<String>,
    pub default_model: Option<String>,
    pub worker_count: Option<u8>,
    pub variants: Option<Vec<LaunchFusionVariantRequest>>,
    pub judge_cli: Option<String>,
    pub judge_model: Option<String>,
    pub with_planning: Option<bool>,
    pub with_evaluator: Option<bool>,
    pub evaluator_cli: Option<String>,
    pub evaluator_model: Option<String>,
    pub qa_workers: Option<Vec<QaWorkerConfig>>,
    pub smoke_test: Option<bool>,
    pub name: Option<String>,
    pub color: Option<String>,
}

#[derive(Deserialize)]
pub struct LaunchSessionRequest {
    #[serde(rename = "mode")]
    pub _mode: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateSessionRequest {
    #[serde(default)]
    pub name: Option<Option<String>>,
    #[serde(default)]
    pub color: Option<Option<String>>,
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

/// POST /api/sessions - Create a session via the vNext API surface.
///
/// For now this immediately launches Hive/Fusion sessions using the existing controller
/// rather than creating a persisted draft session.
pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<LaunchResponse>), ApiError> {
    validate_project_path(&req.project_path)?;
    validate_session_name(req.name.as_deref())?;
    validate_session_color(req.color.as_deref())?;

    let mode = req.mode.trim().to_ascii_lowercase();
    let default_cli = req.default_cli.unwrap_or_else(|| "claude".to_string());
    validate_cli(&default_cli)?;

    match mode.as_str() {
        "hive" => {
            let queen_config = AgentConfig {
                cli: default_cli.clone(),
                model: req.default_model.clone(),
                flags: vec![],
                label: Some("Queen".to_string()),
                name: None,
                description: None,
                role: None,
                initial_prompt: None,
            };

            let worker_count = req.worker_count.unwrap_or(3);
            let worker_config = AgentConfig {
                cli: default_cli.clone(),
                model: req.default_model,
                flags: vec![],
                label: None,
                name: None,
                description: None,
                role: None,
                initial_prompt: None,
            };

            // Build evaluator_config: validate if provided, else fall back to default_cli silently
            let evaluator_config = if let Some(ref evaluator_cli) = req.evaluator_cli {
                validate_cli(evaluator_cli)?;
                Some(AgentConfig {
                    cli: evaluator_cli.clone(),
                    model: req.evaluator_model.clone(),
                    flags: vec![],
                    label: Some("Evaluator".to_string()),
                    name: None,
                    description: None,
                    role: None,
                    initial_prompt: None,
                })
            } else if req.with_evaluator.unwrap_or(false) {
                // Backward compat: if with_evaluator is true but no evaluator_cli, use default_cli silently
                Some(AgentConfig {
                    cli: default_cli,
                    model: req.evaluator_model.clone(),
                    flags: vec![],
                    label: Some("Evaluator".to_string()),
                    name: None,
                    description: None,
                    role: None,
                    initial_prompt: None,
                })
            } else {
                None
            };

            let config = HiveLaunchConfig {
                project_path: req.project_path,
                name: req.name,
                color: req.color,
                queen_config,
                workers: vec![worker_config; worker_count as usize],
                prompt: req.objective.filter(|value| !value.trim().is_empty()),
                with_planning: req.with_planning.unwrap_or(false),
                with_evaluator: req.with_evaluator.unwrap_or(false),
                evaluator_config,
                qa_workers: req.qa_workers,
                smoke_test: req.smoke_test.unwrap_or(false),
            };

            let controller = state.session_controller.write();
            let session = controller
                .launch_hive_v2(config)
                .map_err(ApiError::internal)?;

            Ok((
                StatusCode::CREATED,
                Json(LaunchResponse {
                    session_id: session.id,
                    message: "Session created".to_string(),
                }),
            ))
        }
        "fusion" => {
            let variants = req
                .variants
                .filter(|variants| !variants.is_empty())
                .ok_or_else(|| ApiError::bad_request("Fusion sessions require at least one variant"))?
                .into_iter()
                .map(|variant| {
                    let cli = variant.cli.unwrap_or_else(|| default_cli.clone());
                    validate_cli(&cli)?;
                    Ok(FusionVariantConfig {
                        name: variant.name,
                        cli,
                        model: variant.model,
                        flags: variant.flags.unwrap_or_default(),
                    })
                })
                .collect::<Result<Vec<_>, ApiError>>()?;

            let task_description = req
                .objective
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| ApiError::bad_request("Fusion sessions require a non-empty objective"))?;

            let judge_cli = req.judge_cli.unwrap_or_else(|| default_cli.clone());
            validate_cli(&judge_cli)?;

            let config = FusionLaunchConfig {
                project_path: req.project_path,
                name: req.name,
                color: req.color,
                variants,
                task_description,
                judge_config: AgentConfig {
                    cli: judge_cli,
                    model: req.judge_model.or(req.default_model.clone()),
                    flags: vec![],
                    label: Some("Fusion Judge".to_string()),
                    name: None,
                    description: None,
                    role: None,
                    initial_prompt: None,
                },
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
                    message: "Session created".to_string(),
                }),
            ))
        }
        _ => Err(ApiError::bad_request(
            "Unsupported mode. Valid options: hive, fusion",
        )),
    }
}

/// POST /api/sessions/{id}/launch - Launch a previously created draft session.
pub async fn launch_session(
    Path(id): Path<String>,
    Json(_req): Json<LaunchSessionRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    validate_session_id(&id)?;

    Err(ApiError::internal(
        "Session draft launch is not yet implemented",
    ))
}

/// GET /api/sessions - List all sessions
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SessionListResponse>, ApiError> {
    let persisted = state.storage.list_sessions()
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let active_sessions = state.session_controller.read().list_sessions();
    let mut sessions = persisted
        .into_iter()
        .map(|s| {
            (
                s.id.clone(),
                SessionInfo {
                    id: s.id,
                    name: s.name,
                    color: s.color,
                    session_type: s.session_type,
                    status: s.state,
                    project_path: s.project_path,
                    created_at: s.created_at.to_rfc3339(),
                    last_activity_at: s.last_activity_at.to_rfc3339(),
                },
            )
        })
        .collect::<std::collections::HashMap<_, _>>();

    for session in active_sessions {
        sessions.insert(
            session.id.clone(),
            SessionInfo {
                id: session.id.clone(),
                name: session.name.clone(),
                color: session.color.clone(),
                session_type: match &session.session_type {
                    crate::session::SessionType::Hive { worker_count } => {
                        format!("Hive ({})", worker_count)
                    }
                    crate::session::SessionType::Swarm { planner_count } => {
                        format!("Swarm ({})", planner_count)
                    }
                    crate::session::SessionType::Fusion { variants } => {
                        format!("Fusion ({})", variants.len())
                    }
                    crate::session::SessionType::Solo { cli, .. } => format!("Solo ({})", cli),
                },
                status: format!("{:?}", session.state),
                project_path: session.project_path.to_string_lossy().to_string(),
                created_at: session.created_at.to_rfc3339(),
                last_activity_at: session.last_activity_at.to_rfc3339(),
            },
        );
    }

    let mut sessions = sessions.into_values().collect::<Vec<_>>();
    sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

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
            name: session.name.clone(),
            color: session.color.clone(),
            session_type: match &session.session_type {
                crate::session::SessionType::Hive { worker_count } => format!("Hive ({})", worker_count),
                crate::session::SessionType::Swarm { planner_count } => format!("Swarm ({})", planner_count),
                crate::session::SessionType::Fusion { variants } => format!("Fusion ({})", variants.len()),
                crate::session::SessionType::Solo { cli, .. } => format!("Solo ({})", cli),
            },
            status: format!("{:?}", session.state),
            project_path: session.project_path.to_string_lossy().to_string(),
            created_at: session.created_at.to_rfc3339(),
            last_activity_at: session.last_activity_at.to_rfc3339(),
        }));
    }

    // Try loading from storage if not active
    let persisted = state.storage.load_session(&id)
        .map_err(|_| ApiError::not_found(format!("Session {} not found", id)))?;

    Ok(Json(SessionInfo {
        id: persisted.id,
        name: persisted.name,
        color: persisted.color,
        session_type: match &persisted.session_type {
            SessionTypeInfo::Hive { worker_count } => format!("Hive ({})", worker_count),
            SessionTypeInfo::Swarm { planner_count } => format!("Swarm ({})", planner_count),
            SessionTypeInfo::Fusion { variants } => format!("Fusion ({})", variants.len()),
            SessionTypeInfo::Solo { cli, .. } => format!("Solo ({})", cli),
        },
        status: persisted.state,
        project_path: persisted.project_path,
        created_at: persisted.created_at.to_rfc3339(),
        last_activity_at: persisted
            .last_activity_at
            .unwrap_or(persisted.created_at)
            .to_rfc3339(),
    }))
}

/// POST /api/sessions/hive - Launch a new Hive session
pub async fn launch_hive(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LaunchHiveRequest>,
) -> Result<(StatusCode, Json<LaunchResponse>), ApiError> {
    validate_session_name(req.name.as_deref())?;
    validate_session_color(req.color.as_deref())?;

    let controller = state.session_controller.write();
    let project_path = std::path::PathBuf::from(req.project_path);

    let command = req.command.unwrap_or_else(|| "claude".to_string());
    validate_cli(&command)?;

    let session = controller.launch_hive(
        project_path,
        req.worker_count.unwrap_or(3),
        &command,
        req.task_description,
        req.name,
        req.color,
    ).map_err(ApiError::internal)?;

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
    validate_session_name(req.name.as_deref())?;
    validate_session_color(req.color.as_deref())?;

    let controller = state.session_controller.write();

    let default_cli = "claude".to_string();
    let default_config = AgentConfig {
        cli: default_cli.clone(),
        model: None,
        flags: vec![],
        label: None,
        name: None,
        description: None,
        role: None,
        initial_prompt: None,
    };

    // Build evaluator_config: validate if provided, else fall back to default_cli silently
    let evaluator_config = if let Some(ref evaluator_cli) = req.evaluator_cli {
        validate_cli(evaluator_cli)?;
        Some(AgentConfig {
            cli: evaluator_cli.clone(),
            model: req.evaluator_model.clone(),
            flags: vec![],
            label: Some("Evaluator".to_string()),
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        })
    } else {
        None
    };
    let with_evaluator = evaluator_config.is_some();

    let config = crate::session::SwarmLaunchConfig {
        project_path: req.project_path,
        name: req.name,
        color: req.color,
        queen_config: default_config.clone(),
        planner_count: req.planner_count.unwrap_or(2),
        planner_config: default_config.clone(),
        workers_per_planner: vec![default_config.clone(); 2],
        prompt: req.task_description,
        with_planning: false,
        with_evaluator,
        evaluator_config,
        qa_workers: req.qa_workers,
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

/// POST /api/sessions/solo - Launch a new solo session
pub async fn launch_solo(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LaunchSoloRequest>,
) -> Result<(StatusCode, Json<LaunchResponse>), ApiError> {
    validate_project_path(&req.project_path)?;
    validate_cli(&req.cli)?;
    validate_session_name(req.name.as_deref())?;
    validate_session_color(req.color.as_deref())?;

    let agent_config = AgentConfig {
        cli: req.cli.clone(),
        model: req.model,
        flags: req.flags.unwrap_or_default(),
        label: None,
        name: None,
        description: None,
        role: None,
        initial_prompt: None,
    };

    // Build evaluator_config: validate if provided, else fall back to req.cli silently
    let evaluator_config = if let Some(ref evaluator_cli) = req.evaluator_cli {
        validate_cli(evaluator_cli)?;
        Some(AgentConfig {
            cli: evaluator_cli.clone(),
            model: req.evaluator_model.clone(),
            flags: vec![],
            label: Some("Evaluator".to_string()),
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        })
    } else {
        None
    };
    let with_evaluator = evaluator_config.is_some();

    let config = HiveLaunchConfig {
        project_path: req.project_path,
        name: req.name,
        color: req.color,
        queen_config: agent_config,
        workers: vec![],
        prompt: req.task_description.filter(|t| !t.trim().is_empty()),
        with_planning: false,
        with_evaluator,
        evaluator_config,
        qa_workers: None,
        smoke_test: false,
    };

    let controller = state.session_controller.write();
    let session = controller
        .launch_solo(config)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(LaunchResponse {
            session_id: session.id,
            message: "Solo session launched".to_string(),
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
    validate_session_name(req.name.as_deref())?;
    validate_session_color(req.color.as_deref())?;

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
                flags: v.flags.unwrap_or_default(),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;

    let judge_config = AgentConfig {
        cli: judge_cli,
        model: req.judge_model.or(req.default_model.clone()),
        flags: vec![],
        label: Some("Fusion Judge".to_string()),
        name: None,
        description: None,
        role: None,
        initial_prompt: None,
    };

    let config = FusionLaunchConfig {
        project_path: req.project_path,
        name: req.name,
        color: req.color,
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

/// PATCH /api/sessions/{id} - Update session metadata
pub async fn update_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateSessionRequest>,
) -> Result<Json<SessionInfo>, ApiError> {
    validate_session_id(&id)?;
    validate_session_name(req.name.as_ref().and_then(|name| name.as_deref()))?;
    validate_session_color(req.color.as_ref().and_then(|color| color.as_deref()))?;

    let controller = state.session_controller.write();
    let session = controller
        .update_session_metadata(&id, req.name, req.color)
        .map_err(|e| {
            if e.starts_with("Session not found") {
                ApiError::not_found(e)
            } else {
                ApiError::internal(e)
            }
        })?;

    Ok(Json(SessionInfo {
        id: session.id,
        name: session.name,
        color: session.color,
        session_type: match &session.session_type {
            crate::session::SessionType::Hive { worker_count } => format!("Hive ({})", worker_count),
            crate::session::SessionType::Swarm { planner_count } => format!("Swarm ({})", planner_count),
            crate::session::SessionType::Fusion { variants } => format!("Fusion ({})", variants.len()),
            crate::session::SessionType::Solo { cli, .. } => format!("Solo ({})", cli),
        },
        status: format!("{:?}", session.state),
        project_path: session.project_path.to_string_lossy().to_string(),
        created_at: session.created_at.to_rfc3339(),
        last_activity_at: session.last_activity_at.to_rfc3339(),
    }))
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

/// POST /api/sessions/{id}/close - Close a session
pub async fn close_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_session_id(&id)?;

    let controller = state.session_controller.write();
    controller.close_session(&id)
        .map_err(|e| {
            if e.starts_with("Session not found") {
                ApiError::not_found(e)
            } else {
                ApiError::internal(e)
            }
        })?;

    Ok(Json(serde_json::json!({
        "message": format!("Session {} closed", id)
    })))
}

/// POST /api/sessions/{id}/complete - Mark a session as completed
pub async fn complete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_session_id(&id)?;

    state
        .session_controller
        .read()
        .mark_session_completed(&id)
        .map_err(|e: crate::session::CompletionBlockedError| {
            if e.error.starts_with("Session not found") {
                ApiError::not_found(&e.error)
            } else {
                // Return structured 409 response with unblock paths
                let mut details = std::collections::HashMap::new();
                details.insert("current_state".to_string(), serde_json::json!(e.current_state));
                details.insert("unblock_paths".to_string(), serde_json::json!(e.unblock_paths));
                details.insert("remaining_quiescence_seconds".to_string(), serde_json::json!(e.remaining_quiescence_seconds));
                ApiError::conflict_with_details(&e.error, details)
            }
        })?;

    Ok(Json(serde_json::json!({
        "message": format!("Session {} marked completed", id)
    })))
}
