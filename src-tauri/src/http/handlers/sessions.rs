use crate::actions::{ActionContext, Caller};
use crate::cli::CliRegistry;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use super::{validate_cli, validate_session_id};
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::pty::AgentConfig;
use crate::session::{
    CompletionBlockedError, CompletionError, DebateDebaterConfig, DebateDebaterStatus,
    DebateLaunchConfig, FusionLaunchConfig, FusionVariantConfig, FusionVariantStatus,
    HiveLaunchConfig, QaWorkerConfig,
};

async fn dispatch_session_action(
    state: &Arc<AppState>,
    action: &str,
    input: Value,
) -> Result<Value, ApiError> {
    let ctx = ActionContext::new(Caller::Http, Arc::clone(state));
    state
        .registry()
        .dispatch(action, &ctx, input)
        .await
        .map_err(ApiError::from)
}

fn decode_action_output<T: DeserializeOwned>(action: &str, value: Value) -> Result<T, ApiError> {
    serde_json::from_value(value).map_err(|e| {
        ApiError::internal(format!(
            "Action {} returned an unexpected payload: {}",
            action, e
        ))
    })
}

fn launch_response_from_action_output(
    value: &Value,
    message: &str,
) -> Result<LaunchResponse, ApiError> {
    let session_id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::internal("Launch action returned a session without an id"))?
        .to_string();
    Ok(LaunchResponse {
        session_id,
        message: message.to_string(),
    })
}

fn evaluator_config_from_request(
    evaluator_config: Option<AgentConfig>,
    evaluator_cli: Option<String>,
    evaluator_model: Option<String>,
    default_cli: Option<&str>,
) -> Result<Option<AgentConfig>, ApiError> {
    if let Some(mut config) = evaluator_config {
        validate_cli(&config.cli)?;
        if config.label.is_none() {
            config.label = Some("Evaluator".to_string());
        }
        return Ok(Some(config));
    }

    if let Some(evaluator_cli) = evaluator_cli {
        validate_cli(&evaluator_cli)?;
        return Ok(Some(AgentConfig {
            cli: evaluator_cli,
            model: evaluator_model,
            flags: vec![],
            label: Some("Evaluator".to_string()),
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        }));
    }

    if let Some(default_cli) = default_cli {
        validate_cli(default_cli)?;
        return Ok(Some(AgentConfig {
            cli: default_cli.to_string(),
            model: evaluator_model,
            flags: vec![],
            label: Some("Evaluator".to_string()),
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        }));
    }

    Ok(None)
}

fn completion_blocked_to_api_error(error: CompletionBlockedError) -> ApiError {
    let mut details = std::collections::HashMap::new();
    details.insert(
        "current_state".to_string(),
        serde_json::json!(error.current_state),
    );
    details.insert(
        "unblock_paths".to_string(),
        serde_json::json!(error.unblock_paths),
    );
    details.insert(
        "remaining_quiescence_seconds".to_string(),
        serde_json::json!(error.remaining_quiescence_seconds),
    );
    ApiError::conflict_with_details(error.error, details)
}

fn map_completion_error(error: CompletionError) -> ApiError {
    match error {
        CompletionError::Blocked(blocked) => completion_blocked_to_api_error(blocked),
        CompletionError::NotFound(message) => ApiError::not_found(message),
        CompletionError::Storage(message) => ApiError::internal(message),
    }
}

