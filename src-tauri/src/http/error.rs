use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    /// Optional structured details for enriched error responses (e.g., 409 completion blocked)
    pub details: Option<HashMap<String, Value>>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            details: None,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    #[allow(dead_code)]
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    /// Create a conflict error with structured details
    pub fn conflict_with_details(message: impl Into<String>, details: HashMap<String, Value>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
            details: Some(details),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = if let Some(details) = self.details {
            // Merge message with details for structured response
            let mut map = details;
            map.insert("error".to_string(), json!(self.message));
            Json(Value::Object(map.into_iter().map(|(k, v)| (k, v)).collect()))
        } else {
            Json(json!({
                "error": self.message
            }))
        };
        (self.status, body).into_response()
    }
}
