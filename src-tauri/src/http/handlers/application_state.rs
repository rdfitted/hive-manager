//! Application-state handlers: snapshot, watermark poll, and write.
//!
//! All rusqlite calls are synchronous and wrapped in `tokio::task::spawn_blocking` so
//! the `parking_lot::Mutex<Connection>` guard is never held across an `.await`. The
//! server stamps `updated_at` with its own clock (`chrono::Utc::now`) to avoid client
//! clock-skew issues.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::http::error::ApiError;
use crate::http::handlers::validate_session_id;
use crate::http::state::AppState;
use crate::storage::ApplicationStateRow;

/// Query params for the watermark poll endpoint.
#[derive(Debug, Deserialize)]
pub struct PollQuery {
    /// Exclusive watermark in unix millis; only rows with `updated_at > since` are returned.
    #[serde(default)]
    pub since: i64,
}

/// Request body for a write.
#[derive(Debug, Deserialize)]
pub struct WriteBody {
    pub key: String,
    pub value: serde_json::Value,
}

/// Request body for an atomic take (read-and-delete).
#[derive(Debug, Deserialize)]
pub struct TakeBody {
    pub key: String,
}

/// GET /api/sessions/{id}/application-state
/// Full snapshot of all rows for a session (used for hydrate-on-load / session-switch).
pub async fn get_application_state(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<ApplicationStateRow>>, ApiError> {
    validate_session_id(&session_id)?;

    let db = Arc::clone(&state.app_state_db);
    let rows = tokio::task::spawn_blocking(move || db.read_application_state(&session_id))
        .await
        .map_err(|e| ApiError::internal(format!("Task join error: {e}")))?
        .map_err(|e| ApiError::internal(format!("Failed to read application state: {e}")))?;

    Ok(Json(rows))
}

/// GET /api/sessions/{id}/application-state/poll?since={watermark_ms}
/// Returns only rows changed strictly after the watermark.
pub async fn poll_application_state(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(params): Query<PollQuery>,
) -> Result<Json<Vec<ApplicationStateRow>>, ApiError> {
    validate_session_id(&session_id)?;

    let db = Arc::clone(&state.app_state_db);
    let since = params.since;
    let rows = tokio::task::spawn_blocking(move || db.poll_changed(&session_id, since))
        .await
        .map_err(|e| ApiError::internal(format!("Task join error: {e}")))?
        .map_err(|e| ApiError::internal(format!("Failed to poll application state: {e}")))?;

    Ok(Json(rows))
}

/// POST /api/sessions/{id}/application-state
/// Upsert a key/value with a server-stamped `updated_at`; returns the written row.
pub async fn write_application_state(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(body): Json<WriteBody>,
) -> Result<Json<ApplicationStateRow>, ApiError> {
    validate_session_id(&session_id)?;

    if body.key.is_empty() || body.key.len() > 256 {
        return Err(ApiError::bad_request(
            "Invalid key: must be 1-256 characters",
        ));
    }

    // Server-authoritative timestamp avoids client clock skew.
    let updated_at_ms = chrono::Utc::now().timestamp_millis();
    let db = Arc::clone(&state.app_state_db);
    let key = body.key.clone();
    let value = body.value.clone();
    let session = session_id.clone();

    tokio::task::spawn_blocking(move || db.write(&session, &key, &value, updated_at_ms))
        .await
        .map_err(|e| ApiError::internal(format!("Task join error: {e}")))?
        .map_err(|e| ApiError::internal(format!("Failed to write application state: {e}")))?;

    Ok(Json(ApplicationStateRow {
        session_id,
        key: body.key,
        value: body.value,
        updated_at: updated_at_ms,
    }))
}

/// POST /api/sessions/{id}/application-state/take
/// Atomically read-and-delete a single key in one transaction, returning the taken row
/// or `null`. Used by #128 for one-shot context (`pending_selection_context`) with
/// exactly-one-turn semantics: a lagging/double submit cannot re-consume a key that was
/// already taken, because the read+delete is a single `BEGIN IMMEDIATE` transaction.
pub async fn take_application_state(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(body): Json<TakeBody>,
) -> Result<Json<Option<ApplicationStateRow>>, ApiError> {
    validate_session_id(&session_id)?;

    if body.key.is_empty() || body.key.len() > 256 {
        return Err(ApiError::bad_request(
            "Invalid key: must be 1-256 characters",
        ));
    }

    let db = Arc::clone(&state.app_state_db);
    let key = body.key.clone();
    let session = session_id.clone();

    let row = tokio::task::spawn_blocking(move || db.take_key(&session, &key))
        .await
        .map_err(|e| ApiError::internal(format!("Task join error: {e}")))?
        .map_err(|e| ApiError::internal(format!("Failed to take application state: {e}")))?;

    Ok(Json(row))
}