#[derive(Serialize, Deserialize)]
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
    pub default_cli: Option<String>,
    pub default_model: Option<String>,
    pub queen_config: Option<AgentConfig>,
    pub planner_config: Option<AgentConfig>,
    pub workers_per_planner: Option<Vec<AgentConfig>>,
    pub evaluator_config: Option<AgentConfig>,
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
pub struct LaunchDebateDebaterRequest {
    pub name: String,
    pub stance: Option<String>,
    pub cli: Option<String>,
    pub model: Option<String>,
    pub flags: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct LaunchDebateRequest {
    pub project_path: String,
    pub topic: String,
    pub rounds: Option<u8>,
    pub debaters: Vec<LaunchDebateDebaterRequest>,
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
    pub evaluator_config: Option<AgentConfig>,
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
    pub workers: Option<Vec<AgentConfig>>,
    pub variants: Option<Vec<LaunchFusionVariantRequest>>,
    pub debaters: Option<Vec<LaunchDebateDebaterRequest>>,
    pub rounds: Option<u8>,
    pub judge_cli: Option<String>,
    pub judge_model: Option<String>,
    pub with_planning: Option<bool>,
    pub with_evaluator: Option<bool>,
    pub evaluator_config: Option<AgentConfig>,
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

#[derive(Serialize)]
pub struct DebateStatusResponse {
    pub session_id: String,
    pub state: String,
    pub debaters: Vec<DebateDebaterStatus>,
}

#[derive(Serialize)]
pub struct DebateEvaluationResponse {
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
            let workers = if let Some(workers) = req.workers {
                if workers.is_empty() {
                    return Err(ApiError::bad_request(
                        "Hive launch requires at least one worker",
                    ));
                }
                for worker in &workers {
                    validate_cli(&worker.cli)?;
                }
                workers
            } else {
                vec![worker_config; worker_count as usize]
            };

            let evaluator_config = evaluator_config_from_request(
                req.evaluator_config,
                req.evaluator_cli,
                req.evaluator_model,
                req.with_evaluator
                    .unwrap_or(false)
                    .then_some(default_cli.as_str()),
            )?;
            let with_evaluator = req.with_evaluator.unwrap_or(false) || evaluator_config.is_some();

            let config = HiveLaunchConfig {
                project_path: req.project_path,
                name: req.name,
                color: req.color,
                queen_config,
                workers,
                prompt: req.objective.filter(|value| !value.trim().is_empty()),
                with_planning: req.with_planning.unwrap_or(false),
                with_evaluator,
                evaluator_config,
                qa_workers: req.qa_workers,
                smoke_test: req.smoke_test.unwrap_or(false),
            };

            let output = dispatch_session_action(
                &state,
                "session.launch_hive_v2",
                serde_json::to_value(config).map_err(|e| {
                    ApiError::internal(format!("Failed to serialize launch config: {}", e))
                })?,
            )
            .await?;

            Ok((
                StatusCode::CREATED,
                Json(launch_response_from_action_output(
                    &output,
                    "Session created",
                )?),
            ))
        }
        "fusion" => {
            let variants = req
                .variants
                .filter(|variants| !variants.is_empty())
                .ok_or_else(|| {
                    ApiError::bad_request("Fusion sessions require at least one variant")
                })?
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
                .ok_or_else(|| {
                    ApiError::bad_request("Fusion sessions require a non-empty objective")
                })?;

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

            let output = dispatch_session_action(
                &state,
                "session.launch_fusion",
                serde_json::to_value(config).map_err(|e| {
                    ApiError::internal(format!("Failed to serialize launch config: {}", e))
                })?,
            )
            .await?;

            Ok((
                StatusCode::CREATED,
                Json(launch_response_from_action_output(
                    &output,
                    "Session created",
                )?),
            ))
        }
        "debate" => {
            let debaters = req
                .debaters
                .filter(|debaters| !debaters.is_empty())
                .ok_or_else(|| {
                    ApiError::bad_request("Debate sessions require at least one debater")
                })?
                .into_iter()
                .map(|debater| {
                    let cli = debater.cli.unwrap_or_else(|| default_cli.clone());
                    validate_cli(&cli)?;
                    Ok(DebateDebaterConfig {
                        name: debater.name,
                        stance: debater.stance,
                        cli,
                        model: debater.model,
                        flags: debater.flags.unwrap_or_default(),
                    })
                })
                .collect::<Result<Vec<_>, ApiError>>()?;

            let topic = req
                .objective
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    ApiError::bad_request("Debate sessions require a non-empty objective")
                })?;

            let rounds = req.rounds.unwrap_or(3);
            if rounds == 0 {
                return Err(ApiError::bad_request(
                    "Debate sessions require at least one round",
                ));
            }

            let judge_cli = req.judge_cli.unwrap_or_else(|| default_cli.clone());
            validate_cli(&judge_cli)?;

