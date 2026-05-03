//! CLI Adapter module - normalizes different AI agent CLIs behind a common trait.
//!
//! This module provides the `CliAdapter` trait for abstracting CLI-specific
//! behavior such as command building, signal detection, and bootstrap prompts.

mod claude_code;
mod codex;
mod cursor;
mod droid;
mod gemini;
mod opencode;
mod qwen;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use claude_code::ClaudeCodeAdapter;
pub use codex::CodexAdapter;
pub use cursor::CursorAdapter;
pub use droid::DroidAdapter;
pub use gemini::GeminiAdapter;
pub use opencode::OpenCodeAdapter;
pub use qwen::QwenAdapter;

/// Valid CLI names allowed in the system.
pub const VALID_CLIS: &[&str] = &["claude", "gemini", "codex", "opencode", "cursor", "droid", "qwen"];

/// Validate a CLI name against the allowlist.
pub fn is_valid_cli(cli: &str) -> bool {
    VALID_CLIS.contains(&cli)
}

/// Specification for launching an agent process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLaunchSpec {
    /// CLI name (e.g., "claude", "gemini")
    pub cli: String,
    /// Model identifier (e.g., "opus", "gemini-2.5-pro")
    pub model: Option<String>,
    /// Additional CLI flags
    pub flags: Vec<String>,
    /// Working directory for the process
    pub cwd: PathBuf,
    /// Environment variables to set
    pub env: HashMap<String, String>,
    /// Prompt file path (for file-based prompts)
    pub prompt_file: Option<PathBuf>,
    /// Inline task prompt (for solo mode)
    pub inline_task: Option<String>,
    /// Agent role for context
    pub role: String,
    /// Agent label for display
    pub label: Option<String>,
}

/// Command ready for execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LaunchCommand {
    /// Binary to execute
    pub binary: String,
    /// Arguments to pass
    pub args: Vec<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Working directory
    pub cwd: PathBuf,
}

impl LaunchCommand {
    /// Create a new launch command.
    pub fn new(binary: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: cwd.into(),
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
}

/// Signal detected from CLI output indicating agent status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentSignal {
    /// Agent completed its task successfully
    Completed,
    /// Agent encountered an error
    Failed { message: String },
    /// Agent is waiting for user input
    WaitingInput,
    /// Agent made a tool call (action taken)
    ToolCall { tool: String },
    /// Agent is processing/thinking
    Processing,
}

/// Context for building bootstrap prompts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapContext {
    /// Session ID
    pub session_id: String,
    /// Agent ID
    pub agent_id: String,
    /// Agent role
    pub role: String,
    /// Task file path
    pub task_file: Option<PathBuf>,
    /// Project path
    pub project_path: PathBuf,
    /// Additional context variables
    pub variables: HashMap<String, String>,
}

/// Trait for CLI adapter implementations.
///
/// Each AI agent CLI (Claude, Codex, Gemini, etc.) has different:
/// - Command-line flags for auto-approve and model selection
/// - Output formats and status signals
/// - Bootstrap prompt conventions
///
/// This trait normalizes these differences behind a common interface.
pub trait CliAdapter: Send + Sync {
    /// Returns the CLI name (e.g., "claude", "gemini").
    fn cli_name(&self) -> &'static str;

    /// Builds the launch command for the agent.
    fn build_launch_command(&self, spec: &AgentLaunchSpec) -> LaunchCommand;

    /// Detects status signals from CLI output lines.
    /// Returns None if the line doesn't indicate a status change.
    fn detect_status_signal(&self, line: &str) -> Option<AgentSignal>;

    /// Builds a bootstrap prompt for the agent.
    fn build_bootstrap_prompt(&self, context: &BootstrapContext) -> String;

    /// Returns the auto-approve flag for this CLI, if any.
    fn auto_approve_flag(&self) -> Option<&'static str> {
        None
    }

    /// Returns the model flag for this CLI, if any.
    fn model_flag(&self) -> Option<&'static str> {
        None
    }

    /// Returns the prompt flag for this CLI, if any.
    fn prompt_flag(&self) -> Option<&'static str> {
        None
    }
}

/// Get the appropriate adapter for a CLI name.
pub fn get_adapter(cli: &str) -> Result<Box<dyn CliAdapter>, String> {
    match cli {
        "claude" => Ok(Box::new(ClaudeCodeAdapter)),
        "codex" => Ok(Box::new(CodexAdapter)),
        "cursor" => Ok(Box::new(CursorAdapter)),
        "gemini" => Ok(Box::new(GeminiAdapter)),
        "droid" => Ok(Box::new(DroidAdapter)),
        "opencode" => Ok(Box::new(OpenCodeAdapter)),
        "qwen" => Ok(Box::new(QwenAdapter)),
        _ => Err(format!("Unknown CLI adapter: {}", cli)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_clis() {
        assert!(is_valid_cli("claude"));
        assert!(is_valid_cli("gemini"));
        assert!(is_valid_cli("codex"));
        assert!(is_valid_cli("opencode"));
        assert!(is_valid_cli("cursor"));
        assert!(is_valid_cli("droid"));
        assert!(is_valid_cli("qwen"));
        assert!(!is_valid_cli("unknown"));
    }

    #[test]
    fn test_launch_command_builder() {
        let cmd = LaunchCommand::new("claude", "/project")
            .arg("--dangerously-skip-permissions")
            .arg("--model")
            .arg("opus")
            .env("KEY", "value");

        assert_eq!(cmd.binary, "claude");
        assert_eq!(cmd.args, vec!["--dangerously-skip-permissions", "--model", "opus"]);
        assert_eq!(cmd.env.get("KEY"), Some(&"value".to_string()));
    }

    #[test]
    fn test_get_adapter() {
        let claude = get_adapter("claude").unwrap();
        assert_eq!(claude.cli_name(), "claude");

        let gemini = get_adapter("gemini").unwrap();
        assert_eq!(gemini.cli_name(), "gemini");

        let cursor = get_adapter("cursor").unwrap();
        assert_eq!(cursor.cli_name(), "cursor");

        let qwen = get_adapter("qwen").unwrap();
        assert_eq!(qwen.cli_name(), "qwen");
    }

    #[test]
    fn test_get_adapter_rejects_unknown_cli() {
        match get_adapter("unknown") {
            Ok(adapter) => panic!("Expected error, got adapter {}", adapter.cli_name()),
            Err(error) => assert_eq!(error, "Unknown CLI adapter: unknown"),
        }
    }
}
