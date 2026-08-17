//! Unit-test PTY session stub that avoids linking portable-pty on Windows.

use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use thiserror::Error;

use crate::adapters::{PtySubmitPolicy, PtySubmitResult};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RoleDefinitionRef {
    pub id: String,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkerRole {
    pub role_type: String,
    pub label: String,
    pub default_cli: String,
    pub prompt_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_definition: Option<RoleDefinitionRef>,
}

impl WorkerRole {
    pub fn new(role_type: &str, label: &str, default_cli: &str) -> Self {
        Self {
            role_type: role_type.to_string(),
            label: label.to_string(),
            default_cli: default_cli.to_string(),
            prompt_template: None,
            resolved_definition: None,
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
// BEGIN SHARED SUBMIT GAP OVERRIDE POLICY
const SUBMIT_GAP_ENV: &str = "HIVE_PTY_SUBMIT_GAP_MS";
/// Safety ceiling for the operator override, not a submit-gap default.
///
/// Five minutes bounds how long `submit` can hold the per-session writer mutex while preserving
/// the known successful 65-second observation for the future U1 sweep.
const MAX_PTY_SUBMIT_GAP_OVERRIDE_MS: u64 = 300_000;
static RUNTIME_SUBMIT_GAP: OnceLock<Option<Duration>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubmitGapOverride {
    Unset,
    Valid(Duration),
    Invalid(SubmitGapInvalidReason),
    OverLimit { requested_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubmitGapInvalidReason {
    NonUnicode,
    NotUnsignedInteger,
    Overflow,
}

fn parse_submit_gap_override(value: Option<&std::ffi::OsStr>) -> SubmitGapOverride {
    let Some(value) = value else {
        return SubmitGapOverride::Unset;
    };
    let Some(milliseconds) = value.to_str() else {
        return SubmitGapOverride::Invalid(SubmitGapInvalidReason::NonUnicode);
    };

    match milliseconds.trim().parse::<u64>() {
        Ok(requested_ms) if requested_ms <= MAX_PTY_SUBMIT_GAP_OVERRIDE_MS => {
            SubmitGapOverride::Valid(Duration::from_millis(requested_ms))
        }
        Ok(requested_ms) => SubmitGapOverride::OverLimit { requested_ms },
        Err(error)
            if matches!(
                error.kind(),
                &std::num::IntErrorKind::PosOverflow | &std::num::IntErrorKind::NegOverflow
            ) =>
        {
            SubmitGapOverride::Invalid(SubmitGapInvalidReason::Overflow)
        }
        Err(_) => SubmitGapOverride::Invalid(SubmitGapInvalidReason::NotUnsignedInteger),
    }
}

fn cached_submit_gap_override<F>(
    cache: &OnceLock<Option<Duration>>,
    value: Option<&std::ffi::OsStr>,
    warn: F,
) -> Option<Duration>
where
    F: FnOnce(SubmitGapOverride),
{
    *cache.get_or_init(|| match parse_submit_gap_override(value) {
        SubmitGapOverride::Unset => None,
        SubmitGapOverride::Valid(duration) => Some(duration),
        rejected @ (SubmitGapOverride::Invalid(_) | SubmitGapOverride::OverLimit { .. }) => {
            warn(rejected);
            None
        }
    })
}

fn warn_rejected_submit_gap_override(override_value: SubmitGapOverride) {
    match override_value {
        SubmitGapOverride::Invalid(reason) => tracing::warn!(
            ?reason,
            "Ignoring invalid {SUBMIT_GAP_ENV} override; using the adapter default"
        ),
        SubmitGapOverride::OverLimit { requested_ms } => tracing::warn!(
            requested_ms,
            safety_ceiling_ms = MAX_PTY_SUBMIT_GAP_OVERRIDE_MS,
            "Rejecting over-limit {SUBMIT_GAP_ENV} override; using the adapter default"
        ),
        SubmitGapOverride::Unset | SubmitGapOverride::Valid(_) => {}
    }
}

fn resolve_submit_gap(
    policy: PtySubmitPolicy,
    override_gap: Option<Duration>,
) -> Duration {
    override_gap.unwrap_or(policy.default_gap)
}

fn submit_gap(policy: PtySubmitPolicy) -> Duration {
    let override_value = std::env::var_os(SUBMIT_GAP_ENV);
    let override_gap = cached_submit_gap_override(
        &RUNTIME_SUBMIT_GAP,
        override_value.as_deref(),
        warn_rejected_submit_gap_override,
    );
    resolve_submit_gap(policy, override_gap)
}
// END SHARED SUBMIT GAP OVERRIDE POLICY

const SUBMIT_GAP_TEST_TIMEOUT: Duration = Duration::from_secs(1);
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

struct RecordingWriter {
    writes: Arc<Mutex<Vec<Vec<u8>>>>,
    recent_output: Arc<Mutex<VecDeque<u8>>>,
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
        let mut recent_output = self.recent_output.lock();
        for byte in buf {
            if recent_output.len() == RECENT_OUTPUT_CAPACITY {
                recent_output.pop_front();
            }
            recent_output.push_back(*byte);
        }
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
    submit_policy: PtySubmitPolicy,
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
        submit_policy: PtySubmitPolicy,
        args: &[&str],
        _cwd: Option<&str>,
        _cols: u16,
        _rows: u16,
    ) -> Result<Self, PtyError> {
        let write_records = Arc::new(Mutex::new(Vec::new()));
        let recent_output = Arc::new(Mutex::new(VecDeque::with_capacity(
            RECENT_OUTPUT_CAPACITY,
        )));
        let session = Self {
            role,
            status: Arc::new(parking_lot::RwLock::new(AgentStatus::Starting)),
            submit_policy,
            writer: Arc::new(Mutex::new(SendWriter(Box::new(RecordingWriter {
                writes: Arc::clone(&write_records),
                recent_output: Arc::clone(&recent_output),
            })))),
            write_records,
            submit_gap_control: Arc::new((Mutex::new(SubmitGapControl::default()), Condvar::new())),
            reader: Arc::new(Mutex::new(SendReader(Box::new(std::io::Cursor::new(
                Vec::new(),
            ))))),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            recent_output,
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

    pub fn submit_policy_for_test(&self) -> PtySubmitPolicy {
        self.submit_policy
    }

    pub fn writer_locked_for_test(&self) -> bool {
        self.writer.try_lock().is_none()
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

    pub fn submit(&self, data: &[u8]) -> Result<PtySubmitResult, PtyError> {
        let mut writer = self.writer.lock();
        let payload_bytes_written = if data.is_empty() {
            0
        } else {
            Self::write_bracketed_locked(&mut writer, data)?
        };
        self.pause_submit_after_payload_if_requested();
        std::thread::sleep(submit_gap(self.submit_policy));
        Self::write_locked(&mut writer, b"\r")?;
        Ok(PtySubmitResult {
            payload_bytes_written,
            submit_bytes_written: 1,
        })
    }

    pub fn write_records(&self) -> Vec<Vec<u8>> {
        self.write_records.lock().clone()
    }

    fn write_bracketed_locked(
        writer: &mut SendWriter,
        data: &[u8],
    ) -> Result<usize, PtyError> {
        let sanitized = sanitize_bracketed_paste(data);

        writer.0.write_all(BRACKETED_PASTE_START)?;
        writer.0.flush()?;

        for chunk in sanitized.as_ref().chunks(CHUNK_SIZE) {
            writer.0.write_all(chunk)?;
            writer.0.flush()?;
        }

        writer.0.write_all(BRACKETED_PASTE_END)?;
        writer.0.flush()?;

        Ok(sanitized.as_ref().len())
    }

    pub fn write_bracketed(&self, data: &[u8]) -> Result<(), PtyError> {
        let mut writer = self.writer.lock();
        Self::write_bracketed_locked(&mut writer, data)?;
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
        cached_submit_gap_override, parse_submit_gap_override, resolve_submit_gap,
        sanitize_bracketed_paste, SubmitGapInvalidReason, SubmitGapOverride,
        BRACKETED_PASTE_END, MAX_PTY_SUBMIT_GAP_OVERRIDE_MS,
    };
    use crate::adapters::PtySubmitPolicy;
    use std::cell::Cell;
    use std::ffi::OsStr;
    use std::sync::OnceLock;
    use std::time::Duration;

    #[test]
    fn submit_gap_override_distinguishes_all_supported_and_rejected_inputs() {
        assert_eq!(parse_submit_gap_override(None), SubmitGapOverride::Unset);
        assert_eq!(
            parse_submit_gap_override(Some(OsStr::new("0"))),
            SubmitGapOverride::Valid(Duration::ZERO)
        );
        assert_eq!(
            parse_submit_gap_override(Some(OsStr::new(" 4500 "))),
            SubmitGapOverride::Valid(Duration::from_millis(4_500))
        );
        assert_eq!(
            parse_submit_gap_override(Some(OsStr::new("65000"))),
            SubmitGapOverride::Valid(Duration::from_secs(65))
        );
        assert_eq!(
            parse_submit_gap_override(Some(OsStr::new("not-a-number"))),
            SubmitGapOverride::Invalid(SubmitGapInvalidReason::NotUnsignedInteger)
        );
        assert_eq!(
            parse_submit_gap_override(Some(OsStr::new("18446744073709551616"))),
            SubmitGapOverride::Invalid(SubmitGapInvalidReason::Overflow)
        );
        assert_eq!(
            parse_submit_gap_override(Some(OsStr::new("300001"))),
            SubmitGapOverride::OverLimit {
                requested_ms: 300_001
            }
        );
    }

    #[test]
    fn rejected_submit_gap_override_warns_once_and_defers_to_the_session_policy() {
        let adapter_policy = PtySubmitPolicy {
            adapter: Some("future-tuned-adapter"),
            default_gap: Duration::from_millis(123),
        };

        for (raw, expected_warning) in [
            (
                "not-a-number",
                SubmitGapOverride::Invalid(SubmitGapInvalidReason::NotUnsignedInteger),
            ),
            (
                "18446744073709551616",
                SubmitGapOverride::Invalid(SubmitGapInvalidReason::Overflow),
            ),
            (
                "300001",
                SubmitGapOverride::OverLimit {
                    requested_ms: 300_001,
                },
            ),
        ] {
            let cache = OnceLock::new();
            let warning_count = Cell::new(0);
            let warning = Cell::new(None);
            let override_gap = cached_submit_gap_override(
                &cache,
                Some(OsStr::new(raw)),
                |rejected| {
                    warning_count.set(warning_count.get() + 1);
                    warning.set(Some(rejected));
                },
            );

            assert_eq!(override_gap, None);
            assert_eq!(warning.get(), Some(expected_warning));
            assert_eq!(resolve_submit_gap(adapter_policy, override_gap), adapter_policy.default_gap);

            let second = cached_submit_gap_override(
                &cache,
                Some(OsStr::new("65000")),
                |_| warning_count.set(warning_count.get() + 1),
            );
            assert_eq!(second, None, "a rejected process override stays rejected");
            assert_eq!(warning_count.get(), 1, "a rejected override warns once");
        }

        let valid_cache = OnceLock::new();
        let valid_warning_count = Cell::new(0);
        let valid = cached_submit_gap_override(
            &valid_cache,
            Some(OsStr::new("65000")),
            |_| valid_warning_count.set(valid_warning_count.get() + 1),
        );
        assert_eq!(valid, Some(Duration::from_secs(65)));
        assert_eq!(valid_warning_count.get(), 0);

        let unset_cache = OnceLock::new();
        let unset_warning_count = Cell::new(0);
        let unset = cached_submit_gap_override(&unset_cache, None, |_| {
            unset_warning_count.set(unset_warning_count.get() + 1)
        });
        assert_eq!(unset, None);
        assert_eq!(unset_warning_count.get(), 0);
    }

    #[test]
    fn review_fix_submit_gap_override_rejects_values_above_the_safety_ceiling() {
        assert_eq!(
            parse_submit_gap_override(Some(OsStr::new("65000"))),
            SubmitGapOverride::Valid(Duration::from_secs(65)),
            "the known 65-second success must remain representable"
        );
        assert_eq!(
            parse_submit_gap_override(Some(OsStr::new("300000"))),
            SubmitGapOverride::Valid(Duration::from_millis(
                MAX_PTY_SUBMIT_GAP_OVERRIDE_MS
            )),
            "the named safety ceiling itself remains valid"
        );
        assert_eq!(
            parse_submit_gap_override(Some(OsStr::new("300001"))),
            SubmitGapOverride::OverLimit {
                requested_ms: 300_001
            },
            "an override above the 300-second safety ceiling must be rejected, not clamped"
        );
    }

    #[test]
    fn submit_gap_override_policy_matches_the_production_session() {
        fn policy_block(source: &str) -> String {
            source
                .split_once("// BEGIN SHARED SUBMIT GAP OVERRIDE POLICY")
                .expect("policy start marker")
                .1
                .split_once("// END SHARED SUBMIT GAP OVERRIDE POLICY")
                .expect("policy end marker")
                .0
                .replace("\r\n", "\n")
        }

        let production = policy_block(include_str!("session.rs"));
        let test_stub = policy_block(include_str!("session_stub.rs"));
        assert_eq!(production, test_stub, "the Windows shim must not drift");
    }

    #[test]
    fn review_fix_documented_pty_buffer_sample_byte_count_matches_output() {
        let document = include_str!("../../../docs/pty-submit-sweep.md");
        let json_sample = document
            .split_once("```json")
            .expect("documented JSON sample")
            .1
            .split_once("```")
            .expect("closed JSON sample")
            .0;
        let sample: serde_json::Value =
            serde_json::from_str(json_sample).expect("valid documented JSON sample");
        let output = sample["output"].as_str().expect("sample output string");
        let byte_count = sample["byte_count"]
            .as_u64()
            .expect("sample byte_count") as usize;

        assert_eq!(
            byte_count,
            output.len(),
            "documented byte_count must equal the UTF-8 byte length of output"
        );
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