            let config = DebateLaunchConfig {
                project_path: req.project_path,
                name: req.name,
                color: req.color,
                debaters,
                topic,
                rounds,
                judge_config: AgentConfig {
                    cli: judge_cli,
                    model: req.judge_model.or(req.default_model.clone()),
                    flags: vec![],
                    label: Some("Debate Judge".to_string()),
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

            let output = dispatch_session_action(
                &state,
                "session.launch_debate",
                serde_json::to_value(config).map_err(|e| {
                    ApiError::internal(format!("Failed to serialize launch config: {}", e))
                })?,
            )
            .await?;

            Ok((
                StatusCode::CREATED,
                Json(launch_response_from_action_output(
                    &output,
                    "Session created",
                )?),
            ))
        }
        _ => Err(ApiError::bad_request(
            "Unsupported mode. Valid options: hive, fusion, debate",
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
    let persisted = state
        .storage
        .list_sessions()
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
                    crate::session::SessionType::Debate { variants } => {
                        format!("Debate ({})", variants.len())
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
    let output =
        dispatch_session_action(&state, "session.get_info", serde_json::json!({ "id": id }))
            .await?;
    Ok(Json(decode_action_output("session.get_info", output)?))
}

/// POST /api/sessions/hive - Launch a new Hive session
pub async fn launch_hive(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LaunchHiveRequest>,
) -> Result<(StatusCode, Json<LaunchResponse>), ApiError> {
    let output = dispatch_session_action(
        &state,
        "session.launch_hive",
        serde_json::json!({
            "project_path": req.project_path,
            "task_description": req.task_description,
            "worker_count": req.worker_count,
            "command": req.command,
            "name": req.name,
            "color": req.color,
        }),
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(launch_response_from_action_output(
            &output,
            "Hive session launched",
        )?),
    ))
}

/// POST /api/sessions/swarm - Launch a new Swarm session
pub async fn launch_swarm(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LaunchSwarmRequest>,
) -> Result<(StatusCode, Json<LaunchResponse>), ApiError> {
    let default_cli = req.default_cli.unwrap_or_else(|| "claude".to_string());
    validate_cli(&default_cli)?;
    let default_model = req
        .default_model
        .or_else(|| CliRegistry::default_model(&default_cli).map(str::to_string));
    let default_config = AgentConfig {
        cli: default_cli.clone(),
        model: default_model.clone(),
        flags: vec![],
        label: None,
        name: None,
        description: None,
        role: None,
        initial_prompt: None,
    };
    let queen_config = req.queen_config.unwrap_or_else(|| default_config.clone());
    validate_cli(&queen_config.cli)?;
    let planner_config = req.planner_config.unwrap_or_else(|| default_config.clone());
    validate_cli(&planner_config.cli)?;
    let workers_per_planner = match req.workers_per_planner {
        Some(workers) if workers.is_empty() => {
            return Err(ApiError::bad_request("workers_per_planner cannot be empty"));
        }
        Some(workers) => workers,
        None => vec![default_config.clone(); 2],
    };
    for worker in &workers_per_planner {
        validate_cli(&worker.cli)?;
    }

    let evaluator_config = evaluator_config_from_request(
        req.evaluator_config,
        req.evaluator_cli,
        req.evaluator_model,
        None,
    )?;
    let with_evaluator = evaluator_config.is_some();

    let config = crate::session::SwarmLaunchConfig {
        project_path: req.project_path,
        name: req.name,
        color: req.color,
        queen_config,
        planner_count: req.planner_count.unwrap_or(2),
        planner_config,
        workers_per_planner,
        prompt: req.task_description,
        with_planning: false,
        with_evaluator,
        evaluator_config,
        qa_workers: req.qa_workers,
        smoke_test: false,
        planners: vec![],
    };

    let output = dispatch_session_action(
        &state,
        "session.launch_swarm",
        serde_json::to_value(config)
            .map_err(|e| ApiError::internal(format!("Failed to serialize launch config: {}", e)))?,
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(launch_response_from_action_output(
            &output,
            "Swarm session launched",
        )?),
    ))
}

/// POST /api/sessions/solo - Launch a new solo session
pub async fn launch_solo(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LaunchSoloRequest>,
) -> Result<(StatusCode, Json<LaunchResponse>), ApiError> {
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

    let evaluator_config = evaluator_config_from_request(
        req.evaluator_config,
        req.evaluator_cli,
        req.evaluator_model,
        None,
    )?;
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

    let output = dispatch_session_action(
        &state,
        "session.launch_solo",
        serde_json::to_value(config)
            .map_err(|e| ApiError::internal(format!("Failed to serialize launch config: {}", e)))?,
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(launch_response_from_action_output(
            &output,
            "Solo session launched",
        )?),
    ))
}

/// POST /api/sessions/fusion - Launch a new Fusion session
pub async fn launch_fusion(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LaunchFusionRequest>,
) -> Result<(StatusCode, Json<LaunchResponse>), ApiError> {
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

    let output = dispatch_session_action(
        &state,
        "session.launch_fusion",
        serde_json::to_value(config)
            .map_err(|e| ApiError::internal(format!("Failed to serialize launch config: {}", e)))?,
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(launch_response_from_action_output(
            &output,
            "Fusion session launched",
        )?),
    ))
}

