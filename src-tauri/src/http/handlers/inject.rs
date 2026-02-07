use axum::{
    extract::{Path, State},
    Json,
};
use std::sync::Arc;
use serde_json::{json, Value};
use serde::Deserialize;
use crate::http::error::ApiError;
use crate::http::state::AppState;
use super::validate_session_id;

#[derive(Deserialize)]
pub struct OperatorInjectRequest {
    pub target_agent_id: String,
    pub message: String,
}

#[derive(Deserialize)]
pub struct QueenInjectRequest {
    pub queen_id: String,
    pub target_worker_id: String,
    pub message: String,
}

pub async fn operator_inject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<OperatorInjectRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&id)?;

    let manager = state.injection_manager.read();
    manager
        .operator_inject(
            &id,
            &payload.target_agent_id,
            &payload.message,
        )
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(json!({
        "status": "success",
        "message": format!("Operator injection sent to session {}", id)
    })))
}

pub async fn queen_inject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<QueenInjectRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&id)?;

    let manager = state.injection_manager.read();
    manager
        .queen_inject(
            &id,
            &payload.queen_id,
            &payload.target_worker_id,
            &payload.message,
        )
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(json!({
        "status": "success",
        "message": format!("Queen injection sent to session {}", id)
    })))
}
