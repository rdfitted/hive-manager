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

/// Validate project path for path traversal and existence
pub fn validate_project_path(path: &str) -> Result<(), ApiError> {
    use std::path::Path;
    
    // Check for path traversal sequences
    if path.contains("..") {
        return Err(ApiError::bad_request(
            "Invalid project path: must not contain '..' (path traversal)",
        ));
    }
    
    // Verify the path exists and is a directory
    let project_path = Path::new(path);
    if !project_path.exists() {
        return Err(ApiError::bad_request(format!(
            "Project path does not exist: {}",
            path
        )));
    }
    if !project_path.is_dir() {
        return Err(ApiError::bad_request(format!(
            "Project path is not a directory: {}",
            path
        )));
    }
    
    Ok(())
}
