pub mod actions;
pub mod agents;
pub mod application_state;
pub mod artifacts;
pub mod cells;
pub mod conversations;
pub mod evaluator;
pub mod events;
pub mod health;
pub mod heartbeats;
pub mod inject;
pub mod knowledge;
pub mod learnings;
pub mod planners;
pub mod pty_buffer;
pub mod queue;
pub mod resolver;
pub mod session_files;
pub mod sessions;
pub mod templates;
pub mod work_graph;
pub mod workers;

use crate::http::error::ApiError;
use std::collections::HashSet;

// Must stay in lockstep with adapters/mod.rs::VALID_CLIS.
const VALID_CLIS: &[&str] = &[
    "claude",
    "codex",
    "opencode",
    "cursor",
    "droid",
    "qwen",
];

/// Validate session_id for path traversal attacks
pub fn validate_session_id(session_id: &str) -> Result<(), ApiError> {
    if session_id.contains("..") || session_id.contains('/') || session_id.contains('\\') {
        return Err(ApiError::bad_request(
            "Invalid session ID: must not contain '..', '/', or '\\'",
        ));
    }
    Ok(())
}

/// Validate cell_id to prevent path traversal and malformed names.
pub fn validate_cell_id(cell_id: &str) -> Result<(), ApiError> {
    if cell_id.is_empty() || cell_id.len() > 64 {
        return Err(ApiError::bad_request(
            "Invalid cell ID: must be 1-64 characters",
        ));
    }
    if cell_id.contains("..") || cell_id.contains('/') || cell_id.contains('\\') {
        return Err(ApiError::bad_request(
            "Invalid cell ID: must not contain '..', '/', or '\\'",
        ));
    }
    if !cell_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ApiError::bad_request(
            "Invalid cell ID: only alphanumeric characters, hyphens, and underscores are allowed",
        ));
    }
    Ok(())
}

/// Resolve a possibly-bare agent alias to its session-qualified roster id.
///
/// The Queen's own wait loops post `{"agent_id":"queen"}` — a BARE alias, not
/// `{session}-queen` — while its roster entry is `{session}-queen`. Any check
/// that compares the posted id against the roster must canonicalize first, or it
/// rejects the Queen; and because those loops use `curl -fsS`, a 4xx is a hard
/// non-zero exit that aborts the Queen's shell block mid-wait.
///
/// Canonicalizing (rather than merely allowing the alias) also repairs a latent
/// UI bug: heartbeats used to be stored under the raw `"queen"` key while
/// `GET /api/sessions/active` looks the Queen up by `{session}-queen`, so its
/// `last_activity` / `status` / `summary` were permanently `None`.
///
/// Ids that are already session-qualified are returned unchanged.
pub fn canonical_agent_id(session_id: &str, agent_id: &str) -> String {
    let prefix = format!("{}-", session_id);
    if agent_id.starts_with(&prefix) {
        agent_id.to_string()
    } else {
        format!("{}{}", prefix, agent_id)
    }
}

/// Validate agent_id to prevent path traversal and malformed names.
pub fn validate_agent_id(agent_id: &str) -> Result<(), ApiError> {
    if agent_id.is_empty() || agent_id.len() > 64 {
        return Err(ApiError::bad_request(
            "Invalid agent ID: must be 1-64 characters",
        ));
    }
    if agent_id.contains("..") || agent_id.contains('/') || agent_id.contains('\\') {
        return Err(ApiError::bad_request(
            "Invalid agent ID: must not contain '..', '/', or '\\'",
        ));
    }
    if !agent_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(ApiError::bad_request(
            "Invalid agent ID: only alphanumeric characters and hyphens are allowed",
        ));
    }
    Ok(())
}

pub fn validate_template_id(template_id: &str) -> Result<(), ApiError> {
    if template_id.is_empty() || template_id.len() > 64 {
        return Err(ApiError::bad_request(
            "Invalid template ID: must be 1-64 characters",
        ));
    }
    if template_id.contains("..") || template_id.contains('/') || template_id.contains('\\') {
        return Err(ApiError::bad_request(
            "Invalid template ID: must not contain '..', '/', or '\\'",
        ));
    }
    if !template_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ApiError::bad_request(
            "Invalid template ID: only alphanumeric characters, hyphens, and underscores are allowed",
        ));
    }
    Ok(())
}

/// Validate candidate IDs for path traversal attacks
pub fn validate_candidate_ids(candidate_ids: &[String]) -> Result<(), ApiError> {
    let mut seen = HashSet::new();

    for id in candidate_ids {
        if id.is_empty() || id.len() > 64 {
            return Err(ApiError::bad_request(
                "Invalid candidate ID: must be 1-64 characters",
            ));
        }
        if id.contains("..") || id.contains('/') || id.contains('\\') {
            return Err(ApiError::bad_request(
                "Invalid candidate ID: must not contain '..', '/', or '\\'",
            ));
        }
        if !seen.insert(id) {
            return Err(ApiError::bad_request(format!(
                "Duplicate candidate ID: {}",
                id
            )));
        }
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
