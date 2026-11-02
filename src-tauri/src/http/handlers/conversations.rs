use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use super::{canonical_agent_id, validate_agent_id, validate_session_id};
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::storage::ConversationMessage;

const MAX_MESSAGE_CONTENT_LEN: usize = 1_048_576; // 1MB - allows large pastes
const MAX_FROM_LEN: usize = 64;
pub(crate) const SHARED_CONVERSATION_ID: &str = "shared";

/// Resolve an agent conversation to the same full id used by the roster and UI.
/// `shared` is a reserved session-wide broadcast channel, not an agent id.
pub(crate) fn canonical_conversation_id(session_id: &str, agent_id: &str) -> String {
    if agent_id == SHARED_CONVERSATION_ID {
        SHARED_CONVERSATION_ID.to_string()
    } else {
        canonical_agent_id(session_id, agent_id)
    }
}

#[derive(Debug, Deserialize, Serialize)]
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

fn ensure_session_exists(state: &AppState, session_id: &str) -> Result<(), ApiError> {
    // Failed legacy posts may have left bare directories behind. Only session
    // metadata or the controller roster establishes a real session, including
    // closed/completed sessions whose histories remain on disk.
    let known_session = state
        .session_controller
        .read()
        .get_session(session_id)
        .is_some()
        || state
            .storage
            .session_dir(session_id)
            .join("session.json")
            .is_file();
    if !known_session {
        return Err(ApiError::not_found("Session not found"));
    }
    Ok(())
}

async fn reject_conversation(
    state: &AppState,
    session_id: &str,
    caller_id: Option<&str>,
    claimed_id: &str,
    token: Option<&str>,
    reason: &'static str,
) {
    if super::heartbeats::local_api_security_event(
        state,
        session_id,
        caller_id,
        claimed_id,
        None,
        token,
        reason,
        true,
        "conversation",
    )
    .await
    .is_err()
    {
        tracing::warn!("Failed to persist rejected conversation audit; request remains rejected");
    }
}

/// POST /api/sessions/{id}/conversations/{agent}/append
pub async fn append_conversation(
    State(state): State<Arc<AppState>>,
    Path((session_id, agent_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(req): Json<AppendMessageRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    validate_session_id(&session_id)?;
    validate_agent_id(&agent_id)?;
    let agent_id = canonical_conversation_id(&session_id, &agent_id);
    let from = sanitize_text(&req.from, MAX_FROM_LEN, "from")?;
    validate_agent_id(&from)?;
    let content = sanitize_text(&req.content, MAX_MESSAGE_CONTENT_LEN, "content")?;
    ensure_session_exists(&state, &session_id)?;

    let token_headers = headers.get_all("X-Hive-Agent-Token");
    let token = headers
        .get("X-Hive-Agent-Token")
        .and_then(|header| header.to_str().ok());
    // Match heartbeat compatibility: only a single decodable empty value is absent.
    let usable_token_absent = !headers.contains_key("X-Hive-Agent-Token")
        || (token_headers.iter().count() == 1
            && token.is_some_and(|token| token.trim().is_empty()));
    if !usable_token_absent {
        let authenticated = if token_headers.iter().count() == 1 {
            token.and_then(|token| {
                state
                    .pty_manager
                    .read()
                    .authenticate_heartbeat(&session_id, token)
            })
        } else {
            None
        };
        if authenticated.is_none() {
            // A duplicate or undecodable header cannot safely redact the claimed identity.
            let audit_from = if token_headers.iter().count() == 1 && token.is_some() {
                from.as_str()
            } else {
                "redacted-untrusted-identity"
            };
            reject_conversation(
                &state,
                &session_id,
                None,
                audit_from,
                token,
                "invalid_token",
            )
            .await;
            return Err(ApiError::new(StatusCode::UNAUTHORIZED,
                "Missing or invalid X-Hive-Agent-Token; restart this agent to receive its spawn credential"));
        }
        let (caller_id, _) = authenticated.expect("credential was checked above");
        let payload = serde_json::to_string(&req)
            .map_err(|_| ApiError::bad_request("Invalid conversation payload"))?;
        if token.is_some_and(|token| {
            !token.is_empty()
                && (payload.contains(token)
                    || content.contains(token)
                    || from.contains(token)
                    || agent_id.contains(token))
        }) {
            reject_conversation(
                &state,
                &session_id,
                Some(&caller_id),
                &from,
                token,
                "credential_in_payload",
            )
            .await;
            return Err(ApiError::bad_request(
                "Conversation payload must not contain a credential value",
            ));
        }
        if canonical_agent_id(&session_id, &from) != canonical_agent_id(&session_id, &caller_id) {
            reject_conversation(
                &state,
                &session_id,
                Some(&caller_id),
                &from,
                token,
                "token_owner_mismatch",
            )
            .await;
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "X-Hive-Agent-Token may send only as its owning agent",
            ));
        }
    } else {
        if !state
            .pty_manager
            .read()
            .heartbeat_verifies_when_present(&session_id)
        {
            reject_conversation(&state, &session_id, None, &from, None, "missing_token").await;
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "Missing or invalid X-Hive-Agent-Token",
            ));
        }
        super::heartbeats::local_api_security_event(
            &state,
            &session_id,
            None,
            &from,
            None,
            None,
            "verify_when_present",
            false,
            "conversation",
        )
        .await?;
    }

    let message = state
        .storage
        .append_conversation_message(&session_id, &agent_id, &from, &content)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to append conversation message: {}", e)))?;

    if let Err(error) = state
        .emit_conversation_message(&session_id, &agent_id, &message)
        .await
    {
        tracing::warn!(
            "Failed to emit conversation message for session {} agent {}: {}",
            session_id,
            agent_id,
            error
        );
    }

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
    let agent_id = canonical_conversation_id(&session_id, &agent_id);
    let since = parse_since(query.since)?;
    ensure_session_exists(&state, &session_id)?;

    let messages = state
        .storage
        .read_conversation(&session_id, &agent_id, since)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to read conversation: {}", e)))?;

    Ok(Json(ConversationResponse { messages }))
}

#[cfg(test)]
mod tests;
