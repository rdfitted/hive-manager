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
    session::{AuthStrategy, Session, SessionState, SessionType},
    storage::{PersistedSession, StorageError},
};

use super::{cells::find_cell, validate_cell_id, validate_session_id};

fn session_type_from_persisted(session_type: &crate::storage::SessionTypeInfo) -> SessionType {
    match session_type {
        crate::storage::SessionTypeInfo::Hive { worker_count } => SessionType::Hive {
            worker_count: *worker_count,
        },
        crate::storage::SessionTypeInfo::Swarm { planner_count } => SessionType::Swarm {
            planner_count: *planner_count,
        },
        crate::storage::SessionTypeInfo::Fusion { variants } => SessionType::Fusion {
            variants: variants.clone(),
        },
        crate::storage::SessionTypeInfo::Solo { cli, model } => SessionType::Solo {
            cli: cli.clone(),
            model: model.clone(),
        },
    }
}

fn session_state_from_persisted(state: &str) -> SessionState {
    match state {
        "Planning" => SessionState::Planning,
        "PlanReady" => SessionState::PlanReady,
        "Starting" => SessionState::Starting,
        "WaitingForFusionVariants" => SessionState::WaitingForFusionVariants,
        "SpawningJudge" => SessionState::SpawningJudge,
        "Judging" => SessionState::Judging,
        "AwaitingVerdictSelection" => SessionState::AwaitingVerdictSelection,
        "MergingWinner" => SessionState::MergingWinner,
        "SpawningEvaluator" => SessionState::SpawningEvaluator,
        "QaPassed" => SessionState::QaPassed,
        "QaMaxRetriesExceeded" => SessionState::QaMaxRetriesExceeded,
        "Running" => SessionState::Running,
        "Paused" => SessionState::Paused,
        "Completed" => SessionState::Completed,
        "Closing" => SessionState::Closing,
        "Closed" => SessionState::Closed,
        value if value.starts_with("SpawningWorker(") => {
            let index = value
                .trim_start_matches("SpawningWorker(")
                .trim_end_matches(')')
                .parse()
                .unwrap_or(1);
            SessionState::SpawningWorker(index)
        }
        value if value.starts_with("WaitingForWorker(") => {
            let index = value
                .trim_start_matches("WaitingForWorker(")
                .trim_end_matches(')')
                .parse()
                .unwrap_or(1);
            SessionState::WaitingForWorker(index)
        }
        value if value.starts_with("SpawningPlanner(") => {
            let index = value
                .trim_start_matches("SpawningPlanner(")
                .trim_end_matches(')')
                .parse()
                .unwrap_or(1);
            SessionState::SpawningPlanner(index)
        }
        value if value.starts_with("WaitingForPlanner(") => {
            let index = value
                .trim_start_matches("WaitingForPlanner(")
                .trim_end_matches(')')
                .parse()
                .unwrap_or(1);
            SessionState::WaitingForPlanner(index)
        }
        value if value.starts_with("SpawningFusionVariant(") => {
            let index = value
                .trim_start_matches("SpawningFusionVariant(")
                .trim_end_matches(')')
                .parse()
                .unwrap_or(1);
            SessionState::SpawningFusionVariant(index)
        }
        value if value.starts_with("QaInProgress") => SessionState::QaInProgress { iteration: None },
        value if value.starts_with("QaFailed") => SessionState::QaFailed { iteration: 1 },
        value if value.starts_with("Failed(") => SessionState::Failed(
            value
                .trim_start_matches("Failed(")
                .trim_end_matches(')')
                .to_string(),
        ),
        _ => SessionState::Completed,
    }
}

fn session_from_persisted(persisted: PersistedSession) -> Session {
    Session {
        id: persisted.id,
        name: persisted.name,
        color: persisted.color,
        session_type: session_type_from_persisted(&persisted.session_type),
        project_path: persisted.project_path.into(),
        state: session_state_from_persisted(&persisted.state),
        created_at: persisted.created_at,
        last_activity_at: persisted
            .last_activity_at
            .unwrap_or(persisted.created_at),
        agents: vec![],
        default_cli: persisted.default_cli,
        default_model: persisted.default_model,
        max_qa_iterations: persisted.max_qa_iterations,
        qa_timeout_secs: persisted.qa_timeout_secs,
        auth_strategy: AuthStrategy::default(),
        worktree_path: persisted.worktree_path,
        worktree_branch: persisted.worktree_branch,
    }
}

fn load_session_for_cells(state: &AppState, session_id: &str) -> Result<Session, ApiError> {
    if let Some(session) = {
        let controller = state.session_controller.read();
        controller.get_session(session_id)
    } {
        return Ok(session);
    }

    match state.storage.load_session(session_id) {
        Ok(session) => Ok(session_from_persisted(session)),
        Err(StorageError::SessionNotFound(_)) => {
            Err(ApiError::not_found(format!("Session {} not found", session_id)))
        }
        Err(err) => Err(ApiError::internal(err.to_string())),
    }
}

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

    let session = load_session_for_cells(&state, &session_id)?;
    let cell = find_cell(&session, &state.storage, &cell_id)
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

    let session = load_session_for_cells(&state, &session_id)?;

    if find_cell(&session, &state.storage, &cell_id).is_none() {
        return Err(ApiError::not_found(format!("Cell {} not found", cell_id)));
    }

    if req.artifact.branch.trim().is_empty() {
        return Err(ApiError::bad_request("artifact.branch must not be empty"));
    }
    if req.artifact.commits.is_empty() {
        return Err(ApiError::bad_request("artifact.commits must not be empty"));
    }
    if req.artifact.changed_files.is_empty() {
        return Err(ApiError::bad_request("artifact.changed_files must not be empty"));
    }

    state
        .storage
        .save_artifact(&session_id, &cell_id, &req.artifact)
        .map_err(|err| ApiError::internal(err.to_string()))?;

    state
        .session_controller
        .read()
        .emit_artifact_updated_for_cell(&session_id, &cell_id, None);

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "session_id": session_id,
            "cell_id": cell_id,
            "message": "Artifact persisted"
        })),
    ))
}
