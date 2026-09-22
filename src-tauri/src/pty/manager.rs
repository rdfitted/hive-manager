use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};
use base64::Engine;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use super::output_ring::{clamp_replay_capacity, OutputSnapshot, DEFAULT_REPLAY_CAPACITY};
use super::session::{AgentRole, AgentStatus, PtyError, PtySession, read_from_reader};
use crate::adapters::{pty_submit_policy, PtySubmitResult};
use crate::cli::agent_store;
use crate::tauri_shim::{AppHandle, Emitter};

/// Environment exported to managed agent processes.
///
/// Agent ids are namespaced by their session UUID. Scratch PTYs and malformed ids
/// deliberately receive no partial or guessed identity.
fn agent_identity_env(id: &str, role: &AgentRole) -> Vec<(String, String)> {
    if id.starts_with("scratch:") {
        return Vec::new();
    }

    let Some(session_id) = id.get(..36) else {
        return Vec::new();
    };
    if id.as_bytes().get(36) != Some(&b'-')
        || id.get(37..).map_or(true, str::is_empty)
        || uuid::Uuid::parse_str(session_id).is_err()
    {
        return Vec::new();
    }

    let role = match role {
        AgentRole::MasterPlanner => "master-planner",
        AgentRole::Queen => "queen",
        AgentRole::Planner { .. } => "planner",
        AgentRole::Worker { .. } => "worker",
        AgentRole::Fusion { .. } => "fusion",
        AgentRole::Judge { .. } => "judge",
        AgentRole::Evaluator => "evaluator",
        AgentRole::QaWorker { .. } => "qa-worker",
        AgentRole::Prince => "prince",
        AgentRole::ScratchShell => "scratch-shell",
    };

    vec![
        ("HIVE_SESSION_ID".to_string(), session_id.to_string()),
        ("HIVE_AGENT_ID".to_string(), id.to_string()),
        ("HIVE_ROLE".to_string(), role.to_string()),
    ]
}

/// One coalesced run of child output, delivered on the agent's own event name (#289).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PtyOutput {
    pub id: String,
    /// Absolute byte offset of the first byte in `data` (#287). A pane that replayed a
    /// snapshot applies only chunks whose offset is at or past the snapshot's end.
    pub offset: u64,
    /// Standard base64 with padding. About 1.33× the raw bytes; the previous
    /// `Vec<u8>` serialized as a JSON number array at roughly 4×.
    pub data: String,
}

impl PtyOutput {
    pub fn new(id: impl Into<String>, offset: u64, bytes: &[u8]) -> Self {
        Self {
            id: id.into(),
            offset,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }
}

/// The retained history of one agent's PTY, ready to write into a fresh terminal (#287).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PtySnapshot {
    pub id: String,
    pub offset_start: u64,
    pub offset_end: u64,
    /// Standard base64 with padding, aligned so it starts at a parseable boundary.
    pub data: String,
}

impl PtySnapshot {
    pub fn from_snapshot(id: impl Into<String>, snapshot: OutputSnapshot) -> Self {
        Self {
            id: id.into(),
            offset_start: snapshot.offset_start,
            offset_end: snapshot.offset_end,
            data: base64::engine::general_purpose::STANDARD.encode(&snapshot.data),
        }
    }
}

#[derive(Clone, Serialize)]
pub struct PtyStatusChange {
    pub id: String,
    pub status: AgentStatus,
}

/// Event carrying agent lifecycle transitions. Low volume, so it stays a single name.
pub const PTY_STATUS_EVENT: &str = "pty-status";

/// The event name a pane subscribes to for one agent's output.
///
/// Tauri evaluates an emit only in webviews that hold a JS listener for that exact name,
/// so output for agents nobody has mounted never crosses the IPC bridge, and a pane no
/// longer filters every other agent's bytes in JavaScript. Tauri permits alphanumerics,
/// `-`, `/`, `:` and `_` in event names; anything else is mapped to `_` here and by the
/// frontend's `ptyOutputEventName`, which must stay in lockstep.
pub fn pty_output_event_name(id: &str) -> String {
    let mut name = String::with_capacity("pty-output:".len() + id.len());
    name.push_str("pty-output:");
    name.extend(id.chars().map(|c| {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '/' | ':' | '_') {
            c
        } else {
            '_'
        }
    }));
    name
}

