use axum::{
    extract::{Path, State},
    Json,
};
use std::sync::Arc;

use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::storage::queue::QueueSnapshot;
use super::validate_session_id;

/// GET /api/sessions/{id}/queue — durable run-queue snapshot for the dashboard.
///
/// Reads from the `agent_run_queue` table (the source of truth), not the in-memory
/// `Session.agents` cache, so the response is accurate across app restarts.
pub async fn get_queue(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<QueueSnapshot>, ApiError> {
    validate_session_id(&session_id)?;
    let snapshot = state
        .queue_manager
        .queue_snapshot(&session_id)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(snapshot))
}
