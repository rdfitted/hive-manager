use std::sync::Arc;

use parking_lot::RwLock;
use tauri::{AppHandle, Emitter};
use thiserror::Error;

use crate::pty::PtyManager;
use crate::storage::SessionStorage;

use super::{CoordinationMessage, WorkerStateInfo};

#[derive(Debug, Error)]
pub enum InjectionError {
    #[allow(dead_code)]
    #[error("Session not found: {0}")]
    SessionNotFound(String),
    #[allow(dead_code)]
    #[error("Agent not found: {0}")]
    AgentNotFound(String),
    #[error("Not authorized: {0}")]
    NotAuthorized(String),
    #[error("PTY error: {0}")]
    PtyError(String),
    #[error("Storage error: {0}")]
    StorageError(String),
}

/// Manages Queen injection and coordination
pub struct InjectionManager {
    pty_manager: Arc<RwLock<PtyManager>>,
    storage: SessionStorage,
    app_handle: Option<AppHandle>,
}

impl InjectionManager {
    /// Create a new injection manager
    pub fn new(pty_manager: Arc<RwLock<PtyManager>>, storage: SessionStorage) -> Self {
        Self {
            pty_manager,
            storage,
            app_handle: None,
        }
    }

    /// Set the app handle for event emission
    pub fn set_app_handle(&mut self, handle: AppHandle) {
        self.app_handle = Some(handle);
    }

    /// Validate that the claimed agent ID matches the expected format for the session
    /// and verify it exists in the PTY manager (is a registered agent)
    fn validate_agent_role(
        &self,
        session_id: &str,
        agent_id: &str,
        expected_suffix: &str,
    ) -> Result<(), InjectionError> {
        // Build expected agent ID pattern: session_id-suffix
        let expected_prefix = format!("{}-", session_id);
        
        // Verify agent ID belongs to this session
        if !agent_id.starts_with(&expected_prefix) {
            return Err(InjectionError::NotAuthorized(format!(
                "Agent ID '{}' does not belong to session '{}'",
                agent_id, session_id
            )));
        }
        
        // Verify agent ID has the expected role suffix
        if !agent_id.ends_with(expected_suffix) && !agent_id.contains(expected_suffix) {
            return Err(InjectionError::NotAuthorized(format!(
                "Agent ID '{}' does not have expected role '{}'",
                agent_id, expected_suffix
            )));
        }
        
        // Verify agent exists in PTY manager (is a registered, active agent)
        let pty_manager = self.pty_manager.read();
        if !pty_manager.session_exists(agent_id) {
            return Err(InjectionError::NotAuthorized(format!(
                "Agent '{}' is not a registered active agent",
                agent_id
            )));
        }
        
        Ok(())
    }
    
    /// Queen injects a message to a worker
    pub fn queen_inject(
        &self,
        session_id: &str,
        queen_id: &str,
        target_worker_id: &str,
        message: &str,
    ) -> Result<(), InjectionError> {
        // Strong validation: verify queen_id belongs to session and is an active queen agent
        self.validate_agent_role(session_id, queen_id, "-queen")?;

        // Log to coordination.log
        let coord_message = CoordinationMessage::task(
            &format_agent_display(queen_id),
            &format_agent_display(target_worker_id),
            message,
        );

        self.storage
            .append_coordination_log(session_id, &coord_message)
            .map_err(|e| InjectionError::StorageError(e.to_string()))?;

        // Write to worker's PTY stdin
        self.write_to_agent(target_worker_id, message)?;

        // Emit event for UI
        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("coordination-message", &coord_message);
        }

