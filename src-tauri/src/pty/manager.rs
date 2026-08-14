use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;

use super::session::{AgentRole, AgentStatus, PtyError, PtySession, read_from_reader};
use crate::cli::agent_store;
use crate::tauri_shim::{AppHandle, Emitter};

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

/// Minimum spacing between consecutive codex spawns (#207 fix 3).
///
/// Per-agent store isolation removes the shared-SQLite lock by construction; this gap is
/// the defense-in-depth layer for the startup burst itself, where several codex processes
/// initializing at once amplified contention in the field failure. Kept short — it bounds
/// concurrent startups, it does not serialize the workers' actual runs.
#[cfg(not(test))]
const CODEX_SPAWN_GAP: Duration = Duration::from_millis(750);
#[cfg(test)]
const CODEX_SPAWN_GAP: Duration = Duration::from_millis(10);

pub struct PtyManager {
    sessions: Arc<RwLock<HashMap<String, Arc<PtySession>>>>,
    /// Serialize create/kill so a same-id kill cannot pass between process spawn and
    /// insertion, and a duplicate create cannot replace a still-live process handle.
    lifecycle: Mutex<()>,
    /// When the most recent codex spawn happened. Held on its own mutex so a spawn
    /// waiting out the stagger gap does not block kill/status/list.
    last_codex_spawn: Mutex<Option<std::time::Instant>>,
    app_handle: Option<AppHandle>,
}

