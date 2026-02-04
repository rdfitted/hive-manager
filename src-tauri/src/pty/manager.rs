use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use parking_lot::RwLock;
use tauri::{AppHandle, Emitter};
use serde::Serialize;

use super::session::{AgentRole, AgentStatus, PtyError, PtySession, read_from_reader};

#[derive(Clone, Serialize)]
pub struct PtyOutput {
    pub id: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Serialize)]
pub struct PtyStatusChange {
    pub id: String,
    pub status: AgentStatus,
}

pub struct PtyManager {
    sessions: Arc<RwLock<HashMap<String, Arc<PtySession>>>>,
    app_handle: Option<AppHandle>,
}

// Explicitly implement Send + Sync
unsafe impl Send for PtyManager {}
unsafe impl Sync for PtyManager {}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            app_handle: None,
        }
    }

    pub fn set_app_handle(&mut self, handle: AppHandle) {
        self.app_handle = Some(handle);
    }

    pub fn create_session(
        &self,
        id: String,
        role: AgentRole,
        command: &str,
        args: &[&str],
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<String, PtyError> {
        let session = Arc::new(PtySession::new(id.clone(), role, command, args, cwd, cols, rows)?);

        // Insert session BEFORE spawning reader thread (fixes race condition)
        {
            let mut sessions = self.sessions.write();
            sessions.insert(id.clone(), Arc::clone(&session));
        }

        // Start the output reader thread
        if let Some(ref app_handle) = self.app_handle {
            let session_clone = Arc::clone(&session);
            let app_handle_clone = app_handle.clone();
            let id_clone = id.clone();
            let sessions_ref = Arc::clone(&self.sessions);

            thread::spawn(move || {
                let reader = session_clone.get_reader();
                let mut buf = [0u8; 4096];

                loop {
                    // Check if session still exists
                    {
                        let sessions_read = sessions_ref.read();
                        if !sessions_read.contains_key(&id_clone) {
                            break;
                        }
                    }

                    let bytes_read = match read_from_reader(&reader, &mut buf) {
                        Ok(0) => {
                            // EOF - process exited
                            break;
                        }
                        Ok(n) => n,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        Err(_) => break,
                    };

                    if bytes_read > 0 {
                        tracing::debug!("PTY {} read {} bytes", id_clone, bytes_read);
                        let output = PtyOutput {
                            id: id_clone.clone(),
                            data: buf[..bytes_read].to_vec(),
                        };
                        if let Err(e) = app_handle_clone.emit("pty-output", output) {
                            tracing::error!("Failed to emit pty-output: {}", e);
                        }
                    }
                }

                // Session ended - emit status change
                let _ = app_handle_clone.emit("pty-status", PtyStatusChange {
                    id: id_clone,
                    status: AgentStatus::Completed,
                });
            });
        }

        // Session already inserted before thread spawn (see above)

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("pty-status", PtyStatusChange {
                id: id.clone(),
                status: AgentStatus::Running,
            });
        }

        Ok(id)
    }

    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), PtyError> {
        tracing::debug!("PtyManager::write called for session: {}", id);
        let sessions = self.sessions.read();
        tracing::debug!("Available sessions: {:?}", sessions.keys().collect::<Vec<_>>());
        let session = sessions.get(id).ok_or_else(|| {
            tracing::error!("PTY session not found: {}", id);
            PtyError::NotFound(id.to_string())
        })?;
        tracing::debug!("Found session {}, calling write", id);
        session.write(data)
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), PtyError> {
        let sessions = self.sessions.read();
        let session = sessions.get(id).ok_or_else(|| PtyError::NotFound(id.to_string()))?;
        tracing::debug!("Resizing PTY {} to {}x{}", id, cols, rows);
        session.resize(cols, rows)
    }

    pub fn kill(&self, id: &str) -> Result<(), PtyError> {
        let sessions = self.sessions.read();
        if let Some(session) = sessions.get(id) {
            session.kill()?;
        }
        Ok(())
    }

    pub fn get_status(&self, id: &str) -> Option<AgentStatus> {
        let sessions = self.sessions.read();
        sessions.get(id).map(|s| s.status.read().clone())
    }

    pub fn list_sessions(&self) -> Vec<(String, AgentRole, AgentStatus)> {
        let sessions = self.sessions.read();
        sessions
            .iter()
            .map(|(id, session)| (id.clone(), session.role.clone(), session.status.read().clone()))
            .collect()
    }
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}