        Ok(())
    }

    /// Queen initiates a branch switch for all workers
    pub fn queen_switch_branch(
        &self,
        session_id: &str,
        queen_id: &str,
        worker_ids: &[String],
        branch: &str,
    ) -> Result<Vec<(String, Result<(), InjectionError>)>, InjectionError> {
        // Strong validation: verify queen_id belongs to session and is an active queen agent
        self.validate_agent_role(session_id, queen_id, "-queen")?;

        let message = format!(
            "[BRANCH SWITCH] Switching all workers to branch: {}",
            branch
        );
        self.log_system_message(session_id, "ALL", &message)?;

        // Ctrl+C first to interrupt any running command
        let git_command = format!("\x03git switch {}", branch);

        let mut results = Vec::new();
        for worker_id in worker_ids {
            let result = self.write_to_agent(worker_id, &git_command);

            let status = if result.is_ok() {
                "initiated"
            } else {
                "failed"
            };
            let log_msg = format!(
                "[BRANCH SWITCH] Worker {} switch to '{}': {}",
                format_agent_display(worker_id),
                branch,
                status
            );
            let _ = self.log_system_message(session_id, &format_agent_display(worker_id), &log_msg);

            results.push((worker_id.clone(), result));
        }

        Ok(results)
    }

    /// Sanitize a string for safe logging - prevents log injection attacks
    /// Replaces control characters and limits length
    fn sanitize_for_log(s: &str, max_len: usize) -> String {
        let sanitized: String = s.chars()
            .map(|c| {
                if c.is_control() && c != ' ' {
                    // Replace control characters with their escape representation
                    match c {
                        '\n' => "\\n".to_string(),
                        '\r' => "\\r".to_string(),
                        '\t' => "\\t".to_string(),
                        '\0' => "\\0".to_string(),
                        _ => format!("\\x{:02x}", c as u32),
                    }
                } else {
                    c.to_string()
                }
            })
            .collect();
        
        if sanitized.len() > max_len {
            format!("{}...[truncated]", &sanitized[..max_len])
        } else {
            sanitized
        }
    }
    
    /// Write a message to an agent's PTY and press Enter to submit
    pub fn write_to_agent(&self, agent_id: &str, message: &str) -> Result<(), InjectionError> {
        let pty_manager = self.pty_manager.read();

        // Strip any existing line endings first
        let clean_message = message.trim_end_matches(&['\r', '\n'][..]);

        // Sanitize user-controlled data before logging to prevent log injection
        let safe_agent_id = Self::sanitize_for_log(agent_id, 128);
        let safe_message = Self::sanitize_for_log(clean_message, 500);
        
        tracing::info!("=== INJECTION START ===");
        tracing::info!("Target agent: {}", safe_agent_id);
        tracing::info!("Message length: {} chars", clean_message.len());
        tracing::debug!("Message preview: {}", safe_message);

        // Write the message content with Enter appended
        // On Windows ConPTY, Enter is typically just \r, but some apps need \n
        // We'll send both \r\n to maximize compatibility
        let message_with_enter = format!("{}\r\n", clean_message);

        tracing::debug!(
            "Message bytes count: {}",
            message_with_enter.as_bytes().len()
        );

        pty_manager
            .write(agent_id, message_with_enter.as_bytes())
            .map_err(|e| InjectionError::PtyError(format!("Failed to write: {}", e)))?;

        tracing::info!("=== INJECTION COMPLETE ===");

        Ok(())
    }

    /// Direct injection from operator to any agent (bypasses Queen authorization)
    pub fn operator_inject(
        &self,
        session_id: &str,
        target_agent_id: &str,
        message: &str,
    ) -> Result<(), InjectionError> {
        // Log to coordination.log
        let coord_message = CoordinationMessage::system(
            &format_agent_display(target_agent_id),
            &format!("[OPERATOR] {}", message),
        );

        self.storage
            .append_coordination_log(session_id, &coord_message)
            .map_err(|e| InjectionError::StorageError(e.to_string()))?;

        // Write to agent's PTY stdin
        self.write_to_agent(target_agent_id, message)?;

        // Emit event for UI
        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("coordination-message", &coord_message);
        }

        Ok(())
    }

    /// Notify Queen of new worker availability (logs only, no PTY injection)
    /// Queen spawns workers via HTTP API, so she already knows - no need to inject back
    pub fn notify_queen_worker_added(
        &self,
        session_id: &str,
        queen_id: &str,
        worker: &WorkerStateInfo,
    ) -> Result<(), InjectionError> {
        let message = format!(
            "[SYSTEM] New worker available: {} ({}) - {}",
            worker.id, worker.role.label, worker.cli
        );

        // Log to coordination.log (for audit purposes)
        let coord_message = CoordinationMessage::system(&format_agent_display(queen_id), &message);

        self.storage
            .append_coordination_log(session_id, &coord_message)
            .map_err(|e| InjectionError::StorageError(e.to_string()))?;

        // NOTE: We intentionally do NOT write to Queen's PTY here.
        // Queen spawns workers via HTTP API, so she already knows about them.
        // Injecting back would cause confusing "self-injection" in her terminal.

        // Emit event for UI
        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("coordination-message", &coord_message);
        }

        Ok(())
    }

    /// Notify Queen of worker status change
    #[allow(dead_code)]
    pub fn notify_queen_worker_status(
        &self,
        session_id: &str,
        queen_id: &str,
        worker_id: &str,
        status: &str,
    ) -> Result<(), InjectionError> {
        let message = format!(
            "[SYSTEM] Worker {} status changed: {}",
            format_agent_display(worker_id),
            status
        );

        let coord_message = CoordinationMessage::system(&format_agent_display(queen_id), &message);

        self.storage
            .append_coordination_log(session_id, &coord_message)
            .map_err(|e| InjectionError::StorageError(e.to_string()))?;

        // Emit event for UI
        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("coordination-message", &coord_message);
        }

        Ok(())
    }

    /// Log a system message to coordination log
    pub fn log_system_message(
        &self,
        session_id: &str,
        target: &str,
        message: &str,
    ) -> Result<(), InjectionError> {
        let coord_message = CoordinationMessage::system(target, message);

        self.storage
            .append_coordination_log(session_id, &coord_message)
            .map_err(|e| InjectionError::StorageError(e.to_string()))?;

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("coordination-message", &coord_message);
        }

        Ok(())
    }

    /// Log a progress message from an agent
    #[allow(dead_code)]
    pub fn log_progress(
        &self,
        session_id: &str,
        from_agent: &str,
        message: &str,
    ) -> Result<(), InjectionError> {
        let coord_message =
            CoordinationMessage::progress(&format_agent_display(from_agent), message);

        self.storage
            .append_coordination_log(session_id, &coord_message)
            .map_err(|e| InjectionError::StorageError(e.to_string()))?;

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("coordination-message", &coord_message);
        }

        Ok(())
    }

    /// Worker logs a message to coordination log
    /// Validates that the sender is a registered active worker
    pub fn worker_inject(
        &self,
        session_id: &str,
        worker_id: &str,
        message: &str,
    ) -> Result<(), InjectionError> {
        // Strong validation: verify worker_id belongs to session and is an active worker agent
        self.validate_agent_role(session_id, worker_id, "-worker-")?;

        // Log to coordination.log as a Progress message
        let coord_message =
            CoordinationMessage::progress(&format_agent_display(worker_id), message);

        self.storage
            .append_coordination_log(session_id, &coord_message)
            .map_err(|e| InjectionError::StorageError(e.to_string()))?;

        // Emit event for UI
        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("coordination-message", &coord_message);
        }

        Ok(())
    }

    /// Planner logs a message to coordination log
    /// Validates that the sender is a registered active planner
    pub fn planner_inject(
        &self,
        session_id: &str,
        planner_id: &str,
        message: &str,
    ) -> Result<(), InjectionError> {
        // Strong validation: verify planner_id belongs to session and is an active planner agent
        self.validate_agent_role(session_id, planner_id, "-planner-")?;

        // Log to coordination.log as a Progress message
        let coord_message =
            CoordinationMessage::progress(&format_agent_display(planner_id), message);

        self.storage
            .append_coordination_log(session_id, &coord_message)
            .map_err(|e| InjectionError::StorageError(e.to_string()))?;

        // Emit event for UI
        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("coordination-message", &coord_message);
        }

        Ok(())
    }

    /// Get the coordination log
    pub fn get_coordination_log(
        &self,
        session_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<CoordinationMessage>, InjectionError> {
        self.storage
            .read_coordination_log(session_id, limit)
            .map_err(|e| InjectionError::StorageError(e.to_string()))
    }

    /// Broadcast a message to all workers in a session
    #[allow(dead_code)]
    pub fn broadcast_to_workers(
        &self,
        session_id: &str,
        queen_id: &str,
        worker_ids: &[String],
        message: &str,
    ) -> Result<(), InjectionError> {
        for worker_id in worker_ids {
            self.queen_inject(session_id, queen_id, worker_id, message)?;
        }
        Ok(())
    }
}

