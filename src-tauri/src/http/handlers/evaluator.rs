use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::coordination::{CoordinationMessage, StateManager};
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

#[derive(Debug, Clone, Deserialize)]
pub struct AddQaWorkerRequest {
    pub specialization: String,
    pub label: Option<String>,
    pub cli: Option<String>,
    pub model: Option<String>,
    pub initial_task: Option<String>,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddQaWorkerResponse {
    pub worker_id: String,
    pub role: String,
    pub cli: String,
    pub status: String,
    pub task_file: String,
}

fn validate_qa_specialization(specialization: &str) -> Result<(), ApiError> {
    if matches!(specialization, "ui" | "api" | "a11y") {
        return Ok(());
    }

    Err(ApiError::bad_request(format!(
        "Invalid QA specialization '{}'. Valid options: ui, api, a11y",
        specialization
    )))
}

fn qa_specialization_label(specialization: &str) -> &'static str {
    match specialization {
        "ui" => "UI QA",
        "api" => "API QA",
        "a11y" => "A11Y QA",
        _ => "QA Worker",
    }
}

fn map_add_qa_worker_error(error: String) -> ApiError {
    if error.contains("Session not found") {
        return ApiError::not_found(error);
    }

    if error.contains("Evaluator") && error.contains("not found for session") {
        return ApiError::bad_request(error);
    }

    if error.contains("is not an Evaluator") {
        return ApiError::bad_request(error);
    }

    if error.starts_with("Cannot add")
        || error.starts_with("Invalid")
        || error.contains("expected")
        || error.contains("precondition")
    {
        return ApiError::bad_request(error);
    }

    ApiError::internal(error)
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
        name: None,
        description: None,
        role: None,
        initial_prompt: req.initial_task,
    };

    let evaluator_id = {
        let controller = state.session_controller.write();
        controller
            .launch_evaluator(&session_id, config, false)
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

pub async fn add_qa_worker(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<AddQaWorkerRequest>,
) -> Result<(StatusCode, Json<AddQaWorkerResponse>), ApiError> {
    validate_session_id(&session_id)?;
    validate_qa_specialization(&req.specialization)?;

    let session_default_cli = {
        let controller = state.session_controller.read();
        controller
            .get_session_default_cli(&session_id)
            .unwrap_or_else(|| "claude".to_string())
    };
    let cli = req.cli.unwrap_or(session_default_cli);
    validate_cli(&cli)?;

    let mut flags = Vec::new();
    // Auto-inject --chrome for UI QA workers using claude CLI
    if req.specialization == "ui" && cli == "claude" {
        flags.push("--chrome".to_string());
    }

    let config = AgentConfig {
        cli: cli.clone(),
        model: req.model,
        flags,
        label: req
            .label
            .clone()
            .or_else(|| Some(qa_specialization_label(&req.specialization).to_string())),
        name: None,
        description: None,
        role: None,
        initial_prompt: req.initial_task,
    };

    let agent_info = {
        let controller = state.session_controller.write();
        controller
            .add_qa_worker(
                &session_id,
                config,
                req.specialization.clone(),
                req.parent_id,
            )
            .map_err(map_add_qa_worker_error)?
    };

    let index = match &agent_info.role {
        AgentRole::QaWorker { index, .. } => *index,
        _ => {
            return Err(ApiError::internal(
                "add_qa_worker returned a non-QaWorker role".to_string(),
            ));
        }
    };

    Ok((
        StatusCode::CREATED,
        Json(AddQaWorkerResponse {
            worker_id: agent_info.id,
            role: qa_specialization_label(&req.specialization).to_string(),
            cli,
            status: "Running".to_string(),
            task_file: {
                let controller = state.session_controller.read();
                let session = controller
                    .get_session(&session_id)
                    .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
                SessionController::absolute_task_file_path_for_qa_worker(
                    &session.project_path,
                    &session_id,
                    index as usize,
                )
                .to_string_lossy()
                .to_string()
            },
        }),
    ))
}

// --- Dev Login Endpoint ---

#[derive(Debug, Deserialize)]
pub struct DevLoginQuery {
    pub token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostVerdictRequest {
    pub verdict: String,
    #[serde(default)]
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub rationale: Option<String>,
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

    if matches!(session.state, SessionState::QaInProgress { .. }) {
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

fn override_log_details(verdict: &str) -> Result<(&'static str, &'static str), ApiError> {
    let normalized = verdict.trim().to_ascii_uppercase();
    match normalized.as_str() {
        "PASS" | "QA_VERDICT: PASS" => Ok(("FORCE-PASS", "QA pass")),
        "FAIL" | "QA_VERDICT: FAIL" => Ok(("FORCE-FAIL", "QA fail")),
        _ => Err(ApiError::bad_request(format!(
            "Unsupported QA verdict '{}'",
            verdict
        ))),
    }
}

fn normalize_post_verdict(verdict: &str) -> Result<&'static str, ApiError> {
    match verdict.trim().to_ascii_uppercase().as_str() {
        "PASS" => Ok("PASS"),
        "FAIL" => Ok("FAIL"),
        other => Err(ApiError::bad_request(format!(
            "Unsupported QA verdict '{}'. Expected PASS or FAIL",
            other
        ))),
    }
}

fn build_verdict_content(
    verdict: &str,
    rationale: Option<&str>,
    commit_sha: Option<&str>,
) -> String {
    let mut content = serde_json::Map::new();
    content.insert("kind".to_string(), json!("qa-verdict"));
    content.insert("verdict".to_string(), json!(verdict));
    if let Some(rationale) = rationale {
        content.insert("rationale".to_string(), json!(rationale));
    }
    if let Some(commit_sha) = commit_sha {
        content.insert("commit_sha".to_string(), json!(commit_sha));
    }
    Value::Object(content).to_string()
}

pub(crate) fn apply_verdict(
    state: &AppState,
    session_id: &str,
    verdict: &str,
    is_override: bool,
) -> Result<SessionState, ApiError> {
    let action = if is_override {
        let (action, _) = override_log_details(verdict)?;
        match action {
            "FORCE-PASS" => "force-pass",
            "FORCE-FAIL" => "force-fail",
            _ => unreachable!("override verdicts are normalized before logging"),
        }
    } else {
        "qa-verdict"
    };

    let controller = state.session_controller.read();
    require_qa_in_progress(&controller, session_id, action)?;
    let new_state = controller
        .on_qa_verdict(session_id, verdict)
        .map_err(ApiError::internal)?;
    drop(controller);

    if is_override {
        let (log_action, detail) = override_log_details(verdict)?;
        append_operator_log(state, session_id, log_action, detail);
    }

    Ok(new_state)
}

pub async fn post_verdict(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<PostVerdictRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let verdict = normalize_post_verdict(&req.verdict)?;
    let commit_sha = req
        .commit_sha
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let rationale = req
        .rationale
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let verdict_content = build_verdict_content(verdict, rationale, commit_sha);
    let (project_path, evaluator_id, queen_id, new_state) = {
        let controller = state.session_controller.read();
        require_qa_in_progress(&controller, &session_id, "qa-verdict")?;
        let session = controller
            .get_session(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
        let evaluator_id = session
            .agents
            .iter()
            .find(|agent| matches!(agent.role, AgentRole::Evaluator))
            .map(|agent| agent.id.clone())
            .unwrap_or_else(|| format!("{}-evaluator", session_id));
        let new_state = controller
            .record_http_qa_verdict(&session_id, &evaluator_id, verdict, commit_sha)
            .map_err(ApiError::internal)?;
        (
            session.project_path.clone(),
            evaluator_id,
            format!("{}-queen", session_id),
            new_state,
        )
    };

    let verdict_message = CoordinationMessage::qa_verdict(&evaluator_id, &queen_id, &verdict_content);
    if let Err(err) = state
        .storage
        .append_coordination_log(&session_id, &verdict_message)
    {
        tracing::warn!(
            session_id = %session_id,
            error = %err,
            "Failed to append QA verdict audit log after HTTP verdict"
        );
    }

    let state_manager = StateManager::new(project_path.join(".hive-manager").join(&session_id));
    if let Err(err) = state_manager.write_qa_verdict(
        &evaluator_id,
        &queen_id,
        &verdict_content,
        commit_sha,
    ) {
        tracing::warn!(
            session_id = %session_id,
            error = %err,
            "Failed to persist QA verdict peer record after HTTP verdict"
        );
    }

    Ok(Json(json!({
        "session_id": session_id,
        "action": "qa-verdict",
        "verdict": verdict,
        "new_state": format!("{:?}", new_state),
        "commit_sha": commit_sha,
        "rationale": rationale,
    })))
}

pub async fn force_pass(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let new_state = apply_verdict(&state, &session_id, "QA_VERDICT: PASS", true)?;

    Ok(Json(json!({
        "session_id": session_id,
        "action": "force-pass",
        "new_state": format!("{:?}", new_state)
    })))
}

pub async fn force_fail(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let new_state = apply_verdict(&state, &session_id, "QA_VERDICT: FAIL", true)?;

    Ok(Json(json!({
        "session_id": session_id,
        "action": "force-fail",
        "new_state": format!("{:?}", new_state)
    })))
}

#[cfg(test)]
mod tests {
    use super::map_add_qa_worker_error;
    use axum::http::StatusCode;

    #[test]
    fn maps_missing_session_to_not_found() {
        let error = map_add_qa_worker_error("Session not found: demo-session".to_string());
        assert_eq!(error.status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn maps_missing_evaluator_to_bad_request() {
        let error = map_add_qa_worker_error(
            "Evaluator demo-evaluator not found for session demo-session".to_string(),
        );
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn maps_spawn_failures_to_internal() {
        let error = map_add_qa_worker_error("Failed to spawn QA worker 1: boom".to_string());
        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
