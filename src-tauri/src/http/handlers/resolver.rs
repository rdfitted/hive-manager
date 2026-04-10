use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::http::{error::ApiError, state::AppState};
use crate::orchestrator::resolver::{Resolver, ResolverError};
use crate::session::SessionType;
use crate::storage::StorageError;

use super::{validate_candidate_ids, validate_session_id};

#[derive(Debug, Deserialize)]
pub struct LaunchResolverRequest {
    pub candidate_ids: Vec<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

pub async fn launch_resolver(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(body): Json<LaunchResolverRequest>,
) -> Result<Json<crate::domain::ResolverOutput>, ApiError> {
    validate_session_id(&session_id)?;
    validate_candidate_ids(&body.candidate_ids)?;

    if body.candidate_ids.is_empty() {
        return Err(ApiError::bad_request("candidate_ids must not be empty"));
    }

    // Check session exists and is Fusion mode
    let session = {
        let controller = state.session_controller.read();
        controller.get_session(&session_id)
    };

    let is_fusion = if let Some(ref session) = session {
        matches!(session.session_type, SessionType::Fusion { .. })
    } else {
        // Fall back to persisted session
        match state.storage.load_session(&session_id) {
            Ok(persisted) => matches!(
                persisted.session_type,
                crate::storage::SessionTypeInfo::Fusion { .. }
            ),
            Err(StorageError::SessionNotFound(_)) => {
                return Err(ApiError::not_found(format!(
                    "Session {} not found",
                    session_id
                )));
            }
            Err(err) => return Err(ApiError::internal(err.to_string())),
        }
    };

    if !is_fusion {
        return Err(ApiError::bad_request(
            "Resolver launch is only available for Fusion sessions",
        ));
    }

    // Launch resolver
    let resolver_storage = crate::storage::SessionStorage::new_with_base(
        state.storage.base_dir().clone(),
    )
    .map_err(|err| ApiError::internal(format!("Failed to initialize resolver storage: {}", err)))?;
    let resolver = Resolver::new(resolver_storage);

    // Wait for candidates if timeout specified
    if let Some(timeout_secs) = body.timeout_secs {
        let timeout = std::time::Duration::from_secs(timeout_secs);
        resolver
            .wait_for_candidates(&session_id, &body.candidate_ids, timeout)
            .map_err(|err| match err {
                ResolverError::Timeout => {
                    ApiError::new(StatusCode::REQUEST_TIMEOUT, "Timed out waiting for candidate artifacts")
                }
                ResolverError::NoCandidates => {
                    ApiError::bad_request("No candidate artifacts available")
                }
                other => ApiError::internal(other.to_string()),
            })?;
    }

    let output = resolver
        .launch(&session_id, body.candidate_ids)
        .map_err(|err| match err {
            ResolverError::NoCandidates => {
                ApiError::bad_request("No candidate artifacts available")
            }
            ResolverError::Timeout => {
                ApiError::new(StatusCode::REQUEST_TIMEOUT, "Resolver timed out")
            }
            ResolverError::IncompleteCandidates {
                requested,
                assembled,
            } => ApiError::bad_request(format!(
                "Incomplete candidates: requested {}, assembled {}",
                requested, assembled
            )),
            other => ApiError::internal(other.to_string()),
        })?;

    // Persist output
    resolver
        .persist_output(&session_id, &output)
        .map_err(|err| ApiError::internal(err.to_string()))?;

    // Persist session state as completed (capitalized to match SessionState::Completed serialization).
    // NOTE: In-memory session state is not updated here because SessionController lacks a public
    // state setter. The next session list refresh will pick up the persisted state.
    if let Ok(mut persisted) = state.storage.load_session(&session_id) {
        persisted.state = "Completed".to_string();
        let _ = state.storage.save_session(&persisted);
    }

    Ok(Json(output))
}

pub async fn get_resolver_output(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<crate::domain::ResolverOutput>, ApiError> {
    validate_session_id(&session_id)?;

    let session_exists = if {
        let controller = state.session_controller.read();
        controller.get_session(&session_id).is_some()
    } {
        true
    } else {
        match state.storage.load_session(&session_id) {
            Ok(_) => true,
            Err(StorageError::SessionNotFound(_)) => false,
            Err(err) => return Err(ApiError::internal(err.to_string())),
        }
    };

    if !session_exists {
        return Err(ApiError::not_found(format!(
            "Session {} not found",
            session_id
        )));
    }

    let output = state
        .storage
        .load_resolver_output(&session_id)
        .map_err(|err| ApiError::internal(err.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("Resolver output not found for {}", session_id)))?;

    Ok(Json(output))
}
