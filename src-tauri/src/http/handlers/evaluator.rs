use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::pty::{AgentConfig, AgentRole};
use crate::session::{AuthStrategy, SessionController, SessionState};

use super::validate_session_id;
// validate_cli used by add_evaluator
use super::validate_cli;

#[derive(Debug, Clone, Deserialize)]
pub struct AddEvaluatorRequest {
    pub label: Option<String>,
    pub cli: Option<String>,
    pub model: Option<String>,
    pub initial_task: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddEvaluatorResponse {
    pub evaluator_id: String,
    pub cli: String,
    pub status: String,
    pub prompt_file: String,
}

pub async fn add_evaluator(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<AddEvaluatorRequest>,
) -> Result<(StatusCode, Json<AddEvaluatorResponse>), ApiError> {
    validate_session_id(&session_id)?;

    let session_default_cli = {
        let controller = state.session_controller.read();
        controller
            .get_session_default_cli(&session_id)
            .unwrap_or_else(|| "claude".to_string())
    };
    let cli = req.cli.unwrap_or(session_default_cli);
    validate_cli(&cli)?;

    let config = AgentConfig {
        cli: cli.clone(),
        model: req.model,
        flags: vec![],
        label: req.label.clone().or_else(|| Some("Evaluator".to_string())),
        role: None,
        initial_prompt: req.initial_task,
    };

    let evaluator_id = {
        let controller = state.session_controller.write();
        controller
            .launch_evaluator(&session_id, config)
            .map_err(ApiError::internal)?
            .id
    };

    Ok((
        StatusCode::CREATED,
        Json(AddEvaluatorResponse {
            evaluator_id,
            cli,
            status: "Running".to_string(),
            prompt_file: format!(".hive-manager/{}/prompts/evaluator-prompt.md", session_id),
        }),
    ))
}

pub async fn list_evaluators(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let controller = state.session_controller.read();
    let session = controller
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    let evaluators: Vec<Value> = session
        .agents
        .iter()
        .filter(|agent| matches!(agent.role, AgentRole::Evaluator))
        .map(|agent| {
            json!({
                "id": agent.id,
                "cli": agent.config.cli,
                "label": agent.config.label,
                "status": format!("{:?}", agent.status),
                "prompt_file": format!(".hive-manager/{}/prompts/evaluator-prompt.md", session_id),
            })
        })
        .collect();

    Ok(Json(json!({
        "session_id": session_id,
        "evaluators": evaluators,
        "count": evaluators.len()
    })))
}

// --- Dev Login Endpoint ---

#[derive(Debug, Deserialize)]
pub struct DevLoginQuery {
    pub token: String,
}

pub async fn dev_login(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<DevLoginQuery>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let controller = state.session_controller.read();
    let session = controller
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    match &session.auth_strategy {
        AuthStrategy::DevBypass { token } if *token == query.token => {
            Ok(Json(json!({
                "session_id": session_id,
                "auth": "dev-bypass",
                "granted": true
            })))
        }
        AuthStrategy::DevBypass { .. } => {
            Err(ApiError::new(StatusCode::UNAUTHORIZED, "Invalid dev-bypass token"))
        }
        AuthStrategy::None => {
            Err(ApiError::not_found("Auth not configured for this session"))
        }
    }
}

// --- Force Pass / Force Fail Endpoints ---

fn require_qa_in_progress(
    controller: &SessionController,
    session_id: &str,
    action: &str,
) -> Result<(), ApiError> {
    let session = controller
        .get_session(session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    if matches!(session.state, SessionState::QaInProgress) {
        return Ok(());
    }

    Err(ApiError::bad_request(format!(
        "Cannot {}: session is in {:?} state, expected QaInProgress",
        action, session.state
    )))
}

fn append_operator_log(state: &AppState, session_id: &str, action: &str, detail: &str) {
    let msg = crate::coordination::CoordinationMessage::system(
        "OPERATOR",
        &format!("[{}] Operator forced {} for session {}", action, detail, session_id),
    );
    let _ = state.storage.append_coordination_log(session_id, &msg);
}

pub async fn force_pass(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let controller = state.session_controller.read();
    require_qa_in_progress(&controller, &session_id, "force-pass")?;
    controller
        .on_qa_verdict(&session_id, "QA_VERDICT: PASS")
        .map_err(ApiError::internal)?;
    drop(controller);

    append_operator_log(&state, &session_id, "FORCE-PASS", "QA pass");

    Ok(Json(json!({
        "session_id": session_id,
        "action": "force-pass",
        "new_state": "Running"
    })))
}

pub async fn force_fail(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let controller = state.session_controller.read();
    require_qa_in_progress(&controller, &session_id, "force-fail")?;
    let new_state = controller
        .on_qa_verdict(&session_id, "QA_VERDICT: FAIL")
        .map_err(ApiError::internal)?;
    drop(controller);

    append_operator_log(&state, &session_id, "FORCE-FAIL", "QA fail");

    Ok(Json(json!({
        "session_id": session_id,
        "action": "force-fail",
        "new_state": format!("{:?}", new_state)
    })))
}