/// How long the emitter waits for more reader chunks before flushing a batch. Bounds
/// added latency; TUIs write many tiny chunks per frame, so one event per frame is the
/// common outcome.
const COALESCE_WINDOW: Duration = Duration::from_millis(8);
/// Flush early once this many bytes are buffered, so a firehose cannot grow one event
/// without bound.
const COALESCE_MAX_BYTES: usize = 256 * 1024;
/// How often the emitter logs its throughput counters.
const EMITTER_STATS_INTERVAL: Duration = Duration::from_secs(5);
/// Upper bound on reader chunks queued for the emitter, shared by every PTY. `send`
/// blocks when the queue is full, so a webview that stops draining events applies
/// backpressure to the PTY children exactly as the previous synchronous emit did,
/// instead of buffering their output without limit. At 4 KB reads this caps the
/// in-flight backlog near 4 MB.
const EMITTER_QUEUE_CAPACITY: usize = 1024;

/// Reader-thread → emitter-thread messages. Data and exit travel the same channel so an
/// agent's final bytes are always emitted before its `Completed` status.
enum EmitterMessage {
    Output { id: String, offset: u64, bytes: Vec<u8> },
    Exited { id: String },
}

impl EmitterMessage {
    fn len(&self) -> usize {
        match self {
            EmitterMessage::Output { bytes, .. } => bytes.len(),
            EmitterMessage::Exited { .. } => 0,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CoalescedEvent {
    Output { id: String, offset: u64, bytes: Vec<u8> },
    Exited { id: String },
}

/// Merge a batch of reader messages into at most one output event per contiguous run
/// per agent, preserving per-agent order and keeping data ahead of its exit marker.
fn coalesce(messages: Vec<EmitterMessage>) -> Vec<CoalescedEvent> {
    let mut events: Vec<CoalescedEvent> = Vec::new();
    let mut open: HashMap<String, usize> = HashMap::new();

    for message in messages {
        match message {
            EmitterMessage::Output { id, offset, bytes } => {
                if let Some(&index) = open.get(&id) {
                    if let CoalescedEvent::Output {
                        offset: start,
                        bytes: buffer,
                        ..
                    } = &mut events[index]
                    {
                        if *start + buffer.len() as u64 == offset {
                            buffer.extend_from_slice(&bytes);
                            continue;
                        }
                    }
                }
                open.insert(id.clone(), events.len());
                events.push(CoalescedEvent::Output { id, offset, bytes });
            }
            EmitterMessage::Exited { id } => {
                open.remove(&id);
                events.push(CoalescedEvent::Exited { id });
            }
        }
    }

    events
}

/// Throughput counters for the UI transport (#289 "measure first").
#[derive(Debug, Default)]
pub struct EmitterStats {
    /// Chunks read from PTYs, before coalescing.
    pub raw_chunks: AtomicU64,
    /// Events actually emitted to the webview.
    pub events: AtomicU64,
    /// Raw bytes carried by those events (before base64 expansion).
    pub bytes: AtomicU64,
}

impl EmitterStats {
    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.raw_chunks.load(Ordering::Relaxed),
            self.events.load(Ordering::Relaxed),
            self.bytes.load(Ordering::Relaxed),
        )
    }
}

fn spawn_emitter(
    app_handle: AppHandle,
    receiver: mpsc::Receiver<EmitterMessage>,
    stats: Arc<EmitterStats>,
) {
    let spawned = thread::Builder::new()
        .name("pty-emitter".to_string())
        .spawn(move || {
            let mut pending: Vec<EmitterMessage> = Vec::new();
            let mut window_events = 0u64;
            let mut window_bytes = 0u64;
            let mut window_started = Instant::now();

            loop {
                let first = match receiver.recv() {
                    Ok(message) => message,
                    Err(_) => break,
                };
                let mut buffered = first.len();
                pending.push(first);

                let deadline = Instant::now() + COALESCE_WINDOW;
                let mut disconnected = false;
                while buffered < COALESCE_MAX_BYTES {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    match receiver.recv_timeout(deadline - now) {
                        Ok(message) => {
                            buffered += message.len();
                            pending.push(message);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }

                for event in coalesce(std::mem::take(&mut pending)) {
                    match event {
                        CoalescedEvent::Output { id, offset, bytes } => {
                            stats.events.fetch_add(1, Ordering::Relaxed);
                            stats.bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                            window_events += 1;
                            window_bytes += bytes.len() as u64;
                            let payload = PtyOutput::new(&id, offset, &bytes);
                            if let Err(e) = app_handle.emit(&pty_output_event_name(&id), payload) {
                                tracing::error!("Failed to emit pty output for {id}: {e}");
                            }
                        }
                        CoalescedEvent::Exited { id } => {
                            let _ = app_handle.emit(
                                PTY_STATUS_EVENT,
                                PtyStatusChange {
                                    id,
                                    status: AgentStatus::Completed,
                                },
                            );
                        }
                    }
                }

                let elapsed = window_started.elapsed();
                if elapsed >= EMITTER_STATS_INTERVAL {
                    tracing::debug!(
                        events = window_events,
                        bytes = window_bytes,
                        secs = elapsed.as_secs_f32(),
                        "pty emitter throughput"
                    );
                    window_events = 0;
                    window_bytes = 0;
                    window_started = Instant::now();
                }

                if disconnected {
                    break;
                }
            }
        });

    if let Err(e) = spawned {
        tracing::error!("Failed to spawn the pty emitter thread: {e}");
    }
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
    /// Reader threads hand their chunks to the single emitter thread through this
    /// bounded queue. `None` until a UI attaches, so headless mode records output
    /// without emitting.
    emit_tx: Option<mpsc::SyncSender<EmitterMessage>>,
    emitter_stats: Arc<EmitterStats>,
    /// Bytes of history each new session's ring retains (#287). Clamped on set.
    replay_capacity: usize,
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
            emit_tx: None,
            emitter_stats: Arc::new(EmitterStats::default()),
            replay_capacity: DEFAULT_REPLAY_CAPACITY,
        }
    }

    pub fn set_app_handle(&mut self, handle: AppHandle) {
        if self.emit_tx.is_none() {
            let (sender, receiver) = mpsc::sync_channel(EMITTER_QUEUE_CAPACITY);
            spawn_emitter(handle.clone(), receiver, Arc::clone(&self.emitter_stats));
            self.emit_tx = Some(sender);
        }
        self.app_handle = Some(handle);
    }

    /// Set how much history each *subsequently created* session retains. Existing
    /// sessions keep the ring they were created with.
    pub fn set_replay_capacity(&mut self, bytes: usize) {
        self.replay_capacity = clamp_replay_capacity(bytes);
    }

    pub fn replay_capacity(&self) -> usize {
        self.replay_capacity
    }

    pub fn emitter_stats(&self) -> Arc<EmitterStats> {
        Arc::clone(&self.emitter_stats)
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

        let identity_env = agent_identity_env(&id, &role);
        let session = Arc::new(PtySession::new(
            id.clone(),
            role,
            command,
            pty_submit_policy(command),
            &identity_env,
            &effective_args,
            cwd,
            cols,
            rows,
            self.replay_capacity,
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
        // OS, leaving no way to explain the failure. The bytes always feed the session's
        // output ring; handing them to the emitter thread is the optional part (#289).
        {
            let session_clone = Arc::clone(&session);
            let emit_tx = self.emit_tx.clone();
            let stats = Arc::clone(&self.emitter_stats);
            let id_clone = id.clone();
            let sessions_ref = Arc::clone(&self.sessions);

            let spawned = thread::Builder::new()
                .name(format!("pty-reader-{id}"))
                .spawn(move || {
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
                            // Record first: the offset returned here is what sequences the
                            // emitted chunk against snapshots taken by fresh panes (#287).
                            let offset = session_clone.record_output(&buf[..bytes_read]);
                            stats.raw_chunks.fetch_add(1, Ordering::Relaxed);

                            // Blocks while the bounded queue is full: backpressure reaches
                            // the child through the PTY pipe rather than growing memory.
                            if let Some(tx) = &emit_tx {
                                let _ = tx.send(EmitterMessage::Output {
                                    id: id_clone.clone(),
                                    offset,
                                    bytes: buf[..bytes_read].to_vec(),
                                });
                            }
                        }
                    }

                    // Session ended. The exit marker rides the same channel as the data so
                    // the emitter reports Completed only after the final bytes went out.
                    if let Some(tx) = &emit_tx {
                        let _ = tx.send(EmitterMessage::Exited { id: id_clone });
                    }
                });

            if let Err(e) = spawned {
                tracing::error!("Failed to spawn the pty reader thread for {id}: {e}");
            }
        }

        // Session already inserted before thread spawn (see above)

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(PTY_STATUS_EVENT, PtyStatusChange {
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

    /// Deliver a payload as a bracketed paste and then a discrete bare Enter to submit it.
    pub fn submit(&self, id: &str, data: &[u8]) -> Result<PtySubmitResult, PtyError> {
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

    /// The whole retained history of an agent's PTY, for a freshly mounted pane (#287).
    /// `None` when no PTY exists for `id` (never spawned, or already reaped).
    pub fn snapshot(&self, id: &str) -> Option<PtySnapshot> {
        let sessions = self.sessions.read();
        sessions
            .get(id)
            .map(|session| PtySnapshot::from_snapshot(id, session.snapshot()))
    }

    /// Test hook: the argv a session was actually spawned with, so store isolation can be
    /// asserted at the spawn rather than only in the helper that computes the flags.
    #[cfg(all(test, windows))]
    pub fn spawn_args_for_test(&self, id: &str) -> Option<Vec<String>> {
        let sessions = self.sessions.read();
        sessions.get(id).map(|session| session.args().to_vec())
    }

    #[cfg(all(test, windows))]
    pub fn spawn_env_for_test(&self, id: &str) -> Option<Vec<(String, String)>> {
        let sessions = self.sessions.read();
        sessions.get(id).map(|session| session.env().to_vec())
    }

    #[cfg(all(test, windows))]
    pub fn write_records_for_test(&self, id: &str) -> Option<Vec<Vec<u8>>> {
        let sessions = self.sessions.read();
        sessions.get(id).map(|session| session.write_records())
    }

    /// Test hook: append synthetic child output to a session's diagnostic ring, so the
    /// post-submit confirmation observer (#256) can be exercised against a stub PTY that
    /// has no real child to produce output.
    #[cfg(all(test, windows))]
    pub fn record_output_for_test(&self, id: &str, bytes: &[u8]) {
        let sessions = self.sessions.read();
        if let Some(session) = sessions.get(id) {
            session.record_output(bytes);
        }
    }

    #[cfg(all(test, windows))]
    pub fn submit_policy_for_test(
        &self,
        id: &str,
    ) -> Option<crate::adapters::PtySubmitPolicy> {
        let sessions = self.sessions.read();
        sessions
            .get(id)
            .map(|session| session.submit_policy_for_test())
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
    use crate::pty::{RoleDefinitionRef, WorkerRole};

    fn worker_role() -> AgentRole {
        AgentRole::Worker {
            index: 1,
            parent: None,
        }
    }

    const SESSION_ID: &str = "3dc17391-88d6-46c7-93ba-037f37d36894";

    #[test]
    fn identity_env_uses_stable_labels_for_every_agent_role() {
        let roles = [
            (AgentRole::MasterPlanner, "master-planner"),
            (AgentRole::Queen, "queen"),
            (AgentRole::Planner { index: 1 }, "planner"),
            (
                AgentRole::Worker {
                    index: 1,
                    parent: None,
                },
                "worker",
            ),
            (
                AgentRole::Fusion {
                    variant: "alpha".to_string(),
                },
                "fusion",
            ),
            (
                AgentRole::Judge {
                    session_id: SESSION_ID.to_string(),
                },
                "judge",
            ),
            (AgentRole::Evaluator, "evaluator"),
            (
                AgentRole::QaWorker {
                    index: 1,
                    parent: None,
                },
                "qa-worker",
            ),
            (AgentRole::Prince, "prince"),
            (AgentRole::ScratchShell, "scratch-shell"),
        ];

        for (index, (role, expected_role)) in roles.into_iter().enumerate() {
            let id = format!("{SESSION_ID}-test-{index}");
            assert_eq!(
                agent_identity_env(&id, &role),
                vec![
                    ("HIVE_SESSION_ID".to_string(), SESSION_ID.to_string()),
                    ("HIVE_AGENT_ID".to_string(), id),
                    ("HIVE_ROLE".to_string(), expected_role.to_string()),
                ]
            );
        }
    }

    #[test]
    fn identity_env_accepts_every_production_suffix_shape() {
        for suffix in [
            "master-planner",
            "queen",
            "planner-1",
            "worker-1",
            "fusion-1",
            "debate-1-r2",
            "judge",
            "evaluator",
            "retro-evaluator",
            "qa-worker-1",
            "prince",
        ] {
            let id = format!("{SESSION_ID}-{suffix}");
            let env = agent_identity_env(&id, &AgentRole::Queen);
            assert_eq!(env[0].1, SESSION_ID, "suffix: {suffix}");
            assert_eq!(env[1].1, id, "suffix: {suffix}");
        }
    }

    #[test]
    fn identity_env_fails_open_for_scratch_and_malformed_ids() {
        let invalid_ids = [
            format!("scratch:{SESSION_ID}:terminal"),
            "not-a-uuid-worker".to_string(),
            SESSION_ID.to_string(),
            format!("{SESSION_ID}-"),
            "éééééééééééééééééééééééééééééééééééé-worker".to_string(),
        ];
        for id in invalid_ids {
            assert!(
                agent_identity_env(&id, &AgentRole::ScratchShell).is_empty(),
                "id: {id}"
            );
        }
    }

    #[test]
    fn create_session_applies_identity_env_at_the_pty_choke_point() {
        let manager = PtyManager::new();
        let id = format!("{SESSION_ID}-worker-7");
        manager
            .create_session(
                id.clone(),
                AgentRole::Worker {
                    index: 7,
                    parent: Some(format!("{SESSION_ID}-queen")),
                },
                "claude",
                &[],
                None,
                80,
                24,
            )
            .unwrap();

        assert_eq!(
            manager.spawn_env_for_test(&id).unwrap(),
            vec![
                ("HIVE_SESSION_ID".to_string(), SESSION_ID.to_string()),
                ("HIVE_AGENT_ID".to_string(), id),
                ("HIVE_ROLE".to_string(), "worker".to_string()),
            ]
        );
    }

    fn public_struct_fields(source: &str, struct_name: &str) -> Vec<String> {
        let marker = format!("pub struct {struct_name} {{");
        let body = source
            .split_once(&marker)
            .unwrap_or_else(|| panic!("missing {struct_name}"))
            .1;

        body.lines()
            .take_while(|line| line.trim() != "}")
            .filter_map(|line| {
                line.split("//")
                    .next()
                    .map(str::trim)
                    .and_then(|line| line.strip_prefix("pub "))
                    .map(|field| field.trim_end_matches(',').to_string())
            })
            .collect()
    }

    #[test]
    fn worker_position_and_resolved_identity_are_independent() {
        let position = AgentRole::Worker {
            index: 7,
            parent: Some("queen".to_string()),
        };
        let mut identity = WorkerRole::new("reviewer", "Reviewer", "codex");
        identity.resolved_definition = Some(RoleDefinitionRef {
            id: "security-reviewer".to_string(),
            version: 3,
        });

        match position {
            AgentRole::Worker { index, parent } => {
                assert_eq!(index, 7);
                assert_eq!(parent.as_deref(), Some("queen"));
            }
            other => panic!("expected worker position, got {other:?}"),
        }
        assert_eq!(identity.role_type, "reviewer");
        assert_eq!(
            identity.resolved_definition,
            Some(RoleDefinitionRef {
                id: "security-reviewer".to_string(),
                version: 3,
            })
        );
    }

    #[test]
    fn worker_role_shape_matches_the_windows_test_stub() {
        let production = public_struct_fields(include_str!("session.rs"), "WorkerRole");
        let test_stub = public_struct_fields(include_str!("session_stub.rs"), "WorkerRole");
        let expected = vec![
            "role_type: String",
            "label: String",
            "default_cli: String",
            "prompt_template: Option<String>",
            "resolved_definition: Option<RoleDefinitionRef>",
        ];

        assert_eq!(production, expected);
        assert_eq!(test_stub, expected);
    }

    #[test]
    fn manager_binds_submit_policy_per_adapter_session() {
        let manager = PtyManager::new();
        for (id, command, expected_adapter) in [
            ("codex-policy-agent", "codex", "codex"),
            ("claude-policy-agent", "claude", "claude"),
            ("cursor-policy-agent", "wsl", "cursor"),
        ] {
            manager
                .create_session(
                    id.to_string(),
                    worker_role(),
                    command,
                    &[],
                    None,
                    80,
                    24,
                )
                .unwrap();
            let policy = manager.submit_policy_for_test(id).unwrap();
            assert_eq!(policy.adapter, Some(expected_adapter));
            assert_eq!(policy.default_gap, Duration::from_millis(50));
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

    /// #256: the payload travels inside one bracketed-paste envelope, the envelope is
    /// terminated before Enter, and Enter is its own discrete write outside the envelope.
    #[test]
    fn submit_brackets_one_multiline_payload_then_one_discrete_bare_enter() {
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

        let result = manager
            .submit("submit-agent", b"line one\nline two\nline three")
            .unwrap();

        let writes = manager.write_records_for_test("submit-agent").unwrap();
        assert_eq!(
            writes,
            vec![
                b"\x1b[200~".to_vec(),
                b"line one\nline two\nline three".to_vec(),
                b"\x1b[201~".to_vec(),
                b"\r".to_vec(),
            ]
        );
        let end_marker_index = writes
            .iter()
            .position(|write| write.as_slice() == b"\x1b[201~")
            .expect("bracketed payload must be terminated");
        let enter_index = writes
            .iter()
            .position(|write| write.as_slice() == b"\r")
            .expect("Enter must be written");
        assert!(
            end_marker_index < enter_index,
            "the paste envelope must close before Enter is sent"
        );
        assert!(!writes[1].ends_with(b"\r"));
        assert!(!writes[1].ends_with(b"\n"));
        assert!(!writes[enter_index].contains(&b'\n'));
        assert_eq!(result.payload_bytes_written, b"line one\nline two\nline three".len());
        assert_eq!(result.submit_bytes_written, 1);
    }

    /// #256: an empty payload stays the documented bare-Enter flush — no paste envelope,
    /// only the discrete Enter that submits whatever is already staged in the composer.
    #[test]
    fn submit_with_empty_payload_writes_only_the_bare_enter() {
        let manager = PtyManager::new();
        manager
            .create_session(
                "bare-enter-agent".to_string(),
                worker_role(),
                "codex",
                &[],
                None,
                80,
                24,
            )
            .unwrap();

        let result = manager.submit("bare-enter-agent", b"").unwrap();

        assert_eq!(
            manager.write_records_for_test("bare-enter-agent").unwrap(),
            vec![b"\r".to_vec()]
        );
        assert_eq!(result.payload_bytes_written, 0);
        assert_eq!(result.submit_bytes_written, 1);
    }

    /// #256: an embedded end marker cannot terminate the envelope early and the reported
    /// payload count is the sanitized byte count actually written.
    #[test]
    fn submit_sanitizes_embedded_end_markers_and_reports_sanitized_bytes() {
        let manager = PtyManager::new();
        manager
            .create_session(
                "sanitize-agent".to_string(),
                worker_role(),
                "codex",
                &[],
                None,
                80,
                24,
            )
            .unwrap();

        let result = manager
            .submit("sanitize-agent", b"safe\x1b[201~payload")
            .unwrap();

        assert_eq!(
            manager.write_records_for_test("sanitize-agent").unwrap(),
            vec![
                b"\x1b[200~".to_vec(),
                b"safepayload".to_vec(),
                b"\x1b[201~".to_vec(),
                b"\r".to_vec(),
            ]
        );
        assert_eq!(result.payload_bytes_written, b"safepayload".len());
    }

    /// #256 acceptance: payloads larger than 1 KB (here, larger than one 16 KB write
    /// chunk) still travel in a single envelope and receive exactly one Enter.
    #[test]
    fn submit_chunks_an_oversized_payload_inside_a_single_envelope() {
        let manager = PtyManager::new();
        manager
            .create_session(
                "chunked-agent".to_string(),
                worker_role(),
                "codex",
                &[],
                None,
                80,
                24,
            )
            .unwrap();

        let payload = vec![b'x'; 16 * 1024 + 17];
        let result = manager.submit("chunked-agent", &payload).unwrap();

        let writes = manager.write_records_for_test("chunked-agent").unwrap();
        assert_eq!(writes.first().unwrap().as_slice(), b"\x1b[200~");
        assert_eq!(writes[1].len(), 16 * 1024);
        assert_eq!(writes[2].len(), 17);
        assert_eq!(writes[3].as_slice(), b"\x1b[201~");
        assert_eq!(writes.last().unwrap().as_slice(), b"\r");
        assert_eq!(
            writes
                .iter()
                .filter(|write| write.as_slice() == b"\r")
                .count(),
            1
        );
        assert_eq!(result.payload_bytes_written, payload.len());
    }

    #[test]
    fn unsubmitted_stub_write_is_visible_in_recent_output() {
        const SENTINEL: &[u8] = b"UNSUBMITTED_SENTINEL";

        let manager = PtyManager::new();
        manager
            .create_session(
                "unsubmitted-agent".to_string(),
                worker_role(),
                "claude",
                &[],
                None,
                80,
                24,
            )
            .unwrap();

        manager.write("unsubmitted-agent", SENTINEL).unwrap();

        assert_eq!(
            manager.recent_output("unsubmitted-agent").as_deref(),
            Some("UNSUBMITTED_SENTINEL")
        );
        assert_eq!(
            manager.write_records_for_test("unsubmitted-agent").unwrap(),
            vec![SENTINEL.to_vec()]
        );
    }

    #[test]
    fn submit_blocks_a_concurrent_writer_until_after_enter() {
        let manager = Arc::new(PtyManager::new());
        manager
            .create_session(
                "atomic-submit-agent".to_string(),
                worker_role(),
                "claude",
                &[],
                None,
                80,
                24,
            )
            .unwrap();

        let session = manager
            .sessions
            .read()
            .get("atomic-submit-agent")
            .cloned()
            .unwrap();
        session.pause_submit_after_payload_for_test();

        let (start_writer_tx, start_writer_rx) = std::sync::mpsc::channel();
        let (writer_attempted_tx, writer_attempted_rx) = std::sync::mpsc::channel();
        let concurrent_manager = Arc::clone(&manager);
        let concurrent_writer = thread::spawn(move || {
            start_writer_rx.recv().unwrap();
            writer_attempted_tx.send(()).unwrap();
            concurrent_manager
                .write("atomic-submit-agent", b"interloper")
                .unwrap();
        });

        let submit_manager = Arc::clone(&manager);
        let submitter = thread::spawn(move || {
            submit_manager
                .submit("atomic-submit-agent", b"payload")
                .unwrap();
        });

        assert!(session.wait_for_submit_payload_for_test());
        start_writer_tx.send(()).unwrap();
        writer_attempted_rx.recv().unwrap();
        assert!(
            session.writer_locked_for_test(),
            "submit must retain the writer lock between payload and Enter"
        );
        session.resume_submit_for_test();
        submitter.join().unwrap();
        concurrent_writer.join().unwrap();

        assert_eq!(
            manager
                .write_records_for_test("atomic-submit-agent")
                .unwrap(),
            vec![
                b"\x1b[200~".to_vec(),
                b"payload".to_vec(),
                b"\x1b[201~".to_vec(),
                b"\r".to_vec(),
                b"interloper".to_vec()
            ]
        );
    }

    #[test]
    fn abandoned_submit_pause_times_out_after_test_thread_panics() {
        let manager = Arc::new(PtyManager::new());
        manager
            .create_session(
                "abandoned-submit-pause-agent".to_string(),
                worker_role(),
                "claude",
                &[],
                None,
                80,
                24,
            )
            .unwrap();

        let session = manager
            .sessions
            .read()
            .get("abandoned-submit-pause-agent")
            .cloned()
            .unwrap();
        session.pause_submit_after_payload_for_test();

        let (submit_done_tx, submit_done_rx) = std::sync::mpsc::channel();
        let submit_manager = Arc::clone(&manager);
        let submitter = thread::spawn(move || {
            submit_manager
                .submit("abandoned-submit-pause-agent", b"payload")
                .unwrap();
            submit_done_tx.send(()).unwrap();
        });

        let pause_controller = thread::spawn(move || {
            assert!(session.wait_for_submit_payload_for_test());
            panic!("simulated test panic before resume");
        });
        assert!(pause_controller.join().is_err());
        submit_done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("submit stayed blocked after the test-side resume was abandoned");
        submitter.join().unwrap();

        assert_eq!(
            manager
                .write_records_for_test("abandoned-submit-pause-agent")
                .unwrap(),
            vec![
                b"\x1b[200~".to_vec(),
                b"payload".to_vec(),
                b"\x1b[201~".to_vec(),
                b"\r".to_vec()
            ]
        );
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

    /// #287: the snapshot exposes the same bytes the diagnostic tail sees, with offsets
    /// that sequence it against the live stream.
    #[test]
    fn snapshot_reports_offsets_that_match_the_recorded_stream() {
        let manager = PtyManager::new();
        manager
            .create_session(
                "snapshot-agent".to_string(),
                worker_role(),
                "claude",
                &[],
                None,
                80,
                24,
            )
            .unwrap();

        manager.record_output_for_test("snapshot-agent", b"\x1b[Hfirst frame");
        manager.record_output_for_test("snapshot-agent", b" second");

        let snapshot = manager.snapshot("snapshot-agent").unwrap();
        assert_eq!(snapshot.id, "snapshot-agent");
        assert_eq!(snapshot.offset_start, 0);
        assert_eq!(snapshot.offset_end, 21);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&snapshot.data)
                .unwrap(),
            b"\x1b[Hfirst frame second"
        );
        assert!(manager.snapshot("never-spawned").is_none());
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;

    fn output(id: &str, offset: u64, bytes: &[u8]) -> EmitterMessage {
        EmitterMessage::Output {
            id: id.to_string(),
            offset,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn coalesce_merges_contiguous_chunks_per_agent_and_keeps_exit_after_data() {
        let events = coalesce(vec![
            output("a", 0, b"hel"),
            output("b", 100, b"other"),
            output("a", 3, b"lo"),
            EmitterMessage::Exited {
                id: "a".to_string(),
            },
            output("b", 105, b" agent"),
        ]);

        assert_eq!(
            events,
            vec![
                CoalescedEvent::Output {
                    id: "a".to_string(),
                    offset: 0,
                    bytes: b"hello".to_vec(),
                },
                CoalescedEvent::Output {
                    id: "b".to_string(),
                    offset: 100,
                    bytes: b"other agent".to_vec(),
                },
                CoalescedEvent::Exited {
                    id: "a".to_string(),
                },
            ]
        );
    }

    #[test]
    fn coalesce_starts_a_new_batch_when_offsets_are_not_contiguous() {
        let events = coalesce(vec![output("a", 0, b"abc"), output("a", 10, b"xyz")]);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[1], CoalescedEvent::Output { offset: 10, .. }));
    }

    #[test]
    fn coalesce_never_merges_data_across_an_exit_marker() {
        let events = coalesce(vec![
            output("a", 0, b"abc"),
            EmitterMessage::Exited {
                id: "a".to_string(),
            },
            output("a", 3, b"def"),
        ]);
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[2], CoalescedEvent::Output { offset: 3, .. }));
    }

    #[test]
    fn event_name_is_stable_for_agent_and_scratch_ids_and_sanitizes_others() {
        assert_eq!(
            pty_output_event_name("7c4790a1-370c-4d98-8690-a5fbe4b35e5b-worker-1"),
            "pty-output:7c4790a1-370c-4d98-8690-a5fbe4b35e5b-worker-1"
        );
        assert_eq!(
            pty_output_event_name("scratch:7c4790a1:9f1e"),
            "pty-output:scratch:7c4790a1:9f1e"
        );
        assert_eq!(pty_output_event_name("odd id.1"), "pty-output:odd_id_1");
        assert_eq!(pty_output_event_name("Ω"), "pty-output:_");
    }

    /// #289 payload-shape gate: a 4 KB read must not balloon to the ~16 KB a JSON number
    /// array produced.
    #[test]
    fn a_4kb_chunk_crosses_ipc_in_well_under_1_4x_its_size() {
        let bytes: Vec<u8> = (0..4096u32).map(|i| (i % 256) as u8).collect();
        let payload = PtyOutput::new("7c4790a1-370c-4d98-8690-a5fbe4b35e5b-worker-12", 123, &bytes);
        let json = serde_json::to_string(&payload).unwrap();

        assert_eq!(payload.data.len(), 4096_usize.div_ceil(3) * 4);
        assert!(
            json.len() <= (4096.0 * 1.4) as usize,
            "payload is {} bytes for a 4096-byte chunk",
            json.len()
        );
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&payload.data)
                .unwrap(),
            bytes
        );
    }

    #[test]
    fn replay_capacity_is_clamped_when_set() {
        let mut manager = PtyManager::new();
        assert_eq!(manager.replay_capacity(), DEFAULT_REPLAY_CAPACITY);
        manager.set_replay_capacity(1);
        assert_eq!(manager.replay_capacity(), super::super::output_ring::MIN_REPLAY_CAPACITY);
        manager.set_replay_capacity(usize::MAX);
        assert_eq!(manager.replay_capacity(), super::super::output_ring::MAX_REPLAY_CAPACITY);
    }
}
