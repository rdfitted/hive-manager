use axum::{extract::Path, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::session::SessionState;
use super::validate_session_id;
use super::validate_agent_id;

/// POST /api/sessions/{id}/heartbeat - Body
#[derive(Debug, Deserialize)]
pub struct PostHeartbeatRequest {
    pub agent_id: String,
    pub status: String,
    #[serde(default)]
    pub summary: Option<String>,
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

    let controller = state.session_controller.read();
    if controller.get_session(&session_id).is_none() {
        return Err(ApiError::not_found(format!("Session {} not found", session_id)));
    }

    controller
        .update_heartbeat(&session_id, &req.agent_id, &req.status, req.summary.as_deref())
        .map_err(|e| ApiError::internal(e))?;

    Ok((
        StatusCode::OK,
        Json(PostHeartbeatResponse {
            message: "Heartbeat recorded".to_string(),
        }),
    ))
}

/// GET /api/sessions/active - Returns sessions with Running state and agent heartbeats
pub async fn get_active_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ActiveSessionsResponse>, ApiError> {
    let controller = state.session_controller.read();
    let all_sessions = controller.list_sessions();

    let sessions: Vec<ActiveSessionInfo> = all_sessions
        .into_iter()
        .filter(|s| s.state == SessionState::Running)
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
                    crate::session::SessionType::Solo { cli, .. } => format!("Solo ({})", cli),
                },
                project_path: session.project_path.to_string_lossy().to_string(),
                agents,
            }
        })
        .collect();

    Ok(Json(ActiveSessionsResponse { sessions }))
}
