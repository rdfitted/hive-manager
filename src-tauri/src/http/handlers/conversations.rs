use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::storage::ConversationMessage;
use super::{validate_agent_id, validate_session_id};

const MAX_MESSAGE_CONTENT_LEN: usize = 4096;
const MAX_FROM_LEN: usize = 64;

#[derive(Debug, Deserialize)]
pub struct AppendMessageRequest {
    pub from: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct ConversationQuery {
    pub since: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ConversationResponse {
    pub messages: Vec<ConversationMessage>,
}

fn sanitize_text(input: &str, max_len: usize, field: &str) -> Result<String, ApiError> {
    let sanitized: String = input
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t' || *c == '\r')
        .collect();
    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request(format!("{} cannot be empty", field)));
    }
    if trimmed.len() > max_len {
        return Err(ApiError::bad_request(format!(
            "{} exceeds {} characters",
            field, max_len
        )));
    }
    Ok(trimmed.to_string())
}

fn parse_since(since: Option<String>) -> Result<Option<DateTime<Utc>>, ApiError> {
    match since {
        Some(raw) => {
            let dt = DateTime::parse_from_rfc3339(&raw)
                .map_err(|_| ApiError::bad_request("Invalid since timestamp, expected RFC3339"))?
                .with_timezone(&Utc);
            Ok(Some(dt))
        }
        None => Ok(None),
    }
}

/// POST /api/sessions/{id}/conversations/{agent}/append
pub async fn append_conversation(
    State(state): State<Arc<AppState>>,
    Path((session_id, agent_id)): Path<(String, String)>,
    Json(req): Json<AppendMessageRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    validate_session_id(&session_id)?;
    validate_agent_id(&agent_id)?;
    let from = sanitize_text(&req.from, MAX_FROM_LEN, "from")?;
    validate_agent_id(&from)?;
    let content = sanitize_text(&req.content, MAX_MESSAGE_CONTENT_LEN, "content")?;

    state
        .storage
        .append_conversation_message(&session_id, &agent_id, &from, &content)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to append conversation message: {}", e)))?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "message": "Conversation message appended successfully"
        })),
    ))
}

/// GET /api/sessions/{id}/conversations/{agent}?since=<timestamp>
pub async fn read_conversation(
    State(state): State<Arc<AppState>>,
    Path((session_id, agent_id)): Path<(String, String)>,
    Query(query): Query<ConversationQuery>,
) -> Result<Json<ConversationResponse>, ApiError> {
    validate_session_id(&session_id)?;
    validate_agent_id(&agent_id)?;
    let since = parse_since(query.since)?;

    let messages = state
        .storage
        .read_conversation(&session_id, &agent_id, since)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to read conversation: {}", e)))?;

    Ok(Json(ConversationResponse { messages }))
}
