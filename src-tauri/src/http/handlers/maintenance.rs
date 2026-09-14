//! Operator maintenance routes.
//!
//! `POST /api/maintenance/purge-fixture-sessions[?apply=true]` (#288) lists, and with
//! `apply=true` deletes, the sessions that local test runs leaked into the operator's
//! real store before the HTTP test suite was isolated. A session is a purge candidate
//! only when it is provably not operator work: its project path no longer exists AND
//! either its id is not a UUID (every real session id is) or its project path pointed
//! into the OS temp directory (where `TempDir` fixtures live). Sessions the controller
//! still holds are never touched. Dry-run is the default.

use std::collections::HashSet;
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
    let live: HashSet<String> = state
        .session_controller
        .read()
        .list_sessions()
        .into_iter()
        .map(|session| session.id)
        .collect();

    let report = state
        .storage
        .purge_fixture_sessions(query.apply, &|id| live.contains(id))
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(report))
}
