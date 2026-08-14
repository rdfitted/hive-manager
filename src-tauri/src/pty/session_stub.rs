//! Unit-test PTY session stub that avoids linking portable-pty on Windows.

use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
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
/// Unvalidated default; override with `HIVE_PTY_SUBMIT_GAP_MS` for the #226 sweep.
///
/// Under load, measured payload/Enter separations of 1.2-2.7 s failed twice,
/// while roughly 4-5 s succeeded twice and 65 s succeeded once. Agent busy
/// state was uncontrolled, so those observations do not justify replacing the
/// compiled 50 ms default with another guess.
const SUBMIT_GAP: Duration = Duration::from_millis(50);
const SUBMIT_GAP_ENV: &str = "HIVE_PTY_SUBMIT_GAP_MS";
static RUNTIME_SUBMIT_GAP: OnceLock<Duration> = OnceLock::new();
const SUBMIT_GAP_TEST_TIMEOUT: Duration = Duration::from_secs(1);

fn submit_gap_from_override(value: Option<&str>) -> Duration {
    value
        .and_then(|milliseconds| milliseconds.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(SUBMIT_GAP)
}

fn submit_gap() -> Duration {
    *RUNTIME_SUBMIT_GAP.get_or_init(|| {
        let override_value = std::env::var(SUBMIT_GAP_ENV).ok();
        submit_gap_from_override(override_value.as_deref())
    })
}
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

struct RecordingWriter {
    writes: Arc<Mutex<Vec<Vec<u8>>>>,
}

#[derive(Default)]
struct SubmitGapControl {
    pause: bool,
    payload_written: bool,
    resume: bool,
}

impl Write for RecordingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writes.lock().push(buf.to_vec());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

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
    write_records: Arc<Mutex<Vec<Vec<u8>>>>,
    submit_gap_control: Arc<(Mutex<SubmitGapControl>, Condvar)>,
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
    fn write_locked(writer: &mut SendWriter, data: &[u8]) -> Result<(), PtyError> {
        for chunk in data.chunks(CHUNK_SIZE) {
            writer.0.write_all(chunk)?;
            writer.0.flush()?;
        }

        Ok(())
    }

    pub fn new(
        _id: String,
        role: AgentRole,
        _command: &str,
        args: &[&str],
        _cwd: Option<&str>,
        _cols: u16,
        _rows: u16,
    ) -> Result<Self, PtyError> {
        let write_records = Arc::new(Mutex::new(Vec::new()));
        let session = Self {
            role,
            status: Arc::new(parking_lot::RwLock::new(AgentStatus::Starting)),
            writer: Arc::new(Mutex::new(SendWriter(Box::new(RecordingWriter {
                writes: Arc::clone(&write_records),
            })))),
            write_records,
            submit_gap_control: Arc::new((Mutex::new(SubmitGapControl::default()), Condvar::new())),
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

    pub fn pause_submit_after_payload_for_test(&self) {
        let (control, _) = &*self.submit_gap_control;
        let mut control = control.lock();
        control.pause = true;
        control.payload_written = false;
        control.resume = false;
    }

    pub fn wait_for_submit_payload_for_test(&self) -> bool {
        let (control, changed) = &*self.submit_gap_control;
        let mut control = control.lock();
        let deadline = Instant::now() + SUBMIT_GAP_TEST_TIMEOUT;

        while !control.payload_written {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            changed.wait_for(&mut control, remaining);
        }

        true
    }

    pub fn resume_submit_for_test(&self) {
        let (control, changed) = &*self.submit_gap_control;
        control.lock().resume = true;
        changed.notify_all();
    }

    fn pause_submit_after_payload_if_requested(&self) {
        let (control, changed) = &*self.submit_gap_control;
        let mut control = control.lock();
        if !control.pause {
            return;
        }

        let deadline = Instant::now() + SUBMIT_GAP_TEST_TIMEOUT;
        control.payload_written = true;
        changed.notify_all();
        while !control.resume {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            changed.wait_for(&mut control, remaining);
        }
        *control = SubmitGapControl::default();
    }

    pub fn write(&self, data: &[u8]) -> Result<(), PtyError> {
        let mut writer = self.writer.lock();
        Self::write_locked(&mut writer, data)
    }

    pub fn submit(&self, data: &[u8]) -> Result<(), PtyError> {
        let mut writer = self.writer.lock();
        Self::write_locked(&mut writer, data)?;
        self.pause_submit_after_payload_if_requested();
        std::thread::sleep(submit_gap());
        Self::write_locked(&mut writer, b"\r")
    }

    pub fn write_records(&self) -> Vec<Vec<u8>> {
        self.write_records.lock().clone()
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
    use super::{
        sanitize_bracketed_paste, submit_gap_from_override, BRACKETED_PASTE_END, SUBMIT_GAP,
    };
    use std::time::Duration;

    #[test]
    fn submit_gap_override_parses_milliseconds() {
        assert_eq!(
            submit_gap_from_override(Some(" 4500 ")),
            Duration::from_millis(4_500)
        );
        assert_eq!(submit_gap_from_override(Some("0")), Duration::ZERO);
    }

    #[test]
    fn invalid_submit_gap_override_falls_back_to_default() {
        assert_eq!(submit_gap_from_override(Some("not-a-number")), SUBMIT_GAP);
        assert_eq!(submit_gap_from_override(Some("-1")), SUBMIT_GAP);
        assert_eq!(submit_gap_from_override(None), SUBMIT_GAP);
    }

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
