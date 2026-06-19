//! Error type for the unified action contract.
//!
//! [`ActionError`] is the single error currency for every [`Action`](crate::actions::Action).
//! It deliberately round-trips losslessly to BOTH of the surfaces that dispatch actions:
//!
//! - the Tauri `#[command]` layer, which speaks `Result<T, String>` — via [`ActionError::to_message`];
//! - the Axum HTTP layer, which speaks [`ApiError`] — via `impl From<ActionError> for ApiError`.
//!
//! `ActionStatus` mirrors the categories `ApiError` needs so the conflict-with-details
//! path used by completion flows survives the conversion.

use std::collections::HashMap;

use serde_json::Value;

use crate::http::error::ApiError;

/// Coarse category for an [`ActionError`], chosen to map cleanly onto both
/// HTTP status codes and the Tauri string channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStatus {
    /// Input failed validation or was otherwise malformed (HTTP 400).
    BadRequest,
    /// The referenced resource does not exist (HTTP 404).
    NotFound,
    /// The request conflicts with current state, optionally with structured details (HTTP 409).
    Conflict,
    /// An unexpected internal failure (HTTP 500).
    Internal,
}

/// The unified error returned by every action.
#[derive(Debug, Clone)]
pub struct ActionError {
    pub status: ActionStatus,
    pub message: String,
    /// Optional structured details, preserved across the `ApiError` boundary
    /// (e.g. the 409 completion-blocked payload).
    pub details: Option<HashMap<String, Value>>,
}

impl ActionError {
    pub fn new(status: ActionStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            details: None,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ActionStatus::BadRequest, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ActionStatus::NotFound, message)
    }

    #[allow(dead_code)]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ActionStatus::Conflict, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ActionStatus::Internal, message)
    }

    /// Build a conflict error carrying structured details (mirrors
    /// [`ApiError::conflict_with_details`]).
    #[allow(dead_code)]
    pub fn conflict_with_details(
        message: impl Into<String>,
        details: HashMap<String, Value>,
    ) -> Self {
        Self {
            status: ActionStatus::Conflict,
            message: message.into(),
            details: Some(details),
        }
    }

    /// Render for the Tauri side, which returns `Result<T, String>` to the
    /// frontend. The plain message preserves the exact text the old
    /// `#[command]` bodies returned.
    pub fn to_message(&self) -> String {
        self.message.clone()
    }
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ActionError {}

/// Bridge controller methods that already return `Result<_, String>`: a bare
/// string maps to an internal error by default (callers that know better can
/// build a more specific status explicitly).
impl From<String> for ActionError {
    fn from(message: String) -> Self {
        ActionError::internal(message)
    }
}

impl From<&str> for ActionError {
    fn from(message: &str) -> Self {
        ActionError::internal(message.to_string())
    }
}

/// An `ApiError` raised by a shared validator (e.g. `validate_cli`) folds into
/// an `ActionError` preserving its category.
impl From<ApiError> for ActionError {
    fn from(error: ApiError) -> Self {
        use axum::http::StatusCode;
        let status = match error.status {
            StatusCode::BAD_REQUEST => ActionStatus::BadRequest,
            StatusCode::NOT_FOUND => ActionStatus::NotFound,
            StatusCode::CONFLICT => ActionStatus::Conflict,
            _ => ActionStatus::Internal,
        };
        ActionError {
            status,
            message: error.message,
            details: error.details,
        }
    }
}

/// The other half of the bridge: an `ActionError` becomes an `ApiError` so the
/// HTTP handlers can return it directly. Reuses the existing `ApiError`
/// constructors, including the structured conflict path.
impl From<ActionError> for ApiError {
    fn from(error: ActionError) -> Self {
        match (error.status, error.details) {
            (ActionStatus::BadRequest, _) => ApiError::bad_request(error.message),
            (ActionStatus::NotFound, _) => ApiError::not_found(error.message),
            (ActionStatus::Conflict, Some(details)) => {
                ApiError::conflict_with_details(error.message, details)
            }
            (ActionStatus::Conflict, None) => {
                ApiError::new(axum::http::StatusCode::CONFLICT, error.message)
            }
            (ActionStatus::Internal, _) => ApiError::internal(error.message),
        }
    }
}
