//! Unit-test PTY session stub that avoids linking portable-pty on Windows.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentRole {
    MasterPlanner,
    Queen,
    Planner { index: u8 },
    Worker { index: u8, parent: Option<String> },
    Fusion { variant: String },
    Judge { session_id: String },
    Evaluator,
    QaWorker { index: u8, parent: Option<String> },
    Prince,
    ScratchShell,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentStatus {
    Starting,
    Running,
    Idle,
    WaitingForInput(String),
    Completed,
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkerRole {
    pub role_type: String,
    pub label: String,
    pub default_cli: String,
    pub prompt_template: Option<String>,
}

impl WorkerRole {
    pub fn new(role_type: &str, label: &str, default_cli: &str) -> Self {
        Self {
            role_type: role_type.to_string(),
            label: label.to_string(),
            default_cli: default_cli.to_string(),
            prompt_template: None,
        }
    }
}

impl Default for WorkerRole {
    fn default() -> Self {
        Self::new("general", "General", "claude")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentConfig {
    #[serde(default = "default_cli")]
    pub cli: String,
    pub model: Option<String>,
    #[serde(default)]
    pub flags: Vec<String>,
    pub label: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub role: Option<WorkerRole>,
    pub initial_prompt: Option<String>,
}

fn default_cli() -> String {
    "claude".to_string()
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            cli: "claude".to_string(),
            model: None,
            flags: vec![],
            label: None,
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("Failed to create PTY: {0}")]
    CreateError(String),
    #[error("Failed to spawn command: {0}")]
    SpawnError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("PTY session not found: {0}")]
    NotFound(String),
}

pub(crate) struct SendReader(Box<dyn Read + Send>);
pub(crate) struct SendWriter(Box<dyn Write + Send>);

unsafe impl Send for SendReader {}
unsafe impl Sync for SendReader {}
unsafe impl Send for SendWriter {}
unsafe impl Sync for SendWriter {}

const CHUNK_SIZE: usize = 16 * 1024;
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

fn find_subslice(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() || start >= haystack.len() {
        return None;
    }

    haystack[start..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| start + offset)
}

fn sanitize_bracketed_paste(data: &[u8]) -> Cow<'_, [u8]> {
    let Some(mut next_match) = find_subslice(data, BRACKETED_PASTE_END, 0) else {
        return Cow::Borrowed(data);
    };

    let mut sanitized = Vec::with_capacity(data.len());
    let mut cursor = 0;
    loop {
        sanitized.extend_from_slice(&data[cursor..next_match]);
        cursor = next_match + BRACKETED_PASTE_END.len();

        match find_subslice(data, BRACKETED_PASTE_END, cursor) {
            Some(found) => next_match = found,
            None => {
                sanitized.extend_from_slice(&data[cursor..]);
                break;
            }
        }
    }

    Cow::Owned(sanitized)
}

const RECENT_OUTPUT_CAPACITY: usize = 8 * 1024;

pub struct PtySession {
    pub role: AgentRole,
    pub status: Arc<parking_lot::RwLock<AgentStatus>>,
    writer: Arc<Mutex<SendWriter>>,
    reader: Arc<Mutex<SendReader>>,
    /// The argv this session was created with. The real session hands these to the OS and
    /// forgets them; retaining them here is what lets tests assert that per-agent store
    /// isolation (#207) actually reached the spawn rather than just the helper that
    /// computes it.
    args: Vec<String>,
    recent_output: Arc<Mutex<VecDeque<u8>>>,
}

unsafe impl Send for PtySession {}
unsafe impl Sync for PtySession {}