/// Safely get a substring starting from a byte offset, handling UTF-8 boundaries
/// Returns the substring from the given byte offset, or an empty string if invalid
fn safe_slice_from(s: &str, byte_offset: usize) -> &str {
    if byte_offset >= s.len() {
        return "";
    }
    // Ensure we're at a valid UTF-8 char boundary
    if s.is_char_boundary(byte_offset) {
        &s[byte_offset..]
    } else {
        // Find the next valid char boundary
        let mut offset = byte_offset;
        while offset < s.len() && !s.is_char_boundary(offset) {
            offset += 1;
        }
        if offset < s.len() {
            &s[offset..]
        } else {
            ""
        }
    }
}

/// Safely get a substring up to a byte offset, handling UTF-8 boundaries
/// Returns the substring up to the given byte offset, or the whole string if invalid
fn safe_slice_to(s: &str, byte_offset: usize) -> &str {
    if byte_offset >= s.len() {
        return s;
    }
    // Ensure we're at a valid UTF-8 char boundary
    if s.is_char_boundary(byte_offset) {
        &s[..byte_offset]
    } else {
        // Find the previous valid char boundary
        let mut offset = byte_offset;
        while offset > 0 && !s.is_char_boundary(offset) {
            offset -= 1;
        }
        &s[..offset]
    }
}

