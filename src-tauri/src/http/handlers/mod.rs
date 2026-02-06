pub mod health;
pub mod sessions;
pub mod inject;
pub mod workers;
pub mod planners;
pub mod learnings;

use crate::http::error::ApiError;

const VALID_CLIS: &[&str] = &["claude", "gemini", "codex", "opencode", "cursor", "droid", "qwen"];

/// Validate session_id for path traversal attacks
pub fn validate_session_id(session_id: &str) -> Result<(), ApiError> {
    if session_id.contains("..") || session_id.contains('/') || session_id.contains('\\') {
        return Err(ApiError::bad_request(
            "Invalid session ID: must not contain '..', '/', or '\\'",
        ));
    }
    Ok(())
}

/// Validate CLI against allowlist
pub fn validate_cli(cli: &str) -> Result<(), ApiError> {
    if !VALID_CLIS.contains(&cli) {
        return Err(ApiError::bad_request(format!(
            "Invalid CLI '{}'. Valid options: {}",
            cli,
            VALID_CLIS.join(", ")
        )));
    }
    Ok(())
}