/// POST /api/sessions/debate - Launch a new Debate session
pub async fn launch_debate(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LaunchDebateRequest>,
) -> Result<(StatusCode, Json<LaunchResponse>), ApiError> {
    let default_cli = req.default_cli.unwrap_or_else(|| "claude".to_string());
    validate_cli(&default_cli)?;

    let judge_cli = req.judge_cli.unwrap_or_else(|| default_cli.clone());
    validate_cli(&judge_cli)?;

    let rounds = req.rounds.unwrap_or(3);

    let debaters = req
        .debaters
        .into_iter()
        .map(|d| {
            let cli = d.cli.unwrap_or_else(|| default_cli.clone());
            validate_cli(&cli)?;
            Ok(DebateDebaterConfig {
                name: d.name,
                stance: d.stance,
                cli,
                model: d.model,
                flags: d.flags.unwrap_or_default(),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;

    let judge_config = AgentConfig {
        cli: judge_cli,
        model: req.judge_model.or(req.default_model.clone()),
        flags: vec![],
        label: Some("Debate Judge".to_string()),
        name: None,
        description: None,
        role: None,
        initial_prompt: None,
    };

    let config = DebateLaunchConfig {
        project_path: req.project_path,
        name: req.name,
        color: req.color,
        debaters,
        topic: req.topic,
        rounds,
        judge_config,
        queen_config: None,
        with_planning: req.with_planning.unwrap_or(false),
        default_cli,
        default_model: req.default_model,
    };

    let output = dispatch_session_action(
        &state,
        "session.launch_debate",
        serde_json::to_value(config)
            .map_err(|e| ApiError::internal(format!("Failed to serialize launch config: {}", e)))?,
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(launch_response_from_action_output(
            &output,
            "Debate session launched",
        )?),
    ))
}

/// PATCH /api/sessions/{id} - Update session metadata
pub async fn update_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateSessionRequest>,
) -> Result<Json<SessionInfo>, ApiError> {
    let output = dispatch_session_action(
        &state,
        "session.update_metadata_info",
        serde_json::json!({ "id": id, "name": req.name, "color": req.color }),
    )
    .await?;
    Ok(Json(decode_action_output(
        "session.update_metadata_info",
        output,
    )?))
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

/// GET /api/sessions/{id}/debate/status - Get debate debater statuses
pub async fn get_debate_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<DebateStatusResponse>, ApiError> {
    validate_session_id(&id)?;

    let controller = state.session_controller.read();
    if controller.get_session(&id).is_none() {
        return Err(ApiError::not_found(format!("Session {} not found", id)));
    }

    let debaters = controller
        .get_debate_debater_statuses(&id)
        .map_err(ApiError::internal)?;
    let state_str = controller
        .get_session(&id)
        .map(|s| format!("{:?}", s.state))
        .unwrap_or_else(|| "Unknown".to_string());

    Ok(Json(DebateStatusResponse {
        session_id: id,
        state: state_str,
        debaters,
    }))
}

/// GET /api/sessions/{id}/debate/evaluation - Get debate judge verdict
pub async fn get_debate_evaluation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<DebateEvaluationResponse>, ApiError> {
    validate_session_id(&id)?;

    let controller = state.session_controller.read();
    if controller.get_session(&id).is_none() {
        return Err(ApiError::not_found(format!("Session {} not found", id)));
    }

    let (report_path, report) = controller
        .get_debate_evaluation(&id)
        .map_err(ApiError::internal)?;
    let state_str = controller
        .get_session(&id)
        .map(|s| format!("{:?}", s.state))
        .unwrap_or_else(|| "Unknown".to_string());

    Ok(Json(DebateEvaluationResponse {
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
    let output =
        dispatch_session_action(&state, "session.stop", serde_json::json!({ "id": id })).await?;
    Ok(Json(output))
}

/// POST /api/sessions/{id}/close - Close a session
pub async fn close_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let output =
        dispatch_session_action(&state, "session.close", serde_json::json!({ "id": id })).await?;
    Ok(Json(output))
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
        .map_err(map_completion_error)?;

    Ok(Json(serde_json::json!({
        "message": format!("Session {} marked completed", id)
    })))
}

/// Response body for the run-journal endpoint.
#[derive(Debug, Serialize)]
pub struct RunJournalResponse {
    pub journal: Vec<crate::domain::run_journal::RunJournalEntry>,
    pub ledger: Vec<crate::domain::run_journal::LedgerEntry>,
}

/// GET /api/sessions/{id}/run-journal — the per-step journal + side-effect ledger for a
/// run (#125). Read-only; backed by the shared SQLite DB via `RunJournalStore`. The
/// synchronous rusqlite calls run inside `spawn_blocking` so the connection mutex is
/// never held across an `.await`.
pub async fn get_run_journal(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<RunJournalResponse>, ApiError> {
    validate_session_id(&id)?;

    let store = crate::storage::RunJournalStore::new(Arc::clone(&state.app_state_db));
    let response = tokio::task::spawn_blocking(
        move || -> Result<RunJournalResponse, crate::storage::StorageError> {
            // Defensive: ensure the journal/ledger tables exist (idempotent CREATE TABLE IF NOT
            // EXISTS). Production creates them at startup, but a fresh/in-memory DB (e.g. tests)
            // may not have run that path, in which case read_journal would hit "no such table".
            store.ensure_schema()?;
            let journal = store.read_journal(&id)?;
            let ledger = store.read_ledger(&id)?;
            Ok(RunJournalResponse { journal, ledger })
        },
    )
    .await
    .map_err(|e| ApiError::internal(format!("Task join error: {e}")))?
    .map_err(|e| ApiError::internal(format!("Failed to read run journal: {e}")))?;

    Ok(Json(response))
}
