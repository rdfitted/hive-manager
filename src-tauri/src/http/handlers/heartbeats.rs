use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::validate_agent_id;
use super::validate_session_id;
use crate::http::error::ApiError;
use crate::http::state::AppState;

/// POST /api/sessions/{id}/heartbeat - Body
#[derive(Debug, Deserialize)]
pub struct PostHeartbeatRequest {
    pub agent_id: String,
    pub status: String,
    #[serde(default)]
    pub summary: Option<String>,
    /// Durable queue assignment identity. Older prompts omit this and use the server-side
    /// deterministic fallback; a supplied identity is always treated as an exact fence.
    #[serde(default)]
    pub assignment_id: Option<i64>,
}

/// Response for POST heartbeat
#[derive(Serialize)]
pub struct PostHeartbeatResponse {
    pub message: String,
}

/// Agent info with last_activity for active sessions
#[derive(Serialize)]
pub struct ActiveAgentInfo {
    pub id: String,
    pub role: String,
    pub last_activity: Option<String>,
    pub status: Option<String>,
    pub summary: Option<String>,
}

/// Session in active sessions list
#[derive(Serialize)]
pub struct ActiveSessionInfo {
    pub id: String,
    pub session_type: String,
    pub project_path: String,
    pub agents: Vec<ActiveAgentInfo>,
}

/// GET /api/sessions/active response
#[derive(Serialize)]
pub struct ActiveSessionsResponse {
    pub sessions: Vec<ActiveSessionInfo>,
}

const VALID_HEARTBEAT_STATUSES: &[&str] = &["working", "idle", "completed"];

/// POST /api/sessions/{id}/heartbeat
pub async fn post_heartbeat(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<PostHeartbeatRequest>,
) -> Result<(StatusCode, Json<PostHeartbeatResponse>), ApiError> {
    validate_session_id(&session_id)?;
    validate_agent_id(&req.agent_id)?;

    if !VALID_HEARTBEAT_STATUSES.contains(&req.status.as_str()) {
        return Err(ApiError::bad_request(
            "Status must be one of: working, idle, completed",
        ));
    }
    if req.assignment_id.is_some_and(|assignment_id| assignment_id <= 0) {
        return Err(ApiError::bad_request(
            "assignment_id must be a positive server-issued identity",
        ));
    }

    // #175(f): reject heartbeats for agents that are not part of this session.
    // A ghost id previously returned 200 and, worse, could finalize a durable
    // queue row via `record_heartbeat` — which is how a `completed` beat for a
    // nonexistent `worker-4` silently retired a real queue row.
    //
    // The gate is deliberately FAIL-OPEN across three signals (roster, live PTY,
    // queue row). Every spawn path starts the PTY *before* pushing the roster
    // entry, and several remove the old entry first, so a strict roster check
    // would 404 during those windows — and the heartbeat snippet agents run uses
    // `curl -fsS`, which turns a 404 into a hard non-zero exit that aborts the
    // agent's shell block. Breaking heartbeating is worse than the bug.
    //
    // It still closes the hole: the harmful case is an id with no roster entry,
    // no live PTY and no queue row, and this runs BEFORE `record_heartbeat`.
    // The Queen posts a bare `queen` alias while its roster entry is
    // `{session}-queen`, so RESOLVE the posted id rather than rewriting it: an id
    // that is already a member wins, and only an unknown bare alias is expanded.
    // Rewriting unconditionally would break every agent whose roster id genuinely
    // is unqualified.
    let qualified = super::canonical_agent_id(&session_id, &req.agent_id);

    let (raw_known, qualified_known) = {
        let controller = state.session_controller.read();
        let session = controller.get_session(&session_id).ok_or_else(|| {
            ApiError::not_found(format!("Session {} not found", session_id))
        })?;
        let pty_manager = state.pty_manager.read();
        let known = |id: &str| {
            session.agents.iter().any(|a| a.id == id) || pty_manager.is_alive(id)
        };
        (known(&req.agent_id), known(&qualified))
    };

    let agent_id = if raw_known {
        req.agent_id.clone()
    } else if qualified_known {
        qualified
    } else {
        // Last resort: a durable queue row is proof of a real worker whose roster
        // push has not landed yet.
        let rows = state
            .queue_manager
            .repo()
            .rows_for_session(&session_id)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if rows.iter().any(|row| row.worker_id == req.agent_id) {
            req.agent_id.clone()
        } else if rows.iter().any(|row| row.worker_id == qualified) {
            qualified
        } else {
            tracing::warn!(
                session_id = %session_id,
                agent_id = %req.agent_id,
                "rejecting heartbeat from an agent that is not a member of this session"
            );
            return Err(ApiError::not_found(format!(
                "Agent {} is not a member of session {}",
                req.agent_id, session_id
            )));
        }
    };

    // Scope the (non-Send) parking_lot guard so it is dropped before the await below.
    {
        let controller = state.session_controller.read();
        controller
            .update_heartbeat(&session_id, &agent_id, &req.status, req.summary.as_deref())
            .map_err(|e| ApiError::internal(e))?;
    }

    // #126: record the heartbeat into the durable queue row, advancing the
    // continuation / no-progress counters. A worker with no matching queue row (e.g. the
    // Queen) is simply a no-op here.
    state
        .queue_manager
        .record_heartbeat_for_assignment(
            &session_id,
            &agent_id,
            req.assignment_id,
            &req.status,
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok((
        StatusCode::OK,
        Json(PostHeartbeatResponse {
            message: "Heartbeat recorded".to_string(),
        }),
    ))
}

/// GET /api/sessions/active - Returns active sessions and agent heartbeats
pub async fn get_active_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ActiveSessionsResponse>, ApiError> {
    let controller = state.session_controller.read();
    let all_sessions = controller.list_sessions();

    let sessions: Vec<ActiveSessionInfo> = all_sessions
        .into_iter()
        .filter(|s| s.state.is_monitorable())
        .map(|session| {
            let agents_with_heartbeats = controller.get_heartbeat_info(&session.id);
            let agents: Vec<ActiveAgentInfo> = session
                .agents
                .iter()
                .map(|a| {
                    let hb = agents_with_heartbeats.get(&a.id);
                    ActiveAgentInfo {
                        id: a.id.clone(),
                        role: format!("{:?}", a.role),
                        last_activity: hb.map(|h| h.last_activity.to_rfc3339()),
                        status: hb.map(|h| h.status.clone()),
                        summary: hb.and_then(|h| h.summary.clone()),
                    }
                })
                .collect();

            ActiveSessionInfo {
                id: session.id.clone(),
                session_type: match &session.session_type {
                    crate::session::SessionType::Hive { worker_count } => {
                        format!("Hive ({})", worker_count)
                    }
                    crate::session::SessionType::Swarm { planner_count } => {
                        format!("Swarm ({})", planner_count)
                    }
                    crate::session::SessionType::Fusion { .. } => "Fusion".to_string(),
                    crate::session::SessionType::Debate { .. } => "Debate".to_string(),
                    crate::session::SessionType::Solo { cli, .. } => format!("Solo ({})", cli),
                },
                project_path: session.project_path.to_string_lossy().to_string(),
                agents,
            }
        })
        .collect();

    Ok(Json(ActiveSessionsResponse { sessions }))
}
