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

pub async fn get_retirement_settings(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<crate::session::retirement::RetirementSettings>, ApiError> {
    validate_session_id(&session_id)?;
    if state.session_controller.read().get_session(&session_id).is_none() {
        return Err(ApiError::not_found("Session not found"));
    }
    state.storage.load_retirement_settings(&session_id).map(Json)
        .map_err(|error| ApiError::internal(error.to_string()))
}

pub async fn put_retirement_settings(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(mut settings): Json<crate::session::retirement::RetirementSettings>,
) -> Result<Json<crate::session::retirement::RetirementSettings>, ApiError> {
    validate_session_id(&session_id)?;
    let controller = state.session_controller.read();
    let session = controller.get_session(&session_id).ok_or_else(|| ApiError::not_found("Session not found"))?;
    if let Some(id) = settings.on_call_agent_id.as_mut() {
        validate_agent_id(id)?;
        *id = super::canonical_agent_id(&session_id, id);
        if !session.agents.iter().any(|agent| agent.id == *id) {
            return Err(ApiError::bad_request("On-call agent must be in this session"));
        }
    }
    state.storage.save_retirement_settings(&session_id, &settings)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(Json(settings))
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

    if find_cell(&session, &state.storage, &cell_id).is_none() {
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
    stop_agent_and_release(&state, &session_id, &agent_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod stop_tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn g53_delete_verified_exited_runtime_returns_204_and_releases_instance() {
        let temp = tempfile::tempdir().unwrap();
        let (app, controller, storage, state) = crate::http::tests::setup_test_app_full(temp.path().to_path_buf()).await;
        let session_id = "g53-delete-exited";
        let agent_id = "g53-delete-exited-worker-1";
        let mut session = crate::http::tests::make_test_session_with_agents(session_id, temp.path().to_str().unwrap(), &[agent_id]);
        session.no_git = true;
        controller.read().insert_test_session(session);
        state.pty_manager.read().create_session(agent_id.into(), PtyAgentRole::Worker { index: 1, parent: None },
            "fixture", &["--stub-die-on-start"], None, 80, 24).unwrap();
        let response = app.oneshot(Request::builder().method("DELETE")
            .uri(format!("/api/sessions/{session_id}/agents/{agent_id}"))
            .body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(state.pty_manager.read().instance_id(agent_id), None);
        assert!(matches!(controller.read().get_session(session_id).unwrap().agents[0].status,
            PtyAgentStatus::WaitingForInput(_)));
        assert!(storage.load_session(session_id).is_ok());
    }

    #[tokio::test]
    async fn g53_delete_failed_live_cleanup_keeps_running_roster_and_instance() {
        let temp = tempfile::tempdir().unwrap();
        let (app, controller, _storage, state) = crate::http::tests::setup_test_app_full(temp.path().to_path_buf()).await;
        let session_id = "g53-delete-live";
        let agent_id = "g53-delete-live-worker-1";
        controller.read().insert_test_session(crate::http::tests::make_test_session_with_agents(
            session_id, temp.path().to_str().unwrap(), &[agent_id]));
        state.pty_manager.read().create_session(agent_id.into(), PtyAgentRole::Worker { index: 1, parent: None },
            "fixture", &["--stub-tree-kill-fails-once"], None, 80, 24).unwrap();
        let instance = state.pty_manager.read().instance_id(agent_id).unwrap();
        let response = app.oneshot(Request::builder().method("DELETE")
            .uri(format!("/api/sessions/{session_id}/agents/{agent_id}"))
            .body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(state.pty_manager.read().instance_id(agent_id), Some(instance));
        assert!(matches!(controller.read().get_session(session_id).unwrap().agents[0].status, PtyAgentStatus::Running));
        state.pty_manager.read().kill_instance(agent_id, instance).unwrap();
    }
}

/// Share exact-target stopping and durable claim release with the Tauri command.
pub(crate) async fn stop_agent_and_release(state: &AppState, session_id: &str, agent_id: &str) -> Result<(), ApiError> {
    let instance = {
        let controller = state.session_controller.read();
        let session = controller
            .get_session(session_id)
            .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

        if !session.agents.iter().any(|agent| agent.id == agent_id) {
            return Err(ApiError::not_found(format!("Agent {} not found", agent_id)));
        }
        state.pty_manager.read().instance_id(agent_id)
    };
    let max_releases = state.queue_manager.repo().rows_for_session(session_id)
        .map_err(|error| ApiError::internal(error.to_string()))?.len();
    let mut stopped = false;
    for _ in 0..=max_releases {
        let released = state.queue_manager.release_claim_with_preparation(session_id, agent_id, || {
            if !stopped && instance.is_some() {
                let instance = instance.expect("captured runtime");
                state.session_controller.read().stop_agent_instance(session_id, agent_id, instance)
                    .map_err(|error| if error.contains("runtime instance was replaced") {
                        ApiError::new(StatusCode::CONFLICT, error)
                    } else { ApiError::internal(error) })?;
            } else if state.pty_manager.read().instance_id(agent_id).is_some() {
                return Err(ApiError::new(StatusCode::CONFLICT, "Agent runtime was replaced; stop rejected"));
            }
            Ok(())
        }).await.map_err(|error| ApiError::internal(error.to_string()))?;
        stopped = true;
        match released {
            crate::coordination::queue_manager::GuardedRelease::SpawnInFlight(_) =>
                return Err(ApiError::new(StatusCode::CONFLICT, "Agent spawn is in flight; retry stop after handoff")),
            crate::coordination::queue_manager::GuardedRelease::PreparationFailed(error) => return Err(error),
            crate::coordination::queue_manager::GuardedRelease::Complete {
                outcome: crate::coordination::ReleaseOutcome::Released { .. }, .. } => {},
            crate::coordination::queue_manager::GuardedRelease::Complete { .. } => return Ok(()),
        }
    }
    Err(ApiError::new(StatusCode::CONFLICT, "Agent claims changed during stop; retry release"))
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

    let manager = Arc::clone(&state.injection_manager);
    let injection_session_id = session_id.clone();
    let injection_agent_id = agent_id.clone();
    let injection_result = tokio::task::spawn_blocking(move || {
        manager.read().operator_inject(
            &injection_session_id,
            &injection_agent_id,
            &req.input,
            true,
        )
    })
    .await
    .map_err(|error| ApiError::internal(format!("Injection task failed: {error}")))?;
    let receipt = injection_result.map_err(|error| match error {
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
    super::inject::record_injection_submission(&state, &session_id, &agent_id, &receipt);
    if let Some(delivery) = &receipt.protocol_delivery {
        super::inject::record_delivery(&state, &session_id, &agent_id, &receipt, delivery);
    }

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
        PtyAgentRole::Prince => AgentRole::Reviewer,
        // Scratch shells never enter `Session::agents`; retain a total mapper in case
        // malformed persisted data reaches this boundary.
        PtyAgentRole::ScratchShell => AgentRole::Worker,
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
