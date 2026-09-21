//! Operator maintenance routes.
//!
//! `POST /api/maintenance/purge-fixture-sessions[?apply=true]` (#288) lists, and with
//! `apply=true` deletes, the sessions that local test runs leaked into the operator's
//! real store before the HTTP test suite was isolated. A session is a purge candidate
//! only when it is provably not operator work: its project path no longer exists AND
//! either its id is not a UUID (every real session id is) or its project path pointed
//! into the OS temp directory (where `TempDir` fixtures live). Sessions the controller
//! holds are never touched: each delete runs under the session's lifecycle lock, which
//! `resume_session` also takes. Dry-run is the default.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::storage::FixturePurgeReport;

#[derive(Debug, Default, Deserialize)]
pub struct PurgeFixtureSessionsQuery {
    #[serde(default)]
    pub apply: bool,
}

pub async fn purge_fixture_sessions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PurgeFixtureSessionsQuery>,
) -> Result<Json<FixturePurgeReport>, ApiError> {
    let controller = Arc::clone(&state.session_controller);
    let storage = Arc::clone(&state.storage);
    let apply = query.apply;

    // The scan reads every session directory and, when applying, deletes trees, so it
    // runs on the blocking pool rather than a Tokio worker. Reporting checks liveness per
    // candidate; the delete itself goes through the controller, which holds the session's
    // lifecycle lock across its own liveness check and the removal. `resume_session`
    // takes the same lock, so a fixture cannot be activated in between.
    let report = tokio::task::spawn_blocking(move || {
        storage.purge_fixture_sessions(
            apply,
            &|id| controller.read().get_session(id).is_some(),
            &|id| {
                controller
                    .read()
                    .purge_persisted_session_unless_live(&storage, id)
            },
        )
    })
    .await
    .map_err(|e| ApiError::internal(format!("purge task failed: {e}")))?
    .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(report))
}
