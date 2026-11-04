//! Generic HTTP entrypoints for the action registry.
//!
//! - `GET  /api/actions`        — list every registered action with its JSON Schema (AC1).
//! - `POST /api/actions/{name}` — dispatch any registered action (caller = Http).
//!   This is the future agent/MCP surface.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

use crate::actions::render::envelope_for_action_result;
use crate::actions::{ActionContext, Caller};
use crate::http::error::ApiError;
use crate::http::state::AppState;

#[derive(Serialize)]
pub struct ActionDescriptor {
    pub name: String,
    pub input_schema: Value,
}

#[derive(Serialize)]
pub struct ListActionsResponse {
    pub actions: Vec<ActionDescriptor>,
}

/// GET /api/actions — list registered actions and their input schemas.
pub async fn list_actions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ListActionsResponse>, ApiError> {
    let registry = state.registry();
    let actions = registry
        .list()
        .into_iter()
        .map(|(name, schema)| {
            let input_schema = serde_json::to_value(&schema)
                .map_err(|e| ApiError::internal(format!("Failed to serialize schema: {}", e)))?;
            Ok(ActionDescriptor {
                name: name.to_string(),
                input_schema,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;

    Ok(Json(ListActionsResponse { actions }))
}

/// POST /api/actions/{name} — dispatch a registered action with caller = Http.
/// The request body is the action's input JSON; the response is the action's
/// raw output value wrapped in the `{ renderer?, data }` envelope.
pub async fn dispatch_action(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ApiError> {
    let input = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let mut ctx = ActionContext::new(Caller::Http, Arc::clone(&state));
    // Only approval requests require spawn credentials here. Other actions retain
    // their existing authentication policy and caller provenance.
    if name == "approval.request" {
        let requester = input
            .get("session_id")
            .and_then(Value::as_str)
            .and_then(|session_id| {
                if headers.get_all("X-Hive-Agent-Token").iter().count() != 1 {
                    return None;
                }
                let token = headers.get("X-Hive-Agent-Token")?.to_str().ok()?;
                state
                    .pty_manager
                    .read()
                    .authenticate_heartbeat(session_id, token)
            });
        ctx.requester_agent_id = Some(
            requester
                .ok_or_else(|| {
                    ApiError::new(
                        StatusCode::UNAUTHORIZED,
                        "Approval requests require a valid X-Hive-Agent-Token for this session",
                    )
                })?
                .0,
        );
    }
    let output = state.registry().dispatch(&name, &ctx, input).await?;
    Ok(Json(envelope_for_action_result(&name, output)))
}
