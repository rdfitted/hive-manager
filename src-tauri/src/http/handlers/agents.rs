use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::{
    coordination::InjectionError,
    domain::{Agent, AgentRole, AgentStatus},
    http::{error::ApiError, state::AppState},
    pty::{AgentRole as PtyAgentRole, AgentStatus as PtyAgentStatus},
};

use super::{
    cells::{agent_in_cell, find_cell},
    validate_agent_id, validate_cell_id, validate_session_id,
};

#[derive(Debug, Deserialize)]
pub struct SendAgentInputRequest {
    pub input: String,
}

pub async fn list_agents_in_cell(
    State(state): State<Arc<AppState>>,
    Path((session_id, cell_id)): Path<(String, String)>,
) -> Result<Json<Vec<Agent>>, ApiError> {
    validate_session_id(&session_id)?;
    validate_cell_id(&cell_id)?;

    let controller = state.session_controller.read();
    let session = controller
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
    let heartbeats = controller.get_heartbeat_info(&session_id);

    if find_cell(&session, &cell_id).is_none() {
        return Err(ApiError::not_found(format!("Cell {} not found", cell_id)));
    }

    let agents = session
        .agents
        .iter()
        .filter(|agent| agent_in_cell(&session, &cell_id, agent))
        .map(|agent| Agent {
            id: agent.id.clone(),
            cell_id: cell_id.clone(),
            role: map_agent_role(&agent.role),
            label: agent
                .config
                .label
                .clone()
                .unwrap_or_else(|| agent.id.clone()),
            cli: agent.config.cli.clone(),
            model: agent.config.model.clone(),
            status: map_agent_status(&agent.status),
            process_ref: Some(agent.id.clone()),
            terminal_ref: Some(agent.id.clone()),
            last_event_at: heartbeats.get(&agent.id).map(|heartbeat| heartbeat.last_activity),
        })
        .collect();

    Ok(Json(agents))
}

pub async fn stop_agent(
    State(state): State<Arc<AppState>>,
    Path((session_id, agent_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    validate_session_id(&session_id)?;
    validate_agent_id(&agent_id)?;

    {
        let controller = state.session_controller.read();
        let session = controller
            .get_session(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

        if !session.agents.iter().any(|agent| agent.id == agent_id) {
            return Err(ApiError::not_found(format!("Agent {} not found", agent_id)));
        }
    }

    let controller = state.session_controller.write();
    controller
        .stop_agent(&session_id, &agent_id)
        .map_err(ApiError::internal)?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn send_agent_input(
    State(state): State<Arc<AppState>>,
    Path((session_id, agent_id)): Path<(String, String)>,
    Json(req): Json<SendAgentInputRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    validate_session_id(&session_id)?;
    validate_agent_id(&agent_id)?;

    if req.input.trim().is_empty() {
        return Err(ApiError::bad_request("input must not be empty"));
    }

    {
        let controller = state.session_controller.read();
        let session = controller
            .get_session(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

        if !session.agents.iter().any(|agent| agent.id == agent_id) {
            return Err(ApiError::not_found(format!("Agent {} not found", agent_id)));
        }
    }

    state
        .injection_manager
        .read()
        .operator_inject(&session_id, &agent_id, &req.input)
        .map_err(|error| match error {
            InjectionError::SessionNotFound(id) => {
                ApiError::not_found(format!("Session {} not found", id))
            }
            InjectionError::AgentNotFound(id) => {
                ApiError::not_found(format!("Agent {} not found", id))
            }
            InjectionError::NotAuthorized(msg) => ApiError::bad_request(msg),
            InjectionError::PtyError(msg) | InjectionError::StorageError(msg) => {
                ApiError::internal(msg)
            }
        })?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "session_id": session_id,
            "agent_id": agent_id,
            "message": "Input sent"
        })),
    ))
}

fn map_agent_role(role: &PtyAgentRole) -> AgentRole {
    match role {
        PtyAgentRole::Queen => AgentRole::Queen,
        PtyAgentRole::Worker { .. } | PtyAgentRole::Fusion { .. } => AgentRole::Worker,
        PtyAgentRole::Judge { .. } | PtyAgentRole::Planner { .. } | PtyAgentRole::MasterPlanner => {
            AgentRole::Reviewer
        }
        PtyAgentRole::Evaluator | PtyAgentRole::QaWorker { .. } => AgentRole::Tester,
    }
}

fn map_agent_status(status: &PtyAgentStatus) -> AgentStatus {
    match status {
        PtyAgentStatus::Starting => AgentStatus::Launching,
        PtyAgentStatus::Running | PtyAgentStatus::Idle => AgentStatus::Running,
        PtyAgentStatus::WaitingForInput(_) => AgentStatus::WaitingInput,
        PtyAgentStatus::Completed => AgentStatus::Completed,
        PtyAgentStatus::Error(_) => AgentStatus::Failed,
    }
}
