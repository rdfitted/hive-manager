use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::{
    http::{error::ApiError, state::AppState},
    templates::{builtin_role_packs, builtin_session_templates, SessionTemplate, TemplateCatalog},
};

use super::validate_template_id;

pub async fn list_templates(
    State(state): State<Arc<AppState>>,
) -> Result<Json<TemplateCatalog>, ApiError> {
    let mut templates = builtin_session_templates();
    templates.extend(
        state
            .storage
            .list_user_templates()
            .map_err(|err| ApiError::internal(err.to_string()))?,
    );

    Ok(Json(TemplateCatalog {
        templates,
        role_packs: builtin_role_packs(),
    }))
}

pub async fn get_template(
    State(state): State<Arc<AppState>>,
    Path(template_id): Path<String>,
) -> Result<Json<SessionTemplate>, ApiError> {
    validate_template_id(&template_id)?;

    if let Some(template) = builtin_session_templates()
        .into_iter()
        .find(|template| template.id == template_id)
    {
        return Ok(Json(template));
    }

    let template = state
        .storage
        .load_user_template(&template_id)
        .map_err(|err| ApiError::internal(err.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("Template {} not found", template_id)))?;

    Ok(Json(template))
}

pub async fn create_template(
    State(state): State<Arc<AppState>>,
    Json(mut template): Json<SessionTemplate>,
) -> Result<(StatusCode, Json<SessionTemplate>), ApiError> {
    validate_template_id(&template.id)?;
    if template.name.trim().is_empty() {
        return Err(ApiError::bad_request("Template name must not be empty"));
    }
    if template.cells.is_empty() {
        return Err(ApiError::bad_request("Template must include at least one cell"));
    }
    if template.is_builtin {
        return Err(ApiError::bad_request("Builtin templates cannot be overwritten"));
    }

    template.is_builtin = false;
    state
        .storage
        .save_user_template(&template)
        .map_err(|err| ApiError::internal(err.to_string()))?;

    Ok((StatusCode::CREATED, Json(template)))
}

pub async fn delete_template(
    State(state): State<Arc<AppState>>,
    Path(template_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    validate_template_id(&template_id)?;

    if builtin_session_templates()
        .iter()
        .any(|template| template.id == template_id)
    {
        return Err(ApiError::bad_request(
            "Builtin templates cannot be deleted",
        ));
    }

    let deleted = state
        .storage
        .delete_user_template(&template_id)
        .map_err(|err| ApiError::internal(err.to_string()))?;

    if !deleted {
        return Err(ApiError::not_found(format!(
            "Template {} not found",
            template_id
        )));
    }

    Ok(StatusCode::NO_CONTENT)
}