impl PtySession {
    pub fn new(
        _id: String,
        role: AgentRole,
        _command: &str,
        args: &[&str],
        _cwd: Option<&str>,
        _cols: u16,
        _rows: u16,
    ) -> Result<Self, PtyError> {
        let session = Self {
            role,
            status: Arc::new(parking_lot::RwLock::new(AgentStatus::Starting)),
            writer: Arc::new(Mutex::new(SendWriter(Box::new(std::io::sink())))),
            reader: Arc::new(Mutex::new(SendReader(Box::new(std::io::Cursor::new(
                Vec::new(),
            ))))),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            recent_output: Arc::new(Mutex::new(VecDeque::with_capacity(
                RECENT_OUTPUT_CAPACITY,
            ))),
        };

        // #207 test fixture: a flag-borne sentinel is the only way an integration test
        // can produce a spawn that "succeeds" and then dies during startup — the field
        // failure mode. It flows in through AgentConfig.flags, so the full HTTP path is
        // exercised. The canned output mirrors the real codex lock error so tests can
        // assert the text is surfaced.
        if session.args.iter().any(|arg| arg == "--stub-die-on-start") {
            *session.status.write() = AgentStatus::Completed;
            session.record_output(
                b"failed to initialize state runtime: failed to open log DB: \
                  (code: 5) database is locked (stub)",
            );
        }

        Ok(session)
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn record_output(&self, bytes: &[u8]) {
        let mut buffer = self.recent_output.lock();
        for byte in bytes {
            if buffer.len() == RECENT_OUTPUT_CAPACITY {
                buffer.pop_front();
            }
            buffer.push_back(*byte);
        }
    }

    pub fn recent_output(&self) -> String {
        let buffer = self.recent_output.lock();
        let bytes: Vec<u8> = buffer.iter().copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn write(&self, data: &[u8]) -> Result<(), PtyError> {
        let mut writer = self.writer.lock();

        for chunk in data.chunks(CHUNK_SIZE) {
            writer.0.write_all(chunk)?;
            writer.0.flush()?;
        }

        Ok(())
    }

    pub fn write_bracketed(&self, data: &[u8]) -> Result<(), PtyError> {
        let mut writer = self.writer.lock();
        let sanitized = sanitize_bracketed_paste(data);

        writer.0.write_all(BRACKETED_PASTE_START)?;
        writer.0.flush()?;

        for chunk in sanitized.as_ref().chunks(CHUNK_SIZE) {
            writer.0.write_all(chunk)?;
            writer.0.flush()?;
        }

        writer.0.write_all(BRACKETED_PASTE_END)?;
        writer.0.flush()?;

        Ok(())
    }

    pub fn kill(&self) -> Result<(), PtyError> {
        Ok(())
    }

    /// Mirror the real session's liveness semantics instead of hardcoding
    /// `false`.
    ///
    /// The real `PtySession::is_alive` reports whether the child process is
    /// still running. A stub that always answers `false` makes every production
    /// path gated on PTY liveness — the evaluator/prince respawn checks and the
    /// live-status overlay — permanently untestable on the Windows CI runner,
    /// which is where this crate's tests actually run. Track the stubbed status
    /// instead so those branches are reachable from tests.
    pub fn is_alive(&self) -> bool {
        !matches!(
            *self.status.read(),
            AgentStatus::Completed | AgentStatus::Error(_)
        )
    }

    #[allow(dead_code)]
    pub async fn graceful_terminate(&self) -> Result<(), PtyError> {
        Ok(())
    }

    pub fn resize(&self, _cols: u16, _rows: u16) -> Result<(), PtyError> {
        Ok(())
    }

    pub fn get_reader(&self) -> Arc<Mutex<SendReader>> {
        Arc::clone(&self.reader)
    }
}

pub fn read_from_reader(
    reader: &Arc<Mutex<SendReader>>,
    buf: &mut [u8],
) -> Result<usize, std::io::Error> {
    let mut r = reader.lock();
    r.0.read(buf)
}

#[cfg(test)]
mod tests {
    use super::{sanitize_bracketed_paste, BRACKETED_PASTE_END};

    #[test]
    fn sanitize_bracketed_paste_removes_end_sequence_from_payload() {
        let payload = b"hello\x1b[201~world\x1b[201~!";
        let sanitized = sanitize_bracketed_paste(payload);

        assert_eq!(sanitized.as_ref(), b"helloworld!");
        assert!(!sanitized
            .as_ref()
            .windows(BRACKETED_PASTE_END.len())
            .any(|w| w == BRACKETED_PASTE_END));
    }
}
