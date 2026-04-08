use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::{
    domain::ArtifactBundle,
    http::{error::ApiError, state::AppState},
};

use super::{cells::find_cell, validate_cell_id, validate_session_id};

#[derive(Debug, Deserialize)]
pub struct PostArtifactRequest {
    pub artifact: ArtifactBundle,
}

pub async fn list_artifacts(
    State(state): State<Arc<AppState>>,
    Path((session_id, cell_id)): Path<(String, String)>,
) -> Result<Json<Vec<ArtifactBundle>>, ApiError> {
    validate_session_id(&session_id)?;
    validate_cell_id(&cell_id)?;

    let controller = state.session_controller.read();
    let session = controller
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
    let cell = find_cell(&session, &cell_id)
        .ok_or_else(|| ApiError::not_found(format!("Cell {} not found", cell_id)))?;

    let artifacts = cell.artifacts.into_iter().collect();
    Ok(Json(artifacts))
}

pub async fn post_artifact(
    State(state): State<Arc<AppState>>,
    Path((session_id, cell_id)): Path<(String, String)>,
    Json(req): Json<PostArtifactRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    validate_session_id(&session_id)?;
    validate_cell_id(&cell_id)?;

    let controller = state.session_controller.read();
    let session = controller
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    if find_cell(&session, &cell_id).is_none() {
        return Err(ApiError::not_found(format!("Cell {} not found", cell_id)));
    }

    let _ = req.artifact;

    Err(ApiError::internal(
        "Artifact persistence is not yet implemented for HTTP sessions",
    ))
}
