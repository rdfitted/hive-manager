use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};

use crate::http::{error::ApiError, state::AppState};

use super::validate_session_id;

pub async fn get_resolver_output(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<crate::domain::ResolverOutput>, ApiError> {
    validate_session_id(&session_id)?;

    let session_exists = {
        let controller = state.session_controller.read();
        controller.get_session(&session_id).is_some()
    } || state.storage.load_session(&session_id).is_ok();

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
