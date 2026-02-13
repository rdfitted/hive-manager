use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::storage::Learning;
use super::validate_session_id;

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

#[derive(Debug, Deserialize, Default)]
pub struct LearningsFilter {
    pub category: Option<String>,
    pub keywords: Option<String>,
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

/// Validate SubmitLearningRequest fields (session, task, insight, outcome, files_touched).
/// Shared by submit_learning and submit_learning_for_session.
fn validate_submit_learning_request(req: &SubmitLearningRequest) -> Result<(), ApiError> {
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
    Ok(())
}

/// Build a Learning from a validated SubmitLearningRequest.
fn learning_from_request(req: SubmitLearningRequest) -> (Learning, String) {
    let learning_id = uuid::Uuid::new_v4().to_string();
    let learning = Learning {
        id: learning_id.clone(),
        date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        session: req.session,
        task: req.task,
        outcome: req.outcome,
        keywords: req.keywords,
        insight: req.insight,
        files_touched: req.files_touched,
    };
    (learning, learning_id)
}

/// Apply case-insensitive filtering on learnings by category and keywords
fn filter_learnings(learnings: Vec<Learning>, params: &LearningsFilter) -> Vec<Value> {
    let cat_lower = params.category.as_deref().map(|c| c.to_lowercase());
    let filter_kws: HashSet<String> = params
        .keywords
        .as_deref()
        .map(|k| {
            k.split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    learnings
        .into_iter()
        .filter(|learning| {
            if let Some(ref cat) = cat_lower {
                if learning.outcome.to_lowercase() != *cat {
                    return false;
                }
            }
            if !filter_kws.is_empty()
                && !learning
                    .keywords
                    .iter()
                    .any(|lk| filter_kws.contains(&lk.to_lowercase()))
            {
                return false;
            }
            true
        })
        .map(|learning| {
            json!({
                "id": learning.id,
                "date": learning.date,
                "session": learning.session,
                "task": learning.task,
                "outcome": learning.outcome,
                "keywords": learning.keywords,
                "insight": learning.insight,
                "files_touched": learning.files_touched,
            })
        })
        .collect()
}

/// POST /api/learnings - Submit a learning (project-scoped, legacy)
/// DEPRECATED: Use POST /api/sessions/{session_id}/learnings for new code
pub async fn submit_learning(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SubmitLearningRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    validate_submit_learning_request(&req)?;
    let project_path = resolve_project_path(&state)?;
    let (learning, learning_id) = learning_from_request(req);

    state
        .storage
        .append_learning(&project_path, &learning)
        .map_err(|e| ApiError::internal(format!("Failed to save learning: {}", e)))?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "message": "Learning submitted successfully",
            "learning_id": learning_id,
        })),
    ))
}

/// GET /api/learnings - List all learnings (project-scoped, legacy)
/// DEPRECATED: Use GET /api/sessions/{session_id}/learnings for new code
pub async fn list_learnings(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LearningsFilter>,
) -> Result<Json<Value>, ApiError> {
    let project_path = resolve_project_path(&state)?;

    let learnings = state
        .storage
        .read_learnings(&project_path)
        .map_err(|e| ApiError::internal(format!("Failed to read learnings: {}", e)))?;

    let learnings_json = filter_learnings(learnings, &params);

    Ok(Json(json!({
        "learnings": learnings_json,
        "count": learnings_json.len()
    })))
}

/// GET /api/project-dna - Get curated project DNA content (project-scoped, legacy)
/// DEPRECATED: Use GET /api/sessions/{session_id}/project-dna for new code
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
    validate_session_id(&session_id)?;
    validate_submit_learning_request(&req)?;
    let (learning, learning_id) = learning_from_request(req);

    state
        .storage
        .append_learning_session(&session_id, &learning)
        .map_err(|e| ApiError::internal(format!("Failed to save learning: {}", e)))?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "message": "Learning submitted successfully",
            "learning_id": learning_id,
        })),
    ))
}

/// GET /api/sessions/{id}/learnings - List learnings for a session (session-scoped)
pub async fn list_learnings_for_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(params): Query<LearningsFilter>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    // Use session-scoped storage
    let learnings = state
        .storage
        .read_learnings_session(&session_id)
        .map_err(|e| ApiError::internal(format!("Failed to read learnings: {}", e)))?;

    let learnings_json = filter_learnings(learnings, &params);

    Ok(Json(json!({
        "learnings": learnings_json,
        "count": learnings_json.len()
    })))
}

/// DELETE /api/sessions/{id}/learnings/{learning_id} - Delete a specific learning by ID
pub async fn delete_learning_for_session(
    State(state): State<Arc<AppState>>,
    Path((session_id, learning_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    validate_session_id(&session_id)?;

    let found = state
        .storage
        .delete_learning_session(&session_id, &learning_id)
        .map_err(|e| ApiError::internal(format!("Failed to delete learning: {}", e)))?;

    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!(
            "Learning {} not found in session {}",
            learning_id, session_id
        )))
    }
}

/// GET /api/sessions/{id}/project-dna - Get project DNA for a session (session-scoped)
pub async fn get_project_dna_for_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    // Use session-scoped storage
    let content = state
        .storage
        .read_project_dna_session(&session_id)
        .map_err(|e| ApiError::internal(format!("Failed to read project DNA: {}", e)))?;

    Ok(Json(json!({
        "content": content
    })))
}
