//! Runtime Adapter module - abstracts process execution backends behind a common trait.
//!
//! This module provides the `RuntimeAdapter` trait for abstracting runtime-specific
//! behavior such as process launching, stopping, input writing, and terminal resizing.
//!
//! # Available Runtimes
//!
//! - `LocalPtyRuntime`: PTY-based execution with terminal emulation (via portable_pty)
//! - `LocalProcessRuntime`: Headless process execution (via `std::process::Command`)

mod local_process;
mod local_pty;
mod worktree;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use local_process::LocalProcessRuntime;
pub use local_pty::LocalPtyRuntime;
pub use worktree::{WorktreeError, WorktreeInfo, WorktreeManager};

/// Specification for launching an agent process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchSpec {
    /// Binary to execute
    pub command: String,
    /// Arguments to pass
    pub args: Vec<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Working directory
    pub cwd: Option<PathBuf>,
    /// Initial terminal columns
    pub cols: u16,
    /// Initial terminal rows
    pub rows: u16,
    /// Agent role for context
    pub role: String,
    /// Optional WSL distribution override for Windows-backed launches.
    #[serde(default)]
    pub wsl_distro: Option<String>,
    /// Optional WSL binary path override for Windows-backed launches.
    #[serde(default)]
    pub wsl_binary_path: Option<String>,
}

impl LaunchSpec {
    /// Create a new launch spec with the given command.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            cols: 80,
            rows: 24,
            role: "worker".to_string(),
            wsl_distro: None,
            wsl_binary_path: None,
        }
    }

    /// Add an argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add multiple arguments.
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set the working directory.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Set an environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Add multiple environment variables.
    pub fn envs(mut self, envs: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>) -> Self {
        self.env.extend(envs.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Set the terminal size.
    pub fn terminal_size(mut self, cols: u16, rows: u16) -> Self {
        self.cols = cols;
        self.rows = rows;
        self
    }

    /// Set the agent role.
    pub fn role(mut self, role: impl Into<String>) -> Self {
        self.role = role.into();
        self
    }

    /// Override the WSL distribution used for Windows-backed launches.
    pub fn wsl_distro(mut self, distro: impl Into<String>) -> Self {
        self.wsl_distro = Some(distro.into());
        self
    }

    /// Override the WSL binary path used for Windows-backed launches.
    pub fn wsl_binary_path(mut self, binary_path: impl Into<String>) -> Self {
        self.wsl_binary_path = Some(binary_path.into());
        self
    }
}

/// Information about a launched agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchedAgent {
    /// Unique process identifier
    pub process_id: String,
    /// Current status
    pub status: AgentProcessStatus,
}

/// Status of a launched agent process.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentProcessStatus {
    /// Process is starting up
    Starting,
    /// Process is running normally
    Running,
    /// Process has completed
    Completed,
    /// Process failed
    Failed,
    /// Process was killed
    Killed,
}

/// Error type for runtime operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeError {
    /// Error message
    pub message: String,
    /// Error kind
    pub kind: RuntimeErrorKind,
}

impl RuntimeError {
    /// Create a new runtime error.
    pub fn new(kind: RuntimeErrorKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }

    /// Create a launch error.
    pub fn launch(message: impl Into<String>) -> Self {
        Self::new(RuntimeErrorKind::Launch, message)
    }

    /// Create a stop error.
    pub fn stop(message: impl Into<String>) -> Self {
        Self::new(RuntimeErrorKind::Stop, message)
    }

    /// Create a write error.
    pub fn write(message: impl Into<String>) -> Self {
        Self::new(RuntimeErrorKind::Write, message)
    }

    /// Create a resize error.
    pub fn resize(message: impl Into<String>) -> Self {
        Self::new(RuntimeErrorKind::Resize, message)
    }

    /// Create a not found error.
    pub fn not_found(process_id: impl Into<String>) -> Self {
        Self::new(RuntimeErrorKind::NotFound, format!("Process not found: {}", process_id.into()))
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?}] {}", self.kind, self.message)
    }
}

impl std::error::Error for RuntimeError {}

/// Kind of runtime error.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeErrorKind {
    /// Failed to launch process
    Launch,
    /// Failed to stop process
    Stop,
    /// Failed to write to process
    Write,
    /// Failed to resize terminal
    Resize,
    /// Process not found
    NotFound,
    /// Internal error
    Internal,
}

/// Trait for runtime adapter implementations.
///
/// Different execution backends (PTY, headless process, container, remote)
/// implement this trait to provide a common interface for agent execution.
///
/// # Thread Safety
///
/// Implementations must be `Send + Sync` to allow sharing across async contexts.
pub trait RuntimeAdapter: Send + Sync {
    /// Launch a new agent process.
    ///
    /// Returns a `LaunchedAgent` with the process ID and initial status.
    fn launch(&self, spec: &LaunchSpec) -> Result<LaunchedAgent, RuntimeError>;

    /// Stop a running agent process.
    ///
    /// Returns an error if the process cannot be stopped or doesn't exist.
    fn stop(&self, process_id: &str) -> Result<(), RuntimeError>;

    /// Write input to a running agent process.
    ///
    /// For PTY-based runtimes, this writes to the PTY's stdin.
    /// For headless runtimes, this writes to the process's stdin.
    fn write(&self, process_id: &str, input: &str) -> Result<(), RuntimeError>;

    /// Resize the terminal for a running agent process.
    ///
    /// Only applicable for PTY-based runtimes.
    /// Headless runtimes should return `Ok(())` or an error.
    fn resize(&self, process_id: &str, cols: u16, rows: u16) -> Result<(), RuntimeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_launch_spec_builder() {
        let spec = LaunchSpec::new("claude")
            .arg("--dangerously-skip-permissions")
            .arg("--model")
            .arg("opus")
            .cwd("/project")
            .env("KEY", "value")
            .terminal_size(120, 40)
            .role("worker")
            .wsl_distro("Ubuntu-24.04")
            .wsl_binary_path("/custom/agent");

        assert_eq!(spec.command, "claude");
        assert_eq!(spec.args, vec!["--dangerously-skip-permissions", "--model", "opus"]);
        assert_eq!(spec.cwd, Some(PathBuf::from("/project")));
        assert_eq!(spec.env.get("KEY"), Some(&"value".to_string()));
        assert_eq!(spec.cols, 120);
        assert_eq!(spec.rows, 40);
        assert_eq!(spec.role, "worker");
        assert_eq!(spec.wsl_distro.as_deref(), Some("Ubuntu-24.04"));
        assert_eq!(spec.wsl_binary_path.as_deref(), Some("/custom/agent"));
    }

    #[test]
    fn test_runtime_error_constructors() {
        let err = RuntimeError::launch("Failed to start");
        assert_eq!(err.kind, RuntimeErrorKind::Launch);

        let err = RuntimeError::not_found("agent-1");
        assert_eq!(err.kind, RuntimeErrorKind::NotFound);
        assert!(err.message.contains("agent-1"));
    }

    #[test]
    fn test_runtime_error_display() {
        let err = RuntimeError::launch("Failed to start");
        let display = format!("{}", err);
        assert!(display.contains("Launch"));
        assert!(display.contains("Failed to start"));
    }

    #[test]
    fn test_launched_agent() {
        let agent = LaunchedAgent {
            process_id: "proc-123".to_string(),
            status: AgentProcessStatus::Starting,
        };
        assert_eq!(agent.process_id, "proc-123");
        assert_eq!(agent.status, AgentProcessStatus::Starting);
    }
}
