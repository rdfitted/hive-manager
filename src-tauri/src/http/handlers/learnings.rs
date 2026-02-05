use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::storage::Learning;

/// Request to submit a learning
#[derive(Debug, Deserialize)]
pub struct SubmitLearningRequest {
    pub session: String,
    pub task: String,
    pub outcome: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub insight: String,
    #[serde(default)]
    pub files_touched: Vec<String>,
}

fn resolve_project_path(state: &AppState) -> Result<PathBuf, ApiError> {
    let controller = state.session_controller.read();
    let sessions = controller.list_sessions();

    if sessions.is_empty() {
        return Err(ApiError::bad_request("No active session to determine project path"));
    }

    let first_path = sessions[0].project_path.clone();
    if sessions.iter().any(|s| s.project_path != first_path) {
        return Err(ApiError::bad_request(
            "Multiple active sessions with different project paths. \
             Use session-scoped endpoints instead: \
             GET/POST /api/sessions/{session_id}/learnings, \
             GET /api/sessions/{session_id}/project-dna"
        ));
    }

    Ok(first_path)
}

/// Resolve project_path from a session ID (O(1) HashMap lookup)
fn resolve_session_project_path(state: &AppState, session_id: &str) -> Result<PathBuf, ApiError> {
    let controller = state.session_controller.read();
    let session = controller
        .get_session(session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
    Ok(session.project_path.clone())
}

/// POST /api/learnings - Submit a learning
pub async fn submit_learning(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SubmitLearningRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if req.session.trim().is_empty() {
        return Err(ApiError::bad_request("Session cannot be empty"));
    }

    if req.task.trim().is_empty() {
        return Err(ApiError::bad_request("Task cannot be empty"));
    }

    if req.insight.trim().is_empty() {
        return Err(ApiError::bad_request("Insight cannot be empty"));
    }

    match req.outcome.as_str() {
        "success" | "partial" | "failed" => {}
        _ => {
            return Err(ApiError::bad_request(
                "Outcome must be one of: success, partial, failed",
            ));
        }
    }

    for file_path in &req.files_touched {
        if file_path.contains("..") || file_path.starts_with('/') || file_path.contains('\\') {
            return Err(ApiError::bad_request(format!(
                "Invalid file path: {}",
                file_path
            )));
        }
    }

    let project_path = resolve_project_path(&state)?;

    let learning = Learning {
        date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        session: req.session,
        task: req.task,
        outcome: req.outcome,
        keywords: req.keywords,
        insight: req.insight,
        files_touched: req.files_touched,
    };

    state
        .storage
        .append_learning(&project_path, &learning)
        .map_err(|e| ApiError::internal(format!("Failed to save learning: {}", e)))?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "message": "Learning submitted successfully",
        })),
    ))
}

/// GET /api/learnings - List all learnings
pub async fn list_learnings(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let project_path = resolve_project_path(&state)?;

    let learnings = state
        .storage
        .read_learnings(&project_path)
        .map_err(|e| ApiError::internal(format!("Failed to read learnings: {}", e)))?;

    let learnings_json: Vec<Value> = learnings
        .iter()
        .map(|learning| {
            json!({
                "date": learning.date,
                "session": learning.session,
                "task": learning.task,
                "outcome": learning.outcome,
                "keywords": learning.keywords,
                "insight": learning.insight,
                "files_touched": learning.files_touched,
            })
        })
        .collect();

    Ok(Json(json!({
        "learnings": learnings_json,
        "count": learnings_json.len()
    })))
}

/// GET /api/project-dna - Get curated project DNA content
pub async fn get_project_dna(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    let project_path = resolve_project_path(&state)?;

    let content = state
        .storage
        .read_project_dna(&project_path)
        .map_err(|e| ApiError::internal(format!("Failed to read project DNA: {}", e)))?;

    Ok(Json(json!({
        "content": content
    })))
}

/// POST /api/sessions/{id}/learnings - Submit a learning (session-scoped)
pub async fn submit_learning_for_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<SubmitLearningRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if req.session.trim().is_empty() {
        return Err(ApiError::bad_request("Session cannot be empty"));
    }

    if req.task.trim().is_empty() {
        return Err(ApiError::bad_request("Task cannot be empty"));
    }

    if req.insight.trim().is_empty() {
        return Err(ApiError::bad_request("Insight cannot be empty"));
    }

    match req.outcome.as_str() {
        "success" | "partial" | "failed" => {}
        _ => {
            return Err(ApiError::bad_request(
                "Outcome must be one of: success, partial, failed",
            ));
        }
    }

    for file_path in &req.files_touched {
        if file_path.contains("..") || file_path.starts_with('/') || file_path.contains('\\') {
            return Err(ApiError::bad_request(format!(
                "Invalid file path: {}",
                file_path
            )));
        }
    }

    let project_path = resolve_session_project_path(&state, &session_id)?;

    let learning = Learning {
        date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        session: req.session,
        task: req.task,
        outcome: req.outcome,
        keywords: req.keywords,
        insight: req.insight,
        files_touched: req.files_touched,
    };

    state
        .storage
        .append_learning(&project_path, &learning)
        .map_err(|e| ApiError::internal(format!("Failed to save learning: {}", e)))?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "message": "Learning submitted successfully",
        })),
    ))
}

/// GET /api/sessions/{id}/learnings - List learnings for a session (session-scoped)
pub async fn list_learnings_for_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let project_path = resolve_session_project_path(&state, &session_id)?;

    let learnings = state
        .storage
        .read_learnings(&project_path)
        .map_err(|e| ApiError::internal(format!("Failed to read learnings: {}", e)))?;

    let learnings_json: Vec<Value> = learnings
        .iter()
        .map(|learning| {
            json!({
                "date": learning.date,
                "session": learning.session,
                "task": learning.task,
                "outcome": learning.outcome,
                "keywords": learning.keywords,
                "insight": learning.insight,
                "files_touched": learning.files_touched,
            })
        })
        .collect();

    Ok(Json(json!({
        "learnings": learnings_json,
        "count": learnings_json.len()
    })))
}

/// GET /api/sessions/{id}/project-dna - Get project DNA for a session (session-scoped)
pub async fn get_project_dna_for_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let project_path = resolve_session_project_path(&state, &session_id)?;

    let content = state
        .storage
        .read_project_dna(&project_path)
        .map_err(|e| ApiError::internal(format!("Failed to read project DNA: {}", e)))?;

    Ok(Json(json!({
        "content": content
    })))
}