// Explicitly implement Send + Sync
unsafe impl Send for PtyManager {}
unsafe impl Sync for PtyManager {}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            lifecycle: Mutex::new(()),
            last_codex_spawn: Mutex::new(None),
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
        // #207 fix 3: stagger codex starts. Taken before the lifecycle lock so a spawn
        // waiting out the gap does not stall kill/create of unrelated agents.
        if agent_store::has_contended_store(command) {
            let mut last = self.last_codex_spawn.lock();
            if let Some(previous) = *last {
                let since = previous.elapsed();
                if since < CODEX_SPAWN_GAP {
                    thread::sleep(CODEX_SPAWN_GAP - since);
                }
            }
            *last = Some(std::time::Instant::now());
        }

        let _lifecycle_guard = self.lifecycle.lock();
        let existing = { self.sessions.read().get(&id).cloned() };
        if let Some(existing) = existing {
            if existing.is_alive() {
                return Err(PtyError::CreateError(format!(
                    "PTY session already exists: {id}"
                )));
            }

            // Evaluator/prince respawns intentionally reuse their stable ID after exit.
            // Reap that dead handle, while still rejecting a live same-ID replacement.
            let _ = existing.kill();
            let mut sessions = self.sessions.write();
            if sessions
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &existing))
            {
                sessions.remove(&id);
            }
        }

        // #207: give this agent its own CLI state store before spawning. Codex keeps its
        // SQLite state in one directory shared by every process that inherits the
        // operator's environment, so workers started together race that lock and the
        // losers die during startup with "database is locked". This is the one choke
        // point every agent spawn passes through, which is why the flags are injected
        // here rather than at each of the caller sites.
        //
        // Best-effort by design: if the store directory cannot be created we spawn on the
        // shared store anyway rather than turn a contention mitigation into a hard spawn
        // failure. The warning is the signal that isolation was lost.
        let store_dir = agent_store::store_dir_for(&id);
        let isolation_flags = agent_store::isolation_args(command, &store_dir);
        let isolation_flags = if isolation_flags.is_empty() {
            isolation_flags
        } else {
            match agent_store::ensure_store_dir(&store_dir) {
                Ok(()) => isolation_flags,
                Err(error) => {
                    tracing::warn!(
                        agent = %id,
                        store = %store_dir.display(),
                        %error,
                        "could not create a private CLI state store; falling back to the shared store"
                    );
                    Vec::new()
                }
            }
        };

        // Isolation flags go in front so the CLI's trailing positional prompt stays last.
        let effective_args: Vec<&str> = isolation_flags
            .iter()
            .map(String::as_str)
            .chain(args.iter().copied())
            .collect();

        let session = Arc::new(PtySession::new(
            id.clone(),
            role,
            command,
            &effective_args,
            cwd,
            cols,
            rows,
        )?);

        // Insert session BEFORE spawning reader thread (fixes race condition)
        {
            let mut sessions = self.sessions.write();
            sessions.insert(id.clone(), Arc::clone(&session));
        }

        // Start the output reader thread.
        //
        // #207: this runs whether or not a UI is attached. It used to be gated on
        // `app_handle`, so in headless HTTP mode — and in every test — nothing ever read
        // the PTY and a CLI that died during startup had its error text discarded by the
        // OS, leaving no way to explain the failure. The bytes now always feed the
        // session's rolling diagnostic buffer; emitting to the UI is the optional part.
        {
            let session_clone = Arc::clone(&session);
            let app_handle_clone = self.app_handle.clone();
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
                        session_clone.record_output(&buf[..bytes_read]);

                        if let Some(ref app_handle) = app_handle_clone {
                            let output = PtyOutput {
                                id: id_clone.clone(),
                                data: buf[..bytes_read].to_vec(),
                            };
                            if let Err(e) = app_handle.emit("pty-output", output) {
                                tracing::error!("Failed to emit pty-output: {}", e);
                            }
                        }
                    }
                }

                // Session ended - emit status change
                if let Some(ref app_handle) = app_handle_clone {
                    let _ = app_handle.emit("pty-status", PtyStatusChange {
                        id: id_clone,
                        status: AgentStatus::Completed,
                    });
                }
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

    /// Write a payload and then a discrete bare Enter to submit it.
    pub fn submit(&self, id: &str, data: &[u8]) -> Result<(), PtyError> {
        tracing::debug!("PtyManager::submit called for session: {}", id);
        let sessions = self.sessions.read();
        let session = sessions
            .get(id)
            .ok_or_else(|| PtyError::NotFound(id.to_string()))?;
        session.submit(data)
    }

    /// Write with bracketed paste mode wrapping for large pastes
    pub fn write_bracketed(&self, id: &str, data: &[u8]) -> Result<(), PtyError> {
        tracing::debug!("PtyManager::write_bracketed called for session: {} ({} bytes)", id, data.len());
        let sessions = self.sessions.read();
        let session = sessions.get(id).ok_or_else(|| PtyError::NotFound(id.to_string()))?;
        session.write_bracketed(data)
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), PtyError> {
        let sessions = self.sessions.read();
        let session = sessions.get(id).ok_or_else(|| PtyError::NotFound(id.to_string()))?;
        tracing::debug!("Resizing PTY {} to {}x{}", id, cols, rows);
        session.resize(cols, rows)
    }

    pub fn kill(&self, id: &str) -> Result<(), PtyError> {
        let _lifecycle_guard = self.lifecycle.lock();
        let session = self.sessions.read().get(id).cloned();
        if let Some(session) = session {
            if let Err(error) = session.kill() {
                // Some PTY backends report an error when killing a process that already
                // exited. Drop that dead handle, but retain genuinely live failures so a
                // later cleanup attempt can retry them.
                if session.is_alive() {
                    return Err(error);
                }
            }

            // Remove only the exact session we killed. This avoids retaining its process
            // handle without deleting a same-id replacement created concurrently.
            let mut sessions = self.sessions.write();
            if sessions
                .get(id)
                .is_some_and(|current| Arc::ptr_eq(current, &session))
            {
                sessions.remove(id);
            }
        }
        Ok(())
    }

    pub fn get_status(&self, id: &str) -> Option<AgentStatus> {
        let sessions = self.sessions.read();
        sessions.get(id).map(|s| s.status.read().clone())
    }

    pub fn is_alive(&self, id: &str) -> bool {
        let sessions = self.sessions.read();
        sessions
            .get(id)
            .map(|session| session.is_alive())
            .unwrap_or(false)
    }

    /// Retained tail of an agent's output (#207).
    ///
    /// A PTY merges stdout and stderr, so for a CLI that dies during startup this is the
    /// only place its error text survives — it is what turns "worker failed to start"
    /// into "database is locked".
    pub fn recent_output(&self, id: &str) -> Option<String> {
        let sessions = self.sessions.read();
        sessions.get(id).map(|session| session.recent_output())
    }

    /// Test hook: the argv a session was actually spawned with, so store isolation can be
    /// asserted at the spawn rather than only in the helper that computes the flags.
    #[cfg(all(test, windows))]
    pub fn spawn_args_for_test(&self, id: &str) -> Option<Vec<String>> {
        let sessions = self.sessions.read();
        sessions.get(id).map(|session| session.args().to_vec())
    }

    #[cfg(all(test, windows))]
    pub fn write_records_for_test(&self, id: &str) -> Option<Vec<Vec<u8>>> {
        let sessions = self.sessions.read();
        sessions.get(id).map(|session| session.write_records())
    }

    pub fn list_sessions(&self) -> Vec<(String, AgentRole, AgentStatus)> {
        let sessions = self.sessions.read();
        sessions
            .iter()
            .filter(|(_, session)| !matches!(&session.role, AgentRole::ScratchShell))
            .map(|(id, session)| (id.clone(), session.role.clone(), session.status.read().clone()))
            .collect()
    }
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}