/// Format agent ID for display (extract role from full ID)
fn format_agent_display(agent_id: &str) -> String {
    // IDs are like "session-id-queen" or "session-id-worker-1"
    // Extract the role part using safe UTF-8 slicing
    if agent_id.ends_with("-queen") {
        "QUEEN".to_string()
    } else if agent_id.contains("-worker-") {
        // Extract worker number using safe slicing
        if let Some(idx) = agent_id.rfind("-worker-") {
            let suffix = safe_slice_from(agent_id, idx + 8);
            if suffix.is_empty() {
                "WORKER".to_string()
            } else {
                format!("WORKER-{}", suffix)
            }
        } else {
            "WORKER".to_string()
        }
    } else if agent_id.contains("-planner-") {
        // Extract planner number using safe slicing
        if let Some(idx) = agent_id.rfind("-planner-") {
            let suffix = safe_slice_from(agent_id, idx + 9);
            if suffix.is_empty() {
                "PLANNER".to_string()
            } else if let Some(end_idx) = suffix.find('-') {
                format!("PLANNER-{}", safe_slice_to(suffix, end_idx))
            } else {
                format!("PLANNER-{}", suffix)
            }
        } else {
            "PLANNER".to_string()
        }
    } else {
        agent_id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_agent_display() {
        assert_eq!(format_agent_display("abc123-queen"), "QUEEN");
        assert_eq!(format_agent_display("abc123-worker-1"), "WORKER-1");
        assert_eq!(format_agent_display("abc123-worker-12"), "WORKER-12");
        assert_eq!(format_agent_display("abc123-planner-1"), "PLANNER-1");
        assert_eq!(
            format_agent_display("abc123-planner-1-worker-2"),
            "WORKER-2"
        );
    }

    #[test]
    fn test_worker_inject_validates_worker_id() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(temp_dir.path().to_path_buf()).unwrap();
        let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
        let manager = InjectionManager::new(pty_manager, storage);

        // Should fail for non-worker IDs
        let result = manager.worker_inject("test-session", "abc123-queen", "test message");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            InjectionError::NotAuthorized(_)
        ));

        let result = manager.worker_inject("test-session", "abc123-planner-1", "test message");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            InjectionError::NotAuthorized(_)
        ));
    }

    #[test]
    fn test_planner_inject_validates_planner_id() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(temp_dir.path().to_path_buf()).unwrap();
        let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
        let manager = InjectionManager::new(pty_manager, storage);

        // Should fail for non-planner IDs
        let result = manager.planner_inject("test-session", "abc123-queen", "test message");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            InjectionError::NotAuthorized(_)
        ));

        let result = manager.planner_inject("test-session", "abc123-worker-1", "test message");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            InjectionError::NotAuthorized(_)
        ));
    }
}
