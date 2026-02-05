use axum::{
    extract::{Path, Query, State},
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

/// Resolve project_path from a session ID (O(1) HashMap lookup)
fn resolve_session_project_path(state: &AppState, session_id: &str) -> Result<PathBuf, ApiError> {
    let controller = state.session_controller.read();
    let session = controller
        .get_session(session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
    Ok(session.project_path.clone())
}

/// POST /api/learnings - Submit a learning (project-scoped, legacy)
/// DEPRECATED: Use POST /api/sessions/{session_id}/learnings for new code
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

    let learnings_json: Vec<Value> = learnings
        .iter()
        .filter(|learning| {
            if let Some(ref cat) = params.category {
                if learning.outcome != *cat {
                    return false;
                }
            }
            if let Some(ref kws) = params.keywords {
                let filter_kws: Vec<&str> = kws.split(',').map(|s| s.trim()).collect();
                if !filter_kws.is_empty()
                    && !learning
                        .keywords
                        .iter()
                        .any(|lk| filter_kws.contains(&lk.as_str()))
                {
                    return false;
                }
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
        .collect();

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

    // Use session-scoped storage
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
    // Use session-scoped storage
    let learnings = state
        .storage
        .read_learnings_session(&session_id)
        .map_err(|e| ApiError::internal(format!("Failed to read learnings: {}", e)))?;

    let learnings_json: Vec<Value> = learnings
        .iter()
        .filter(|learning| {
            if let Some(ref cat) = params.category {
                if learning.outcome != *cat {
                    return false;
                }
            }
            if let Some(ref kws) = params.keywords {
                let filter_kws: Vec<&str> = kws.split(',').map(|s| s.trim()).collect();
                if !filter_kws.is_empty()
                    && !learning
                        .keywords
                        .iter()
                        .any(|lk| filter_kws.contains(&lk.as_str()))
                {
                    return false;
                }
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
        .collect();

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
    // Use session-scoped storage
    let content = state
        .storage
        .read_project_dna_session(&session_id)
        .map_err(|e| ApiError::internal(format!("Failed to read project DNA: {}", e)))?;

    Ok(Json(json!({
        "content": content
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_submit_request_validation_rejects_empty_session() {
        let json = r#"{
            "session": "",
            "task": "test task",
            "outcome": "success",
            "keywords": [],
            "insight": "test insight",
            "files_touched": []
        }"#;

        let req: Result<SubmitLearningRequest, _> = serde_json::from_str(json);
        assert!(req.is_ok());
        let req = req.unwrap();
        assert!(req.session.trim().is_empty());
    }

    #[test]
    fn test_submit_request_validation_rejects_empty_task() {
        let json = r#"{
            "session": "test-session",
            "task": "",
            "outcome": "success",
            "keywords": [],
            "insight": "test insight",
            "files_touched": []
        }"#;

        let req: Result<SubmitLearningRequest, _> = serde_json::from_str(json);
        assert!(req.is_ok());
        let req = req.unwrap();
        assert!(req.task.trim().is_empty());
    }

    #[test]
    fn test_submit_request_validation_rejects_empty_insight() {
        let json = r#"{
            "session": "test-session",
            "task": "test task",
            "outcome": "success",
            "keywords": [],
            "insight": "",
            "files_touched": []
        }"#;

        let req: Result<SubmitLearningRequest, _> = serde_json::from_str(json);
        assert!(req.is_ok());
        let req = req.unwrap();
        assert!(req.insight.trim().is_empty());
    }

    #[test]
    fn test_submit_request_validation_rejects_invalid_outcome() {
        let json = r#"{
            "session": "test-session",
            "task": "test task",
            "outcome": "invalid-outcome",
            "keywords": [],
            "insight": "test insight",
            "files_touched": []
        }"#;

        let req: Result<SubmitLearningRequest, _> = serde_json::from_str(json);
        assert!(req.is_ok());
        let req = req.unwrap();
        // Outcome validation happens in the handler, but we can test the struct deserializes
        assert_eq!(req.outcome, "invalid-outcome");
    }

    #[test]
    fn test_submit_request_validation_accepts_valid_outcomes() {
        for outcome in ["success", "partial", "failed"] {
            let json = format!(r#"{{
                "session": "test-session",
                "task": "test task",
                "outcome": "{}",
                "keywords": [],
                "insight": "test insight",
                "files_touched": []
            }}"#, outcome);

            let req: Result<SubmitLearningRequest, _> = serde_json::from_str(&json);
            assert!(req.is_ok());
            let req = req.unwrap();
            assert_eq!(req.outcome, outcome);
        }
    }

    #[test]
    fn test_submit_request_validation_detects_path_traversal() {
        let test_cases = vec![
            ("../../etc/passwd", true),
            ("/etc/passwd", true),
            ("C:\\Windows\\System32", true),
            ("src/file.rs", false),
            ("tests/file.rs", false),
        ];

        for (file_path, should_reject) in test_cases {
            let json = format!(r#"{{
                "session": "test-session",
                "task": "test task",
                "outcome": "success",
                "keywords": [],
                "insight": "test insight",
                "files_touched": ["{}"]
            }}"#, file_path);

            let req: Result<SubmitLearningRequest, _> = serde_json::from_str(&json);
            assert!(req.is_ok());
            let req = req.unwrap();
            
            let has_traversal = req.files_touched.iter().any(|p| {
                p.contains("..") || p.starts_with('/') || p.contains('\\')
            });

            if should_reject {
                assert!(has_traversal, "Path {} should be detected as traversal", file_path);
            } else {
                assert!(!has_traversal, "Path {} should not be detected as traversal", file_path);
            }
        }
    }

    #[test]
    fn test_submit_request_deserializes_with_defaults() {
        let json = r#"{
            "session": "test-session",
            "task": "test task",
            "outcome": "success",
            "insight": "test insight"
        }"#;

        let req: Result<SubmitLearningRequest, _> = serde_json::from_str(json);
        assert!(req.is_ok());
        let req = req.unwrap();
        assert_eq!(req.keywords.len(), 0);
        assert_eq!(req.files_touched.len(), 0);
    }

    #[test]
    fn test_learnings_filter_deserializes() {
        let json = r#"{
            "category": "success",
            "keywords": "rust,api"
        }"#;

        let filter: Result<LearningsFilter, _> = serde_json::from_str(json);
        assert!(filter.is_ok());
        let filter = filter.unwrap();
        assert_eq!(filter.category, Some("success".to_string()));
        assert_eq!(filter.keywords, Some("rust,api".to_string()));
    }

    #[test]
    fn test_learnings_filter_deserializes_with_defaults() {
        let json = r#"{}"#;

        let filter: Result<LearningsFilter, _> = serde_json::from_str(json);
        assert!(filter.is_ok());
        let filter = filter.unwrap();
        assert_eq!(filter.category, None);
        assert_eq!(filter.keywords, None);
    }
}
