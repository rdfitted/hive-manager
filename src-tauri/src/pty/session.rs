use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::borrow::Cow;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use parking_lot::Mutex;
use thiserror::Error;

use crate::adapters::{PtySubmitPolicy, PtySubmitResult};

use super::output_ring::{OutputRing, OutputSnapshot, RECENT_OUTPUT_VIEW_BYTES};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentRole {
    MasterPlanner,  // Initial planning agent that generates plan.md
    Queen,
    Planner { index: u8 },
    Worker { index: u8, parent: Option<String> },
    Fusion { variant: String },
    Judge { session_id: String },
    Evaluator,
    QaWorker { index: u8, parent: Option<String> },
    /// Remediation authority — a peer to the Queen and Evaluator. Receives the QA
    /// team's findings and spawns its own fix team (regular `Worker`s parented to
    /// the Prince) to resolve them before the Queen pushes the PR.
    Prince,
    /// Operator-owned shell that is scoped to a session but is not a managed agent.
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

/// Worker role configuration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RoleDefinitionRef {
    pub id: String,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkerRole {
    pub role_type: String,          // "backend", "frontend", "coherence", "simplify", or custom
    pub label: String,              // Display name
    pub default_cli: String,        // Default CLI for this role
    pub prompt_template: Option<String>, // Path to template or inline prompt
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
    pub cli: String,              // "claude", "codex", "opencode", "cursor", "droid", "qwen"
    pub model: Option<String>,    // "opus", "gpt-6-sol", "gpt-6-luna", etc.
    #[serde(default)]
    pub flags: Vec<String>,       // Additional CLI flags
    pub label: Option<String>,    // Display name
    #[serde(default)]
    pub name: Option<String>,     // Stable agent name
    #[serde(default)]
    pub description: Option<String>, // One-line task summary
    pub role: Option<WorkerRole>, // Worker role assignment
    pub initial_prompt: Option<String>, // Prompt to inject on spawn
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

// Wrapper to make the reader/writer Send
pub(crate) struct SendReader(Box<dyn Read + Send>);
pub(crate) struct SendWriter(Box<dyn Write + Send>);

unsafe impl Send for SendReader {}
unsafe impl Sync for SendReader {}
unsafe impl Send for SendWriter {}
unsafe impl Sync for SendWriter {}

// Wrapper to keep the master PTY alive and allow resize
pub(crate) struct MasterPtyHandle(Box<dyn portable_pty::MasterPty + Send>);
unsafe impl Send for MasterPtyHandle {}
unsafe impl Sync for MasterPtyHandle {}

fn into_io_error<E: std::fmt::Display>(error: E) -> PtyError {
    PtyError::IoError(std::io::Error::other(error.to_string()))
}

#[cfg(windows)]
fn kill_windows_tree(pid: u32) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut helper = Command::new("taskkill")
        .arg("/PID")
        .arg(pid.to_string())
        .args(["/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = helper.try_wait()? {
            return if status.success() || status.code() == Some(128) {
                Ok(())
            } else {
                Err(std::io::Error::other(format!(
                    "taskkill /PID {pid} /T /F exited with {status}"
                )))
            };
        }
        if Instant::now() >= deadline {
            let _ = helper.kill();
            let _ = helper.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("taskkill /PID {pid} /T /F timed out"),
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

impl MasterPtyHandle {
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        use portable_pty::PtySize;
        self.0
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(into_io_error)
    }
}

/// Maximum chunk size for PTY writes (16KB) - respects Windows pipe buffer limits
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

/// Bracketed paste mode escape sequences
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

pub struct PtySession {
    pub role: AgentRole,
    pub status: Arc<parking_lot::RwLock<AgentStatus>>,
    submit_policy: PtySubmitPolicy,
    writer: Arc<Mutex<SendWriter>>,
    reader: Arc<Mutex<SendReader>>,
    child: Arc<Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>>,
    #[cfg(windows)]
    child_pid: Option<u32>,
    master: Arc<Mutex<MasterPtyHandle>>,
    /// Bounded, offset-addressed history of everything the child wrote to the PTY.
    ///
    /// A PTY merges stdout and stderr, and before #207 those bytes were forwarded to the
    /// UI and otherwise discarded — with no app handle (headless HTTP mode, and every
    /// test) no reader thread ran at all, so a CLI that died during startup left no trace
    /// in the process, on disk, or in the DB. Startup verification needs this text to say
    /// *why* a worker failed instead of just that it did. Since #287 the same ring also
    /// serves terminal replay, so a freshly mounted pane can show the agent's screen.
    output_ring: Arc<Mutex<OutputRing>>,
}

// Make PtySession Send + Sync
unsafe impl Send for PtySession {}
unsafe impl Sync for PtySession {}

impl PtySession {
    fn write_locked(writer: &mut SendWriter, data: &[u8]) -> Result<(), PtyError> {
        // Write in chunks to respect Windows pipe buffer limits.
        for chunk in data.chunks(CHUNK_SIZE) {
            let result = writer.0.write_all(chunk);
            if let Err(ref e) = result {
                tracing::error!("PTY write_all failed: {}", e);
                return Err(into_io_error(e));
            }
            let flush_result = writer.0.flush();
            if let Err(ref e) = flush_result {
                tracing::error!("PTY flush failed: {}", e);
                return Err(into_io_error(e));
            }
        }

        Ok(())
    }

    pub fn new(
        _id: String,
        role: AgentRole,
        command: &str,
        submit_policy: PtySubmitPolicy,
        identity_env: &[(String, String)],
        args: &[&str],
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
        replay_capacity: usize,
    ) -> Result<Self, PtyError> {
        use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

        tracing::info!("Creating PTY session: command={} args={:?} cwd={:?}", command, args, cwd);

        let pty_system = NativePtySystem::default();

        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::CreateError(e.to_string()))?;

        // On Windows, create a batch file to avoid shell quoting issues
        // This is the same pattern used by /hive command
        let mut cmd = if cfg!(windows) {
            // Create temp batch file with the full command
            let batch_content = Self::create_batch_content(command, args);
            let batch_path = Self::write_temp_batch(&batch_content)?;

            tracing::info!("Created batch file: {} with content:\n{}", batch_path.display(), batch_content);

            let mut cmd = CommandBuilder::new("cmd.exe");
            cmd.args(&["/c", &batch_path.to_string_lossy()]);
            cmd
        } else {
            let mut cmd = CommandBuilder::new(command);
            cmd.args(args);
            cmd
        };

        for key in ["HIVE_SESSION_ID", "HIVE_AGENT_ID", "HIVE_ROLE"] {
            cmd.env_remove(key);
        }
        for (key, value) in identity_env {
            cmd.env(key, value);
        }

        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }

        let mut child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::SpawnError(e.to_string()))?;
        #[cfg(windows)]
        let child_pid = child.process_id();

        // #175(d): the child is ALREADY RUNNING at this point. Returning `Err`
        // from either step below without killing it would leave a live process
        // that no `PtySession` owns — and the caller, seeing `Err`, releases the
        // worker's durable queue claim so a retry can spawn a second one. Any
        // failure past the spawn must therefore reap the child first.
        let writer = match pty_pair.master.take_writer() {
            Ok(writer) => writer,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(into_io_error(e));
            }
        };

        let reader = match pty_pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(into_io_error(e));
            }
        };

        // Keep the master alive - dropping it closes the PTY!
        let master = pty_pair.master;

        Ok(Self {
            role,
            status: Arc::new(parking_lot::RwLock::new(AgentStatus::Starting)),
            submit_policy,
            writer: Arc::new(Mutex::new(SendWriter(writer))),
            reader: Arc::new(Mutex::new(SendReader(reader))),
            child: Arc::new(Mutex::new(Some(child))),
            #[cfg(windows)]
            child_pid,
            master: Arc::new(Mutex::new(MasterPtyHandle(master))),
            output_ring: Arc::new(Mutex::new(OutputRing::new(replay_capacity))),
        })
    }

    /// Append `bytes` to the output ring, evicting the oldest bytes once the ring is
    /// full. Called by the reader thread for every chunk, whether or not the UI is
    /// attached. Returns the absolute byte offset at which `bytes` starts (#287).
    pub fn record_output(&self, bytes: &[u8]) -> u64 {
        self.output_ring.lock().record(bytes)
    }

    /// The retained tail of the child's output, lossily decoded. Fixed at the legacy
    /// 8 KB view so inject's before/after comparisons are unaffected by ring size.
    pub fn recent_output(&self) -> String {
        self.output_ring.lock().tail_lossy(RECENT_OUTPUT_VIEW_BYTES)
    }

    /// The whole retained history, aligned for a terminal to parse from its first byte.
    pub fn snapshot(&self) -> OutputSnapshot {
        self.output_ring.lock().snapshot()
    }

    pub fn write(&self, data: &[u8]) -> Result<(), PtyError> {
        tracing::debug!("PTY write: {} bytes: {:?}", data.len(), String::from_utf8_lossy(data));
        let mut writer = self.writer.lock();
        if data == Self::SUBMIT_KEYSTROKE {
            Self::write_submit_keystroke_locked(&mut writer)?;
        } else {
            Self::write_locked(&mut writer, data)?;
        }

        tracing::debug!("PTY write complete");
        Ok(())
    }

    // BEGIN SHARED SUBMIT KEYSTROKE PRIMITIVE
    const SUBMIT_KEYSTROKE: &'static [u8] = b"\r";

    fn write_submit_keystroke_locked(writer: &mut SendWriter) -> Result<usize, PtyError> {
        Self::write_locked(writer, Self::SUBMIT_KEYSTROKE)?;
        Ok(Self::SUBMIT_KEYSTROKE.len())
    }
    // END SHARED SUBMIT KEYSTROKE PRIMITIVE

    /// Deliver a payload inside a bracketed-paste envelope, then Enter as a discrete
    /// bare carriage return outside the envelope.
    ///
    /// The envelope is what makes the Enter register: a raw fast byte burst trips TUI
    /// paste-coalescing (codex suppresses Enter for ~120 ms after a burst), which used to
    /// swallow the follow-up `\r` as a literal newline inside the paste (#256). The end
    /// marker tells the composer the paste is over, so the Enter is interpreted as a
    /// keystroke rather than payload. An empty payload writes only the Enter, preserving
    /// the documented bare-Enter flush for content already staged in a composer.
    pub fn submit(&self, data: &[u8]) -> Result<PtySubmitResult, PtyError> {
        let mut writer = self.writer.lock();
        let payload_bytes_written = if data.is_empty() {
            0
        } else {
            Self::write_bracketed_locked(&mut writer, data)?
        };
        // Keep the per-session writer locked so no concurrent write can be
        // interleaved and accidentally submitted by this Enter.
        std::thread::sleep(submit_gap(self.submit_policy));
        let submit_bytes_written = Self::write_submit_keystroke_locked(&mut writer)?;
        Ok(PtySubmitResult {
            payload_bytes_written,
            submit_bytes_written,
        })
    }

    /// Write `data` as one bracketed-paste envelope while the caller already holds the
    /// writer lock. Returns the sanitized payload byte count (framing markers excluded).
    fn write_bracketed_locked(
        writer: &mut SendWriter,
        data: &[u8],
    ) -> Result<usize, PtyError> {
        let sanitized = sanitize_bracketed_paste(data);

        // Send bracketed paste start sequence
        writer.0.write_all(BRACKETED_PASTE_START)
            .map_err(into_io_error)?;
        writer.0.flush()
            .map_err(into_io_error)?;

        // Write data in chunks with flush between each
        for chunk in sanitized.as_ref().chunks(CHUNK_SIZE) {
            writer.0.write_all(chunk)
                .map_err(into_io_error)?;
            writer.0.flush()
                .map_err(into_io_error)?;
        }

        // Send bracketed paste end sequence
        writer.0.write_all(BRACKETED_PASTE_END)
            .map_err(into_io_error)?;
        writer.0.flush()
            .map_err(into_io_error)?;

        Ok(sanitized.as_ref().len())
    }

    /// Write with bracketed paste mode wrapping - used for paste operations
    pub fn write_bracketed(&self, data: &[u8]) -> Result<(), PtyError> {
        tracing::debug!("PTY write_bracketed: {} bytes", data.len());
        let mut writer = self.writer.lock();
        Self::write_bracketed_locked(&mut writer, data)?;
        tracing::debug!("PTY write_bracketed complete");
        Ok(())
    }

    pub fn kill(&self) -> Result<(), PtyError> {
        #[cfg(windows)]
        return self.kill_with_tree_killer(kill_windows_tree);

        #[cfg(not(windows))]
        {
            let mut child = self.child.lock();
            if let Some(ref mut c) = *child {
                if c.try_wait().map_err(into_io_error)?.is_some() {
                    return Ok(());
                }
                c.kill().map_err(into_io_error)?;
            }
            Ok(())
        }
    }

    #[cfg(windows)]
    fn kill_with_tree_killer(
        &self,
        tree_killer: impl FnOnce(u32) -> std::io::Result<()>,
    ) -> Result<(), PtyError> {
        let mut child = self.child.lock();
        if let Some(pid) = self.child_pid {
            // A reaped direct child says nothing about descendants. Always try the
            // recorded PID; /T can only enumerate children while that root exists.
            // Descendants reparented after root exit cannot be recovered by PID.
            match tree_killer(pid) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    tracing::warn!(pid, %error, "PTY process-tree kill failed; retaining cleanup target");
                    if let Some(ref mut c) = *child {
                        if c.try_wait().ok().flatten().is_none() {
                            let _ = c.kill();
                        }
                    }
                    return Err(PtyError::IoError(error));
                }
            }
        }
        if let Some(ref mut c) = *child {
            if c.try_wait().map_err(into_io_error)?.is_some() {
                return Ok(());
            }
            c.kill().map_err(into_io_error)?;
        }
        Ok(())
    }

    /// Check if the process is still running
    #[allow(dead_code)]
    pub fn is_alive(&self) -> bool {
        let mut child = self.child.lock();
        if let Some(ref mut c) = *child {
            // Try to check if the process is still running
            c.try_wait().ok().flatten().is_none()
        } else {
            false
        }
    }

    /// Gracefully terminate the process by sending Ctrl+C, waiting, then killing if needed
    #[allow(dead_code)]
    pub async fn graceful_terminate(&self) -> Result<(), PtyError> {
        // Send Ctrl+C for CLI tools
        self.write(b"\x03")?;

        // Wait up to 5 seconds for graceful exit
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        // Force kill if still running
        if self.is_alive() {
            self.kill()?;
        }

        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        let master = self.master.lock();
        master.resize(cols, rows)
    }

    pub fn get_reader(&self) -> Arc<Mutex<SendReader>> {
        Arc::clone(&self.reader)
    }

    /// Create batch file content for Windows command execution
    #[cfg(windows)]
    fn create_batch_content(command: &str, args: &[&str]) -> String {
        let mut lines = vec!["@echo off".to_string()];

        // Add CLI-specific environment variables
        if command == "opencode" {
            lines.push("set OPENCODE_YOLO=true".to_string());
        }

        // Build the command line with proper quoting
        let mut cmd_line = command.to_string();
        for arg in args {
            // Quote args that contain spaces or special characters
            if arg.contains(' ') || arg.contains('"') || arg.contains('&') || arg.contains('|') {
                // Escape any existing quotes and wrap in quotes
                let escaped = arg.replace('"', "\\\"");
                cmd_line.push_str(&format!(" \"{}\"", escaped));
            } else {
                cmd_line.push_str(&format!(" {}", arg));
            }
        }

        lines.push(cmd_line);
        lines.join("\r\n")
    }

    #[cfg(not(windows))]
    fn create_batch_content(_command: &str, _args: &[&str]) -> String {
        String::new()
    }

    /// Write a temporary batch file and return its path
    #[cfg(windows)]
    fn write_temp_batch(content: &str) -> Result<std::path::PathBuf, PtyError> {
        use std::io::Write;

        let temp_dir = std::env::temp_dir().join("hive-manager");
        std::fs::create_dir_all(&temp_dir)
            .map_err(|e| PtyError::CreateError(format!("Failed to create temp dir: {}", e)))?;

        // Generate unique filename
        let filename = format!("agent-{}.bat", uuid::Uuid::new_v4());
        let path = temp_dir.join(filename);

        let mut file = std::fs::File::create(&path)
            .map_err(|e| PtyError::CreateError(format!("Failed to create batch file: {}", e)))?;

        file.write_all(content.as_bytes())
            .map_err(|e| PtyError::CreateError(format!("Failed to write batch file: {}", e)))?;

        Ok(path)
    }

    #[cfg(not(windows))]
    fn write_temp_batch(_content: &str) -> Result<std::path::PathBuf, PtyError> {
        Err(PtyError::CreateError("Batch files only supported on Windows".to_string()))
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

// Helper function to read from SendReader
pub fn read_from_reader(reader: &Arc<Mutex<SendReader>>, buf: &mut [u8]) -> Result<usize, std::io::Error> {
    let mut r = reader.lock();
    r.0.read(buf)
}

#[cfg(test)]
mod tests {
    use super::{sanitize_bracketed_paste, BRACKETED_PASTE_END};

    #[cfg(windows)]
    mod windows_tree {
        use std::fs;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::thread;
        use std::time::{Duration, Instant};

        use tempfile::TempDir;

        use super::super::{AgentRole, PtySession};
        use crate::adapters::pty_submit_policy;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        fn pid_is_live(pid: u32) -> bool {
            Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command"])
                .arg(format!(
                    "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}"
                ))
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        }

        struct PingCleanup(Option<u32>);

        impl Drop for PingCleanup {
            fn drop(&mut self) {
                if let Some(pid) = self.0 {
                    let _ = Command::new("taskkill")
                        .arg("/PID")
                        .arg(pid.to_string())
                        .args(["/T", "/F"])
                        .creation_flags(CREATE_NO_WINDOW)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
        }

        #[test]
        fn killing_real_pty_session_reaps_ping_grandchild() {
            let temp = TempDir::new().unwrap();
            let pid_file = temp.path().join("ping.pid");
            let script_file = temp.path().join("spawn-ping.ps1");
            let escaped_pid_file = pid_file.to_string_lossy().replace('\'', "''");
            fs::write(
                &script_file,
                format!(
                    "$ErrorActionPreference = 'Stop'\n\
                     $child = Start-Process -FilePath ping.exe -ArgumentList '-n 600 127.0.0.1' -PassThru -NoNewWindow\n\
                     [IO.File]::WriteAllText('{escaped_pid_file}', [string]$child.Id)\n\
                     Wait-Process -Id $child.Id\n"
                ),
            ).unwrap();
            let script = script_file.to_string_lossy();
            let session = PtySession::new(
                "synthetic-tree-kill".to_string(),
                AgentRole::Worker { index: 1, parent: None },
                "powershell.exe",
                pty_submit_policy("powershell.exe"),
                &[],
                &["-NoProfile", "-NonInteractive", "-File", &script],
                temp.path().to_str(),
                80,
                24,
                8192,
            ).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let pid = loop {
                if let Ok(value) = fs::read_to_string(&pid_file) {
                    if let Ok(pid) = value.trim().parse::<u32>() {
                        break pid;
                    }
                }
                assert!(Instant::now() < deadline, "ping PID was not reported");
                thread::sleep(Duration::from_millis(50));
            };
            let mut cleanup = PingCleanup(Some(pid));
            assert!(pid_is_live(pid), "ping grandchild exited before kill");
            session.kill().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while pid_is_live(pid) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(50));
            }
            assert!(!pid_is_live(pid), "ping grandchild {pid} survived PTY kill");
            cleanup.0 = None;
        }

        #[test]
        fn killing_already_exited_real_pty_session_is_idempotent() {
            let temp = TempDir::new().unwrap();
            let session = PtySession::new(
                "synthetic-exited".to_string(),
                AgentRole::Worker { index: 1, parent: None },
                "cmd.exe",
                pty_submit_policy("cmd.exe"),
                &[],
                &["/c", "exit", "0"],
                temp.path().to_str(),
                80,
                24,
                8192,
            ).unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            while session.is_alive() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }
            assert!(!session.is_alive(), "synthetic child did not exit");
            let mut attempted_pid = None;
            session.kill_with_tree_killer(|pid| {
                attempted_pid = Some(pid);
                Ok(())
            }).unwrap();
            assert_eq!(attempted_pid, session.child_pid);
            session.kill().unwrap();
            session.kill().unwrap();
        }

        #[test]
        fn failed_tree_kill_of_exited_root_remains_retryable() {
            let temp = TempDir::new().unwrap();
            let session = PtySession::new(
                "synthetic-exited-failure".to_string(),
                AgentRole::Worker { index: 1, parent: None },
                "cmd.exe",
                pty_submit_policy("cmd.exe"),
                &[],
                &["/c", "exit", "0"],
                temp.path().to_str(),
                80,
                24,
                8192,
            ).unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            while session.is_alive() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }
            assert!(!session.is_alive());
            assert!(session.kill_with_tree_killer(|_| Err(std::io::Error::other("partial failure"))).is_err());
            let mut retried = false;
            session.kill_with_tree_killer(|_| {
                retried = true;
                Ok(())
            }).unwrap();
            assert!(retried);
        }
    }

    #[test]
    fn sanitize_bracketed_paste_removes_end_sequence_from_payload() {
        let payload = b"hello\x1b[201~world\x1b[201~!";
        let sanitized = sanitize_bracketed_paste(payload);

        assert_eq!(sanitized.as_ref(), b"helloworld!");
        assert!(!sanitized.as_ref().windows(BRACKETED_PASTE_END.len()).any(|w| w == BRACKETED_PASTE_END));
    }
}
