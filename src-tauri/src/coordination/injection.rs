use std::sync::Arc;

use parking_lot::RwLock;
use tauri::{AppHandle, Emitter};
use thiserror::Error;

use crate::pty::PtyManager;
use crate::storage::SessionStorage;

use super::{CoordinationMessage, StateManager, WorkerStateInfo};

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

    /// Queen injects a message to a worker
    pub fn queen_inject(
        &self,
        session_id: &str,
        queen_id: &str,
        target_worker_id: &str,
        message: &str,
    ) -> Result<(), InjectionError> {
        // Validate sender is Queen (ID should end with -queen)
        if !queen_id.ends_with("-queen") {
            return Err(InjectionError::NotAuthorized(
                "Only Queen can inject messages to workers".to_string(),
            ));
        }
        if is_qa_worker_id(target_worker_id) {
            return Err(InjectionError::NotAuthorized(
                "Queen cannot inject directly into QA workers".to_string(),
            ));
        }

        // Log to coordination.log
        let coord_message = if target_worker_id.ends_with("-evaluator") {
            CoordinationMessage::milestone_ready(
                &format_agent_display(queen_id),
                &format_agent_display(target_worker_id),
                message,
            )
        } else {
            CoordinationMessage::task(
                &format_agent_display(queen_id),
                &format_agent_display(target_worker_id),
                message,
            )
        };

        self.storage
            .append_coordination_log(session_id, &coord_message)
            .map_err(|e| InjectionError::StorageError(e.to_string()))?;

        if target_worker_id.ends_with("-evaluator") {
            self.write_session_peer_message(session_id, |state| {
                state.write_milestone_ready(queen_id, target_worker_id, message)
            })?;
        }

        // Write to worker's PTY stdin
        self.write_to_agent(target_worker_id, message)?;

        // Emit event for UI
        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("coordination-message", &coord_message);
        }

        Ok(())
    }

    /// Evaluator injects a message to the Queen or its QA workers.
    pub fn evaluator_inject(
        &self,
        session_id: &str,
        evaluator_id: &str,
        target_agent_id: &str,
        message: &str,
    ) -> Result<(), InjectionError> {
        if !evaluator_id.ends_with("-evaluator") {
            return Err(InjectionError::NotAuthorized(
                "Only the Evaluator can inject peer QA messages".to_string(),
            ));
        }
        if is_queen_worker_id(target_agent_id) {
            return Err(InjectionError::NotAuthorized(
                "Evaluator cannot inject directly into Queen workers".to_string(),
            ));
        }

        let coord_message = if target_agent_id.ends_with("-queen") {
            CoordinationMessage::qa_verdict(
                &format_agent_display(evaluator_id),
                &format_agent_display(target_agent_id),
                message,
            )
        } else {
            CoordinationMessage::peer_feedback(
                &format_agent_display(evaluator_id),
                &format_agent_display(target_agent_id),
                message,
            )
        };

        self.storage
            .append_coordination_log(session_id, &coord_message)
            .map_err(|e| InjectionError::StorageError(e.to_string()))?;

        if target_agent_id.ends_with("-queen") {
            self.write_session_peer_message(session_id, |state| {
                state.write_qa_verdict(evaluator_id, target_agent_id, message)
            })?;
        } else {
            self.write_session_peer_message(session_id, |state| {
                state.write_evaluator_feedback(evaluator_id, target_agent_id, message)
            })?;
        }

        self.write_to_agent(target_agent_id, message)?;

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
        // Validate sender is Queen (ID should end with -queen)
        if !queen_id.ends_with("-queen") {
            return Err(InjectionError::NotAuthorized(
                "Only Queen can initiate branch switches".to_string(),
            ));
        }

        let message = format!("[BRANCH SWITCH] Switching all workers to branch: {}", branch);
        self.log_system_message(session_id, "ALL", &message)?;

        // Ctrl+C first to interrupt any running command
        let git_command = format!("\x03git switch {}", branch);

        let mut results = Vec::new();
        for worker_id in worker_ids {
            let result = self.write_to_agent(worker_id, &git_command);

            let status = if result.is_ok() { "initiated" } else { "failed" };
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

    /// Write a message to an agent's PTY and press Enter to submit
    pub fn write_to_agent(&self, agent_id: &str, message: &str) -> Result<(), InjectionError> {
        let pty_manager = self.pty_manager.read();

        // Strip any existing line endings first
        let clean_message = message.trim_end_matches(&['\r', '\n'][..]);

        tracing::info!("=== INJECTION START ===");
        tracing::info!("Target agent: {}", agent_id);
        tracing::info!("Message: {:?}", clean_message);
        tracing::info!("Message bytes: {:?}", clean_message.as_bytes());

        // Write the message content with Enter appended
        // On Windows ConPTY, Enter is typically just \r, but some apps need \n
        // We'll send both \r\n to maximize compatibility
        let message_with_enter = format!("{}\r\n", clean_message);

        tracing::info!("Full message with enter: {:?}", message_with_enter.as_bytes());

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
        let coord_message = CoordinationMessage::progress(&format_agent_display(from_agent), message);

        self.storage
            .append_coordination_log(session_id, &coord_message)
            .map_err(|e| InjectionError::StorageError(e.to_string()))?;

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

    fn write_session_peer_message<F>(&self, session_id: &str, write_fn: F) -> Result<(), InjectionError>
    where
        F: FnOnce(&StateManager) -> Result<(), super::StateError>,
    {
        let persisted = self
            .storage
            .load_session(session_id)
            .map_err(|e| InjectionError::StorageError(e.to_string()))?;
        let session_path = std::path::PathBuf::from(persisted.project_path)
            .join(".hive-manager")
            .join(session_id);
        let state = StateManager::new(session_path);
        write_fn(&state).map_err(|e| InjectionError::StorageError(e.to_string()))
    }

}

fn is_qa_worker_id(agent_id: &str) -> bool {
    agent_id.contains("-qa-worker-")
}

fn is_queen_worker_id(agent_id: &str) -> bool {
    agent_id.contains("-worker-") && !agent_id.contains("-qa-worker-")
}

/// Format agent ID for display (extract role from full ID)
fn format_agent_display(agent_id: &str) -> String {
    // IDs are like "session-id-queen" or "session-id-worker-1"
    // Extract the role part
    if agent_id.ends_with("-queen") {
        "QUEEN".to_string()
    } else if agent_id.ends_with("-evaluator") {
        "EVALUATOR".to_string()
    } else if agent_id.contains("-qa-worker-") {
        if let Some(idx) = agent_id.rfind("-qa-worker-") {
            format!("QA-WORKER-{}", &agent_id[idx + 11..])
        } else {
            "QA-WORKER".to_string()
        }
    } else if agent_id.contains("-worker-") {
        // Extract worker number
        if let Some(idx) = agent_id.rfind("-worker-") {
            format!("WORKER-{}", &agent_id[idx + 8..])
        } else {
            "WORKER".to_string()
        }
    } else if agent_id.contains("-planner-") {
        // Extract planner number
        if let Some(idx) = agent_id.rfind("-planner-") {
            let suffix = &agent_id[idx + 9..];
            if let Some(end_idx) = suffix.find('-') {
                format!("PLANNER-{}", &suffix[..end_idx])
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
        assert_eq!(format_agent_display("abc123-evaluator"), "EVALUATOR");
        assert_eq!(format_agent_display("abc123-qa-worker-3"), "QA-WORKER-3");
        assert_eq!(format_agent_display("abc123-worker-1"), "WORKER-1");
        assert_eq!(format_agent_display("abc123-worker-12"), "WORKER-12");
        assert_eq!(format_agent_display("abc123-planner-1"), "PLANNER-1");
        assert_eq!(format_agent_display("abc123-planner-1-worker-2"), "WORKER-2");
    }

    #[test]
    fn test_role_boundaries() {
        assert!(is_qa_worker_id("abc123-qa-worker-2"));
        assert!(!is_qa_worker_id("abc123-worker-2"));
        assert!(is_queen_worker_id("abc123-worker-2"));
        assert!(!is_queen_worker_id("abc123-qa-worker-2"));
        assert!(!is_queen_worker_id("abc123-evaluator"));
    }
}