// The stub PTY session records its argv, so these run only where the stub is active.
#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn worker_role() -> AgentRole {
        AgentRole::Worker {
            index: 1,
            parent: None,
        }
    }

    /// #207: the spawn itself — not just the helper that computes the flags — must carry
    /// the private store, and the flags must precede the positional prompt.
    #[test]
    fn codex_spawns_carry_a_private_sqlite_home() {
        let manager = PtyManager::new();
        manager
            .create_session(
                "iso-codex-agent".to_string(),
                worker_role(),
                "codex",
                &["exec", "do the thing"],
                None,
                80,
                24,
            )
            .unwrap();

        let args = manager.spawn_args_for_test("iso-codex-agent").unwrap();
        assert_eq!(args[0], "-c");
        assert!(args[1].starts_with("sqlite_home="), "got: {}", args[1]);
        assert!(args[1].contains("iso-codex-agent"), "got: {}", args[1]);
        assert_eq!(
            args.last().map(String::as_str),
            Some("do the thing"),
            "the positional prompt must stay last"
        );
    }

    /// #207: only the CLI with the shared-store problem is touched.
    #[test]
    fn non_codex_spawns_keep_their_argv_untouched() {
        let manager = PtyManager::new();
        manager
            .create_session(
                "plain-claude-agent".to_string(),
                worker_role(),
                "claude",
                &["-p", "hello"],
                None,
                80,
                24,
            )
            .unwrap();

        let args = manager.spawn_args_for_test("plain-claude-agent").unwrap();
        assert_eq!(args, vec!["-p".to_string(), "hello".to_string()]);
    }

    #[test]
    fn submit_writes_payload_then_bare_enter_separately() {
        let manager = PtyManager::new();
        manager
            .create_session(
                "submit-agent".to_string(),
                worker_role(),
                "claude",
                &[],
                None,
                80,
                24,
            )
            .unwrap();

        manager.submit("submit-agent", b"hello").unwrap();

        let writes = manager.write_records_for_test("submit-agent").unwrap();
        assert_eq!(writes, vec![b"hello".to_vec(), b"\r".to_vec()]);
        assert!(!writes[0].ends_with(b"\r"));
        assert!(!writes[0].ends_with(b"\n"));
        assert_eq!(writes[1], b"\r");
        assert!(!writes[1].contains(&b'\n'));
    }

    /// #207: a session that dies during startup reports dead and keeps its final output
    /// available for diagnostics until it is killed/removed.
    #[test]
    fn a_startup_death_is_visible_and_its_output_survives() {
        let manager = PtyManager::new();
        manager
            .create_session(
                "doa-agent".to_string(),
                worker_role(),
                "claude",
                &["--stub-die-on-start"],
                None,
                80,
                24,
            )
            .unwrap();

        assert!(!manager.is_alive("doa-agent"));
        let output = manager.recent_output("doa-agent").unwrap();
        assert!(output.contains("database is locked"), "got: {output}");

        manager.kill("doa-agent").unwrap();
        assert!(manager.recent_output("doa-agent").is_none());
    }
}
