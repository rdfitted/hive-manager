use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;

use crate::http::error::ApiError;
use crate::http::state::AppState;

use super::{validate_agent_id, validate_session_id};

#[derive(Debug, Serialize)]
pub struct PtyBufferResponse {
    pub session_id: String,
    pub agent_id: String,
    pub output: String,
    pub byte_count: usize,
}

/// GET /api/sessions/{id}/agents/{agent_id}/pty-buffer
///
/// The route observes the existing 8 KB ring buffer only. It never writes bytes or submits input.
pub async fn get_pty_buffer(
    State(state): State<Arc<AppState>>,
    Path((session_id, agent_id)): Path<(String, String)>,
) -> Result<Json<PtyBufferResponse>, ApiError> {
    validate_session_id(&session_id)?;
    validate_agent_id(&agent_id)?;

    let session = state
        .session_controller
        .read()
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session not found: {session_id}")))?;
    if !session.agents.iter().any(|agent| agent.id == agent_id) {
        return Err(ApiError::not_found(format!(
            "Agent {agent_id} not found in session {session_id}"
        )));
    }

    let output = state
        .pty_manager
        .read()
        .recent_output(&agent_id)
        .ok_or_else(|| ApiError::not_found(format!("PTY not found for agent: {agent_id}")))?;
    let byte_count = output.len();
    Ok(Json(PtyBufferResponse {
        session_id,
        agent_id,
        output,
        byte_count,
    }))
}
