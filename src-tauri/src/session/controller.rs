use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::cli::{CliRegistry, CliBehavior};
use crate::pty::{AgentRole, AgentStatus, AgentConfig, PtyManager, WorkerRole};
use crate::storage::SessionStorage;
use crate::coordination::{HierarchyNode, StateManager, WorkerStateInfo};
use crate::watcher::TaskFileWatcher;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionType {
    Hive { worker_count: u8 },
    Swarm { planner_count: u8 },
    Fusion { variants: Vec<String> },
    Solo { cli: String, model: Option<String> },
}

#[derive(Debug)]
pub enum SessionError {
    NotFound(String),
    ConfigError(String),
    SpawnError(String),
    TerminationError(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::NotFound(s) => write!(f, "Session not found: {}", s),
            SessionError::ConfigError(s) => write!(f, "Config error: {}", s),
            SessionError::SpawnError(s) => write!(f, "Spawn error: {}", s),
            SessionError::TerminationError(s) => write!(f, "Termination error: {}", s),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<String> for SessionError {
    fn from(s: String) -> Self {
        SessionError::ConfigError(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionState {
    Planning,      // Master Planner is running
    PlanReady,     // Plan generated, waiting for user to continue
    Starting,
    SpawningWorker(u8),      // Which worker is being spawned (sequential mode)
    WaitingForWorker(u8),    // Which worker we're waiting on (sequential mode)
    SpawningPlanner(u8),     // Which planner is being spawned (Swarm sequential mode)
    WaitingForPlanner(u8),   // Which planner we're waiting on (Swarm sequential mode)
    SpawningFusionVariant(u8),    // Which fusion variant is being spawned
    WaitingForFusionVariants,     // All variants running, waiting for completion
    SpawningJudge,                // Launching judge after all variants complete
    Judging,                      // Judge evaluating implementations
    AwaitingVerdictSelection,     // User choosing winner
    MergingWinner,                // Merging winning variant
    Running,
    Paused,
    Completed,
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub role: AgentRole,
    pub status: AgentStatus,
    pub config: AgentConfig,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiveLaunchConfig {
    pub project_path: String,
    pub queen_config: AgentConfig,
    pub workers: Vec<AgentConfig>,
    pub prompt: Option<String>,
    #[serde(default)]
    pub with_planning: bool,  // If true, spawn Master Planner first
    #[serde(default)]
    pub smoke_test: bool,     // If true, create a minimal test plan without real investigation
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmLaunchConfig {
    pub project_path: String,
    pub queen_config: AgentConfig,
    pub planner_count: u8,                    // How many planners
    pub planner_config: AgentConfig,          // Config shared by all planners
    pub workers_per_planner: Vec<AgentConfig>, // Workers shared config (applied to each planner)
    pub prompt: Option<String>,
    #[serde(default)]
    pub with_planning: bool,  // If true, spawn Master Planner first
    #[serde(default)]
    pub smoke_test: bool,     // If true, create a minimal test plan without real investigation

    // Legacy support - if planners vec is provided, use it instead
    #[serde(default)]
    pub planners: Vec<PlannerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerConfig {
    pub config: AgentConfig,
    pub domain: String,
    pub workers: Vec<AgentConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionLaunchConfig {
    pub project_path: String,
    pub variants: Vec<FusionVariantConfig>,
    pub task_description: String,
    pub judge_config: AgentConfig,
    #[serde(default)]
    pub queen_config: Option<AgentConfig>,
    #[serde(default)]
    pub with_planning: bool,
    #[serde(default = "default_fusion_cli")]
    pub default_cli: String,
    pub default_model: Option<String>,
}

fn default_fusion_cli() -> String {
    "claude".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionVariantConfig {
    pub name: String,
    pub cli: String,
    pub model: Option<String>,
    #[serde(default)]
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FusionSessionMetadata {
    base_branch: String,
    variants: Vec<FusionVariantMetadata>,
    judge_config: AgentConfig,
    task_description: String,
    decision_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FusionVariantMetadata {
    index: u8,
    name: String,
    slug: String,
    branch: String,
    worktree_path: String,
    task_file: String,
    agent_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionVariantStatus {
    pub index: u8,
    pub name: String,
    pub branch: String,
    pub worktree_path: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub session_type: SessionType,
    pub project_path: PathBuf,
    pub state: SessionState,
    pub created_at: DateTime<Utc>,
    pub agents: Vec<AgentInfo>,
    pub default_cli: String,
    pub default_model: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct SessionUpdate {
    pub session: Session,
}

/// Per-agent heartbeat data for stall detection
#[derive(Debug, Clone)]
pub struct AgentHeartbeatInfo {
    pub last_activity: DateTime<Utc>,
    pub status: String,
    pub summary: Option<String>,
}

pub struct SessionController {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    pty_manager: Arc<RwLock<PtyManager>>,
    app_handle: Option<AppHandle>,
    storage: Option<Arc<SessionStorage>>,
    task_watchers: Mutex<HashMap<String, TaskFileWatcher>>,
    /// session_id -> agent_id -> heartbeat info
    agent_heartbeats: Arc<RwLock<HashMap<String, HashMap<String, AgentHeartbeatInfo>>>>,
}

// Explicitly implement Send + Sync
unsafe impl Send for SessionController {}
unsafe impl Sync for SessionController {}

/// Generate CLI-specific polling instructions based on the CLI's behavioral profile
fn get_polling_instructions(cli: &str, task_file: &str) -> String {
    match CliRegistry::get_behavior(cli) {
        CliBehavior::ExplicitPolling => {
            format!(
                r#"
## Polling Protocol (MANDATORY)
Run this bash loop to wait for task activation:
```bash
while true; do
  STATUS=$(grep "^## Status:" "{}" | head -1)
  if [[ "$STATUS" == *"ACTIVE"* ]]; then break; fi
  sleep 30
done
```
"#,
                task_file
            )
        }
        CliBehavior::ActionProne => {
            format!(
                r#"
## WAIT FOR ACTIVATION (CRITICAL)
WARNING: You MUST wait for your task file Status to become ACTIVE.
WARNING: Do NOT start working just because you received this prompt.
WARNING: Read {} - if Status is STANDBY, WAIT.

Check the file, then wait. Do not proceed until ACTIVE.
"#,
                task_file
            )
        }
        CliBehavior::InstructionFollowing => {
            format!(
                r#"
## Task Coordination
Read {}. Begin work only when Status is ACTIVE.
"#,
                task_file
            )
        }
        CliBehavior::Interactive => {
            format!(
                r#"
## Task Coordination
Read {}. Begin work only when Status is ACTIVE.
Use the interactive interface to monitor your task file.
"#,
                task_file
            )
        }
    }
}

impl SessionController {
    pub fn new(pty_manager: Arc<RwLock<PtyManager>>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            pty_manager,
            app_handle: None,
            storage: None,
            task_watchers: Mutex::new(HashMap::new()),
            agent_heartbeats: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn set_app_handle(&mut self, handle: AppHandle) {
        self.app_handle = Some(handle.clone());
        let mut pty_manager = self.pty_manager.write();
        pty_manager.set_app_handle(handle);
    }

    pub fn set_storage(&mut self, storage: Arc<SessionStorage>) {
        self.storage = Some(storage);
    }

    pub fn launch_hive(
        &self,
        project_path: PathBuf,
        worker_count: u8,
        command: &str,
        prompt: Option<String>,
    ) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();
        let mut agents = Vec::new();
        let prompt_str = prompt.unwrap_or_default();
        let cwd = project_path.to_str().unwrap_or(".");

        // Parse command - support "command arg1 arg2" format
        let parts: Vec<&str> = command.split_whitespace().collect();
        let (cmd, base_args) = if parts.is_empty() {
            ("cmd.exe", vec![])
        } else {
            (parts[0], parts[1..].to_vec())
        };

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent
            let queen_id = format!("{}-queen", session_id);
            let mut queen_args = base_args.clone();

            // Add prompt as positional argument if provided and command is claude
            if cmd == "claude" && !prompt_str.is_empty() {
                queen_args.push(&prompt_str);
            }

            tracing::info!("Launching Queen agent: {} {:?} in {:?}", cmd, queen_args, project_path);

            pty_manager
                .create_session(
                    queen_id.clone(),
                    AgentRole::Queen,
                    cmd,
                    &queen_args.iter().map(|s| *s).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| {
                    let err_msg = format!("Failed to spawn Queen: {}", e);
                    tracing::error!("{}", err_msg);
                    err_msg
                })?;

            let queen_config = AgentConfig {
                cli: cmd.to_string(),
                model: if cmd == "claude" { Some("opus-4-6".to_string()) } else { None },
                flags: base_args.iter().map(|s| s.to_string()).collect(),
                label: None,
                role: None,
                initial_prompt: None,
            };

            agents.push(AgentInfo {
                id: queen_id,
                role: AgentRole::Queen,
                status: AgentStatus::Running,
                config: queen_config,
                parent_id: None,
            });

            // Create Worker agents
            for i in 1..=worker_count {
                let worker_id = format!("{}-worker-{}", session_id, i);
                let worker_args = base_args.clone();

                tracing::info!("Launching Worker {} agent: {} {:?} in {:?}", i, cmd, worker_args, project_path);

                pty_manager
                    .create_session(
                        worker_id.clone(),
                        AgentRole::Worker { index: i, parent: None },
                        cmd,
                        &worker_args.iter().map(|s| *s).collect::<Vec<_>>(),
                        Some(cwd),
                        120,
                        30,
                    )
                    .map_err(|e| {
                        let err_msg = format!("Failed to spawn Worker {}: {}", i, e);
                        tracing::error!("{}", err_msg);
                        err_msg
                    })?;

                let worker_config = AgentConfig {
                    cli: cmd.to_string(),
                    model: if cmd == "claude" { Some("opus-4-6".to_string()) } else { None },
                    flags: worker_args.iter().map(|s| s.to_string()).collect(),
                    label: None,
                    role: None,
                    initial_prompt: None,
                };

                agents.push(AgentInfo {
                    id: worker_id.clone(),
                    role: AgentRole::Worker { index: i, parent: Some(format!("{}-queen", session_id)) },
                    status: AgentStatus::Running,
                    config: worker_config,
                    parent_id: Some(format!("{}-queen", session_id)),
                });
            }
        }

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Hive { worker_count },
            project_path,
            state: SessionState::Running,
            created_at: Utc::now(),
            agents,
            default_cli: cmd.to_string(),
            default_model: if cmd == "claude" { Some("opus-4-6".to_string()) } else { None },
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        Ok(session)
    }

    pub fn get_session(&self, id: &str) -> Option<Session> {
        let sessions = self.sessions.read();
        sessions.get(id).cloned()
    }

    /// Get the default CLI for a session
    pub fn get_session_default_cli(&self, session_id: &str) -> Option<String> {
        let sessions = self.sessions.read();
        sessions.get(session_id).map(|s| s.default_cli.clone())
    }

    pub fn list_sessions(&self) -> Vec<Session> {
        let sessions = self.sessions.read();
        sessions.values().cloned().collect()
    }

    // --- Heartbeat / Stall Detection ---

    /// Update heartbeat for an agent. Emits Tauri event if status changed.
    pub fn update_heartbeat(
        &self,
        session_id: &str,
        agent_id: &str,
        status: &str,
        summary: Option<&str>,
    ) -> Result<(), String> {
        let now = Utc::now();
        let prev_status = {
            let mut heartbeats = self.agent_heartbeats.write();
            let session_map = heartbeats.entry(session_id.to_string()).or_default();
            let prev = session_map.get(agent_id).map(|h| h.status.clone());
            session_map.insert(
                agent_id.to_string(),
                AgentHeartbeatInfo {
                    last_activity: now,
                    status: status.to_string(),
                    summary: summary.map(String::from),
                },
            );
            prev
        };
        let status_changed = prev_status.as_ref().map(|s| s != status).unwrap_or(true);
        if status_changed {
            if let Some(ref app_handle) = self.app_handle {
                let _ = app_handle.emit("heartbeat-status-changed", serde_json::json!({
                    "session_id": session_id,
                    "agent_id": agent_id,
                    "status": status,
                    "summary": summary,
                }));
            }
        }
        Ok(())
    }

    /// Get agents with no activity for longer than threshold.
    pub fn get_stalled_agents(
        &self,
        session_id: &str,
        threshold: Duration,
    ) -> Vec<(String, DateTime<Utc>)> {
        let now = Utc::now();
        let threshold_secs = threshold.as_secs() as i64;
        let heartbeats = self.agent_heartbeats.read();
        let Some(agents) = heartbeats.get(session_id) else {
            return vec![];
        };
        agents
            .iter()
            .filter_map(|(agent_id, info)| {
                let elapsed = (now - info.last_activity).num_seconds();
                if elapsed > threshold_secs && info.status != "completed" {
                    Some((agent_id.clone(), info.last_activity))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get heartbeat info for a session (for active sessions endpoint).
    pub fn get_heartbeat_info(&self, session_id: &str) -> HashMap<String, AgentHeartbeatInfo> {
        let heartbeats = self.agent_heartbeats.read();
        heartbeats
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    fn emit_session_update(&self, session_id: &str) {
        let session = {
            let sessions = self.sessions.read();
            sessions.get(session_id).cloned()
        };

        if let (Some(app_handle), Some(session)) = (self.app_handle.as_ref(), session) {
            let _ = app_handle.emit("session-update", SessionUpdate { session });
        }
    }

    /// Insert a session directly (for testing purposes only)
    #[cfg(test)]
    pub fn insert_test_session(&self, session: Session) {
        let mut sessions = self.sessions.write();
        sessions.insert(session.id.clone(), session);
    }

    pub fn stop_session(&self, id: &str) -> Result<(), String> {
        let session = {
            let sessions = self.sessions.read();
            sessions.get(id).cloned()
        };

        if let Some(session) = session {
            let pty_manager = self.pty_manager.read();
            for agent in &session.agents {
                let _ = pty_manager.kill(&agent.id);
            }

            {
                let mut sessions = self.sessions.write();
                if let Some(s) = sessions.get_mut(id) {
                    s.state = SessionState::Completed;
                }
            }

            if let Some(ref app_handle) = self.app_handle {
                let sessions = self.sessions.read();
                if let Some(session) = sessions.get(id) {
                    let _ = app_handle.emit("session-update", SessionUpdate {
                        session: session.clone(),
                    });
                }
            }

            Ok(())
        } else {
            Err(format!("Session not found: {}", id))
        }
    }

    pub fn stop_agent(&self, session_id: &str, agent_id: &str) -> Result<(), String> {
        let pty_manager = self.pty_manager.read();
        pty_manager.kill(agent_id).map_err(|e| e.to_string())?;

        {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                if let Some(agent) = session.agents.iter_mut().find(|a| a.id == agent_id) {
                    agent.status = AgentStatus::Completed;
                }
            }
        }

        Ok(())
    }

    /// Build command and args from AgentConfig
    /// Returns (command, args) with CLI-specific flags already added
    fn build_command(config: &AgentConfig) -> (String, Vec<String>) {
        let mut args = Vec::new();

        // Add CLI-specific flags
        match config.cli.as_str() {
            "claude" => {
                // Claude CLI requires --dangerously-skip-permissions for automated use
                args.push("--dangerously-skip-permissions".to_string());
                if let Some(ref model) = config.model {
                    args.push("--model".to_string());
                    args.push(model.clone());
                }
            }
            "gemini" => {
                // Gemini CLI uses -y for auto-approve
                args.push("-y".to_string());
                if let Some(ref model) = config.model {
                    args.push("-m".to_string());
                    args.push(model.clone());
                }
            }
            "codex" => {
                // Codex CLI uses --dangerously-bypass-approvals-and-sandbox
                args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
                if let Some(ref model) = config.model {
                    args.push("-m".to_string());
                    args.push(model.clone());
                }
            }
            "opencode" => {
                // OpenCode relies on OPENCODE_YOLO=true env var (set in batch file)
                if let Some(ref model) = config.model {
                    args.push("-m".to_string());
                    args.push(model.clone());
                }
            }
            "cursor" => {
                // Cursor Agent via WSL - interactive TUI mode
                args.push("-d".to_string());
                args.push("Ubuntu".to_string());
                args.push("/root/.local/bin/agent".to_string());
                args.push("--force".to_string());  // Auto-approve commands
                // Cursor uses global model setting, no --model flag
            }
            "droid" => {
                // Droid CLI - interactive TUI mode
                // Model selected via /model command or config
                // No auto-approve flag available in interactive mode
            }
            "qwen" => {
                // Qwen Code CLI - interactive mode with auto-approve
                args.push("-y".to_string());  // YOLO mode for auto-approve
                if let Some(ref model) = config.model {
                    args.push("-m".to_string());
                    args.push(model.clone());
                }
            }
            _ => {
                // For other CLIs, just add model flag if specified
                if let Some(ref model) = config.model {
                    args.push("--model".to_string());
                    args.push(model.clone());
                }
            }
        }

        // Add any extra flags from config
        args.extend(config.flags.clone());

        // Determine the actual command to run
        let command = match config.cli.as_str() {
            "cursor" => "wsl".to_string(),  // Cursor runs via WSL
            _ => config.cli.clone(),         // Others use CLI name as command
        };

        (command, args)
    }

    /// Add prompt argument to args based on CLI type
    /// Each CLI has different syntax for accepting initial prompts
    fn add_prompt_to_args(cli: &str, args: &mut Vec<String>, prompt_path: &str) {
        let prompt_arg = format!("Read {} and execute.", prompt_path);
        match cli {
            "claude" | "codex" | "cursor" | "droid" => {
                // Claude, Codex, Cursor, Droid accept prompt as positional argument
                args.push(prompt_arg);
            }
            "qwen" => {
                // Qwen uses -i for interactive mode with initial prompt
                args.push("-i".to_string());
                args.push(prompt_arg);
            }
            "gemini" => {
                // Gemini uses -i flag for initial prompt
                args.push("-i".to_string());
                args.push(prompt_arg);
            }
            "opencode" => {
                // OpenCode uses --prompt flag
                args.push("--prompt".to_string());
                args.push(prompt_arg);
            }
            _ => {
                // Default: try positional argument
                args.push(prompt_arg);
            }
        }
    }

    /// Add an inline task prompt to args based on CLI type (solo mode).
    /// This bypasses prompt files and uses each CLI's native prompt flag/convention.
    fn add_inline_task_to_args(cli: &str, args: &mut Vec<String>, task: &str) {
        match cli {
            "claude" | "gemini" => {
                args.push("-p".to_string());
                args.push(task.to_string());
            }
            "codex" => {
                args.push("-q".to_string());
                args.push(task.to_string());
            }
            "cursor" | "droid" => {
                args.push(task.to_string());
            }
            _ => {
                args.push(task.to_string());
            }
        }
    }

    /// Build command/args for solo launch.
    /// When task is Some, passes it inline via CLI flags (non-interactive).
    /// When task is None, opens the CLI in interactive mode.
    fn build_solo_command(config: &AgentConfig, task: Option<&str>) -> (String, Vec<String>) {
        let mut args = Vec::new();

        match config.cli.as_str() {
            "claude" => {
                if task.is_some() {
                    args.push("--dangerously-skip-permissions".to_string());
                }
                if let Some(task) = task {
                    Self::add_inline_task_to_args("claude", &mut args, task);
                }
                if let Some(ref model) = config.model {
                    args.push("--model".to_string());
                    args.push(model.clone());
                }
            }
            "gemini" => {
                if let Some(task) = task {
                    Self::add_inline_task_to_args("gemini", &mut args, task);
                }
                if let Some(ref model) = config.model {
                    args.push("--model".to_string());
                    args.push(model.clone());
                }
            }
            "droid" => {
                if let Some(task) = task {
                    Self::add_inline_task_to_args("droid", &mut args, task);
                }
                if let Some(ref model) = config.model {
                    args.push("--model".to_string());
                    args.push(model.clone());
                }
            }
            "codex" => {
                if let Some(task) = task {
                    Self::add_inline_task_to_args("codex", &mut args, task);
                }
                if let Some(ref model) = config.model {
                    args.push("--model".to_string());
                    args.push(model.clone());
                }
            }
            "cursor" => {
                if let Some(task) = task {
                    Self::add_inline_task_to_args("cursor", &mut args, task);
                }
            }
            _ => {
                if let Some(ref model) = config.model {
                    args.push("--model".to_string());
                    args.push(model.clone());
                }
                if let Some(task) = task {
                    Self::add_inline_task_to_args(&config.cli, &mut args, task);
                }
            }
        }

        args.extend(config.flags.clone());
        (config.cli.clone(), args)
    }

    fn run_git_in_dir(project_path: &PathBuf, args: &[&str]) -> Result<String, String> {
        if !project_path.exists() {
            return Err(format!("Project path does not exist: {}", project_path.display()));
        }

        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(project_path);

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run git {:?}: {}", args, e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let message = if !stderr.is_empty() { stderr } else { stdout };
            return Err(if message.is_empty() {
                format!("Git command failed: git {}", args.join(" "))
            } else {
                message
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn slugify_variant_name(name: &str) -> String {
        let mut out = String::new();
        let mut prev_dash = false;

        for ch in name.trim().chars() {
            let lowered = ch.to_ascii_lowercase();
            if lowered.is_ascii_alphanumeric() {
                out.push(lowered);
                prev_dash = false;
            } else if !prev_dash {
                out.push('-');
                prev_dash = true;
            }
        }

        let out = out.trim_matches('-').to_string();
        if out.is_empty() { "variant".to_string() } else { out }
    }

    fn unique_variant_slug(name: &str, seen: &mut HashMap<String, u16>) -> String {
        let base = Self::slugify_variant_name(name);
        let count = seen.entry(base.clone()).and_modify(|v| *v += 1).or_insert(1);
        if *count == 1 {
            base
        } else {
            format!("{}-{}", base, count)
        }
    }

    fn fusion_metadata_path(project_path: &PathBuf, session_id: &str) -> PathBuf {
        project_path
            .join(".hive-manager")
            .join(session_id)
            .join("fusion-config.json")
    }

    fn write_fusion_metadata(
        project_path: &PathBuf,
        session_id: &str,
        metadata: &FusionSessionMetadata,
    ) -> Result<(), String> {
        let metadata_path = Self::fusion_metadata_path(project_path, session_id);
        if let Some(parent) = metadata_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create fusion metadata dir: {}", e))?;
        }

        let json = serde_json::to_string_pretty(metadata)
            .map_err(|e| format!("Failed to serialize fusion metadata: {}", e))?;
        std::fs::write(&metadata_path, json)
            .map_err(|e| format!("Failed to write fusion metadata: {}", e))
    }

    fn read_fusion_metadata(project_path: &PathBuf, session_id: &str) -> Result<FusionSessionMetadata, String> {
        let metadata_path = Self::fusion_metadata_path(project_path, session_id);
        let json = std::fs::read_to_string(&metadata_path)
            .map_err(|e| format!("Failed to read fusion metadata {}: {}", metadata_path.display(), e))?;
        serde_json::from_str(&json)
            .map_err(|e| format!("Failed to parse fusion metadata: {}", e))
    }

    fn parse_task_status(content: &str) -> Option<String> {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(status) = trimmed.strip_prefix("## Status:") {
                return Some(status.trim().to_string());
            }
            if let Some(status) = trimmed.strip_prefix("**Status**:") {
                return Some(status.trim().to_string());
            }
        }
        None
    }

    fn read_task_status(task_path: &str) -> String {
        let path = PathBuf::from(task_path);
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => return "UNKNOWN".to_string(),
        };

        Self::parse_task_status(&content).unwrap_or_else(|| "UNKNOWN".to_string())
    }

    fn is_task_completed(task_path: &str) -> bool {
        Self::read_task_status(task_path) == "COMPLETED"
    }

    fn write_fusion_variant_task_file(
        project_path: &PathBuf,
        session_id: &str,
        variant_index: u8,
        variant_name: &str,
        task_description: &str,
    ) -> Result<PathBuf, String> {
        let tasks_dir = project_path.join(".hive-manager").join(session_id).join("tasks");
        std::fs::create_dir_all(&tasks_dir)
            .map_err(|e| format!("Failed to create tasks directory: {}", e))?;

        let filename = format!("fusion-variant-{}-task.md", variant_index);
        let file_path = tasks_dir.join(filename);
        let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");

        let content = format!(
r#"# Task Assignment - Fusion Variant {variant_index} ({variant_name})

## Status: ACTIVE

## Role Constraints

- **EXECUTOR**: You have full authority to implement and fix issues.
- **SCOPE**: Build this variant only.
- **GIT**: Commit your changes to your fusion branch.

## Instructions

{task_description}

## Completion Protocol

When task is complete, update this file:
1. Change Status to: COMPLETED
2. Add a summary under a new Result section

If blocked, change Status to: BLOCKED and describe the issue.

---
Last updated: {timestamp}
"#,
            variant_index = variant_index,
            variant_name = variant_name,
            task_description = task_description,
            timestamp = timestamp,
        );

        std::fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write fusion task file: {}", e))?;
        Ok(file_path)
    }

    fn build_fusion_worker_prompt(
        session_id: &str,
        variant_index: u8,
        variant_name: &str,
        branch: &str,
        worktree_path: &str,
        task_description: &str,
        cli: &str,
    ) -> String {
        let task_file = format!(".hive-manager/{}/tasks/fusion-variant-{}-task.md", session_id, variant_index);
        let polling_instructions = get_polling_instructions(cli, &task_file);

        format!(
r#"You are a Fusion worker implementing variant "{variant_name}".
Working directory: {worktree_path}
Branch: {branch}

## Your Task
{task_description}

## Rules
- Work ONLY within your worktree directory
- Commit all changes to your branch
- Do NOT interact with other variants
- When complete, update your task file status to COMPLETED

## Task Coordination
Read {task_file}. Begin work only when Status is ACTIVE.{polling_instructions}"#,
            variant_name = variant_name,
            worktree_path = worktree_path,
            branch = branch,
            task_description = task_description,
            task_file = task_file,
            polling_instructions = polling_instructions,
        )
    }

    fn build_fusion_judge_prompt(
        session_id: &str,
        variants: &[FusionVariantMetadata],
        decision_file: &str,
    ) -> String {
        let variant_list = variants
            .iter()
            .map(|v| format!("- {}: {}", v.name, v.worktree_path))
            .collect::<Vec<_>>()
            .join("\n");

        let diff_commands = variants
            .iter()
            .map(|v| format!("git diff fusion/{session_id}/base..{}", v.branch))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
r#"You are the Judge evaluating {variant_count} competing implementations.

## Variants
{variant_list}

## Evaluation Process
1. For each variant, run:
{diff_commands}
2. Review code quality, correctness, test coverage, and pattern adherence
3. Write comparison report to: {decision_file}

## Constraints
- You are read-only for code changes. Do NOT edit application code.
- Only produce the evaluation report and recommendation.

## Report Format
# Evaluation Report
## Variant Comparison
| Criterion | Variant A | Variant B | Notes |
## Recommendation
Winner: [variant name]
Rationale: [explanation]

## Learning Submission (REQUIRED)

After writing the evaluation report, submit learnings about what you observed.

### Step 1: Read existing learnings to avoid duplicates
```bash
curl -s "http://localhost:18800/api/sessions/{session_id}/learnings"
```

### Step 2: Submit learnings (one per insight)
```bash
curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/learnings" \
  -H "Content-Type: application/json" \
  -d '{{"content": "YOUR LEARNING HERE", "category": "CATEGORY", "source": "fusion-judge"}}'
```

### What to capture:
- **Which variant won and why** (category: "architecture")
- **Code quality patterns** observed — good and bad (category: "code-quality")
- **Architectural insights** from comparing approaches (category: "architecture")
- **Anti-patterns to avoid** (category: "anti-pattern")
"#,
            variant_count = variants.len(),
            variant_list = variant_list,
            diff_commands = diff_commands,
            decision_file = decision_file,
            session_id = session_id,
        )
    }

    /// Build the Master Planner's prompt for Fusion planning phase
    fn build_fusion_master_planner_prompt(
        session_id: &str,
        task_description: &str,
        variants: &[FusionVariantConfig],
    ) -> String {
        let variant_count = variants.len();
        let mut variant_table = String::new();
        for (i, v) in variants.iter().enumerate() {
            let index = i + 1;
            let name = if v.name.trim().is_empty() { format!("Variant {}", index) } else { v.name.trim().to_string() };
            variant_table.push_str(&format!("| {} | {} | {} |\n", index, name, v.cli));
        }

        // Determine phase 0 based on whether a task was provided
        let phase0 = if task_description.trim().is_empty() {
            String::from(r#"## PHASE 0: Gather Task (FIRST STEP)

**No task was provided.** You must first ask the user what they want to work on.

Ask the user: "What would you like the Fusion variants to compete on? You can:
- Provide a GitHub issue number (e.g., #42 or just 42)
- Describe a feature you want to implement
- Describe a bug you want to fix
- Describe code you want to refactor"

**If user provides a GitHub Issue number:**
1. Fetch issue details using: gh issue view <number> --json number,title,body,labels,state
2. Extract requirements and acceptance criteria from the issue body

**Once you have the task, proceed to PHASE 1.**

---

"#)
        } else if task_description.trim().starts_with('#') || task_description.trim().parse::<u32>().is_ok() {
            let issue_num = task_description.trim().trim_start_matches('#');
            format!(r#"## PHASE 0: Fetch GitHub Issue

The user wants to work on GitHub issue: **#{}**

**Fetch the issue details now:**
```bash
gh issue view {} --json number,title,body,labels,state
```

Extract from the response:
- Issue title and full description
- Acceptance criteria (look for checkboxes in the body)
- Labels (bug, feature, enhancement, etc.)

**Once you have the full context, proceed to PHASE 1.**

---

"#, issue_num, issue_num)
        } else {
            format!(r#"## PHASE 0: Task Provided

The user wants to work on:

**{}**

**Proceed directly to PHASE 1.**

---

"#, task_description)
        };

        format!(
r#"# Master Planner - Fusion Mode

You are the **Master Planner** for a Fusion session. Your job is to analyze the task and create a plan that documents how multiple independent variants will each tackle the same problem.

## Session Info

- **Session ID**: {session_id}
- **Mode**: Fusion (competing variants)
- **Plan Output**: `.hive-manager/{session_id}/plan.md`

## Variants

{variant_count} variants will compete, each implementing the SAME task independently:

| # | Name | CLI |
|---|------|-----|
{variant_table}

{phase0}

## PHASE 1: Your Mission

1. **Analyze the task** — understand what needs to be done, identify key decisions
2. **Document expected approaches** — for each variant, describe what strategies or patterns they might use. Since each variant works independently, they may naturally take different approaches.
3. **Identify evaluation criteria** — what should the Judge look for when comparing results? (correctness, code quality, performance, test coverage, etc.)
4. **Write the plan** to `.hive-manager/{session_id}/plan.md`

## Plan Format

Write the plan in this structure:

```markdown
# Fusion Plan

## Task Summary
[Concise description of what needs to be built/fixed]

## Key Decisions
- [Decision points where variants may diverge]

## Evaluation Criteria
- [ ] Correctness — does it work?
- [ ] Code quality — clean, readable, maintainable?
- [ ] Test coverage — are edge cases handled?
- [ ] Performance — efficient implementation?
- [ ] Pattern adherence — follows project conventions?

## Notes
[Any additional context for the variants and judge]
```

## IMPORTANT
- Write the plan to `.hive-manager/{session_id}/plan.md` and then STOP
- Do NOT implement anything — you are a planner, not a coder
- Keep the plan concise — variants will each receive the same task description
"#,
            session_id = session_id,
            variant_count = variant_count,
            variant_table = variant_table,
            phase0 = phase0,
        )
    }

    /// Build the Fusion Queen's prompt — monitors variants, spawns Judge when all complete
    fn build_fusion_queen_prompt(
        cli: &str,
        session_id: &str,
        variants: &[FusionVariantMetadata],
        task_description: &str,
    ) -> String {
        let variant_count = variants.len();
        let mut variant_info = String::new();
        let mut task_files = String::new();
        for v in variants {
            variant_info.push_str(&format!("| {} | {} | {} | {} |\n", v.index, v.name, v.branch, v.worktree_path));
            task_files.push_str(&format!("- Variant {} ({}): `{}`\n", v.index, v.name, v.task_file));
        }

        let hardening = if CliRegistry::needs_role_hardening(cli) {
            r#"
WARNING: CRITICAL ROLE CONSTRAINTS

You are the QUEEN - the top-level coordinator. You do NOT implement.

### You ARE allowed to:
- Read plan.md, task files, coordination.log
- Spawn Judge via HTTP API (use curl)
- Monitor variant progress
- Report status updates

### You are PROHIBITED from:
- Editing application source code
- Running implementation commands
- Implementing features directly
"#
        } else {
            ""
        };

        format!(
r#"# Queen Agent - Fusion Session

You are the **Queen** monitoring a Fusion session where {variant_count} variants compete to implement the same task.
{hardening}

## Session Info

- **Session ID**: {session_id}
- **Mode**: Fusion (competing variants)
- **Plan**: `.hive-manager/{session_id}/plan.md`
- **Tools Directory**: `.hive-manager/{session_id}/tools/`

## Task

{task_description}

## Variants

| # | Name | Branch | Worktree |
|---|------|--------|----------|
{variant_info}

## Task Files to Monitor

{task_files}

## Your Protocol

### Phase 1: Monitor Variants

Poll variant task files every 30 seconds to check for COMPLETED or FAILED status:

```bash
for file in {task_file_glob}; do echo "=== $file ==="; head -5 "$file"; done
```

A variant is complete when its task file contains `Status: COMPLETED`.

### Phase 2: Spawn Judge

When ALL {variant_count} variants have COMPLETED status, spawn the Judge:

```bash
curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
  -H "Content-Type: application/json" \
  -d '{{"cli": "{cli}", "role": "judge"}}'
```

### Phase 3: Monitor Judge

After spawning the Judge, monitor the evaluation directory:
- Decision file: `.hive-manager/{session_id}/evaluation/decision.md`
- When the decision file exists and is non-empty, report completion

## Status Reporting

Write status updates to `.hive-manager/{session_id}/coordination.log`:
```
[TIMESTAMP] QUEEN: Variant N (name) - COMPLETED/IN_PROGRESS/FAILED
[TIMESTAMP] QUEEN: All variants complete - spawning Judge
[TIMESTAMP] QUEEN: Judge evaluation complete
```

## Learning Tools

Read tool docs in `.hive-manager/{session_id}/tools/` for:
- `submit-learning.md` — Record observations
- `list-learnings.md` — View existing learnings
"#,
            variant_count = variant_count,
            hardening = hardening,
            session_id = session_id,
            task_description = task_description,
            variant_info = variant_info,
            task_files = task_files,
            task_file_glob = format!(".hive-manager/{}/tasks/fusion-variant-*-task.md", session_id),
            cli = cli,
        )
    }

    /// Build the Master Planner's prompt for initial planning phase
    fn build_master_planner_prompt(session_id: &str, user_prompt: &str, workers: &[AgentConfig]) -> String {
        // Build worker info section
        let mut worker_table = String::new();
        for (i, worker_config) in workers.iter().enumerate() {
            let index = i + 1;
            let role_label = worker_config.role.as_ref()
                .map(|r| r.label.clone())
                .unwrap_or_else(|| format!("Worker {}", index));
            let cli = &worker_config.cli;
            worker_table.push_str(&format!(
                "| Worker {} | {} | {} |\n",
                index, role_label, cli
            ));
        }

        let worker_count = workers.len();

        // Determine phase 0 based on whether a task was provided
        let phase0 = if user_prompt.trim().is_empty() {
            String::from(r#"## PHASE 0: Gather Task (FIRST STEP)

**No task was provided.** You must first ask the user what they want to work on.

Ask the user: "What would you like me to help you with today? You can:
- Provide a GitHub issue number (e.g., #42 or just 42)
- Describe a feature you want to implement
- Describe a bug you want to fix
- Describe code you want to refactor"

**If user provides a GitHub Issue number:**
1. Fetch issue details using: gh issue view <number> --json number,title,body,labels,state
2. Extract requirements and acceptance criteria from the issue body

**Once you have the task, proceed to PHASE 1.**

---

"#)
        } else if user_prompt.trim().starts_with('#') || user_prompt.trim().parse::<u32>().is_ok() {
            // Looks like a GitHub issue number
            let issue_num = user_prompt.trim().trim_start_matches('#');
            format!(r#"## PHASE 0: Fetch GitHub Issue

The user wants to work on GitHub issue: **#{}**

**Fetch the issue details now:**
```bash
gh issue view {} --json number,title,body,labels,state
```

Extract from the response:
- Issue title and full description
- Acceptance criteria (look for checkboxes in the body)
- Labels (bug, feature, enhancement, etc.)

**Once you have the full context, proceed to PHASE 1.**

---

"#, issue_num, issue_num)
        } else {
            format!(r#"## PHASE 0: Task Provided

The user wants to work on:

**{}**

**Proceed directly to PHASE 1.**

---

"#, user_prompt)
        };

        format!(
r#"# Master Planner - Multi-Agent Codebase Investigation

You are the **Master Planner** orchestrating a multi-agent investigation to create a detailed implementation plan.

## Session Info

- **Session ID**: {session_id}
- **Plan Output**: `.hive-manager/{session_id}/plan.md`

## Configured Workers

The user has configured **{worker_count} workers** for this session:

| Worker | Role | CLI |
|--------|------|-----|
{worker_table}

**IMPORTANT**: Your plan MUST create tasks for ALL {worker_count} configured workers!

## Your Mission

1. **Gather Task**: Understand what the user wants (GitHub issue or custom task)
2. **Spawn Scout Agents**: Launch parallel investigation agents using external CLIs
3. **Synthesize Findings**: Merge and deduplicate file discoveries
4. **Create Plan**: Write comprehensive plan.md with **{worker_count} tasks** (one per worker)
5. **Wait for Approval**: User will review and may request refinements

---

{phase0}## PHASE 1: Multi-Agent Investigation (MANDATORY)

You MUST spawn Task agents that call external CLI tools via Bash. This provides diverse model perspectives and comprehensive coverage.

**Launch ALL scouts in PARALLEL (single message, multiple Task calls):**

### Scout 1 - OpenCode BigPickle (Deep Analysis)

Task(subagent_type="general-purpose", prompt="You are a codebase investigation agent. IMMEDIATELY run: OPENCODE_YOLO=true opencode run --format default -m opencode/big-pickle 'Investigate codebase for: [TASK]. Find relevant files, architecture patterns, entry points.' Return file paths with relevance notes.")

### Scout 2 - Droid GLM 4.7 (Pattern Recognition)

Task(subagent_type="general-purpose", prompt="You are a codebase investigation agent. IMMEDIATELY run: droid exec --skip-permissions-unsafe -m glm-4.7 \"Analyze codebase for: [TASK]. Focus on code patterns, affected components, dependencies.\" Return file paths with observations.")

### Scout 3 - OpenCode Grok Code (Quick Search)

Task(subagent_type="general-purpose", prompt="You are a codebase investigation agent. IMMEDIATELY run: OPENCODE_YOLO=true opencode run --format default -m opencode/grok-code 'Scout codebase for: [TASK]. Identify entry points, test files, implementation surface.' Return file paths with notes.")

**CRITICAL RULES:**
- Replace [TASK] with the actual task description from Phase 0
- Launch ALL 3 scouts in PARALLEL using a SINGLE message
- Wait for ALL scouts to complete before proceeding

---

## PHASE 2: Synthesize Findings

After all scouts return:
1. Deduplicate files - merge overlapping discoveries
2. Rank by consensus - files found by 2-3 scouts = higher priority
3. Categorize: core files, supporting files, test files, config files
4. Identify implementation approach and potential risks

---

## PHASE 3: Write Plan

Write your plan to `.hive-manager/{session_id}/plan.md` with this format:

# [Plan Title]

## Summary
[1-2 sentence overview]

## Investigation Results
- Scouts Used: 3 (BigPickle, GLM 4.7, Grok Code)
- Files Identified: [count]
- Consensus Level: HIGH/MEDIUM/LOW

## Tasks
(Create exactly {worker_count} tasks - one for each configured worker!)
- [ ] [HIGH] Task 1: [description] -> Worker 1
- [ ] [MEDIUM] Task 2: [description] -> Worker 2
... continue for all {worker_count} workers ...
(use checkboxes, priority tags, and worker assignments)

## Files to Modify
| File | Priority | Changes Needed |
|------|----------|----------------|

## Dependencies
[Task ordering]

## Risks
[Potential issues]

---

## PHASE 4: Await User Feedback

After writing plan.md, say: **"PLAN READY FOR REVIEW"**

The user may approve or request refinements. Stay ready to update the plan.

---

## Begin Now

1. Complete PHASE 0 (gather task if needed)
2. Launch ALL 3 scout agents in PARALLEL
3. Synthesize findings
4. Write plan to `.hive-manager/{session_id}/plan.md`
5. Say "PLAN READY FOR REVIEW""#,
            session_id = session_id,
            phase0 = phase0,
            worker_count = worker_count,
            worker_table = worker_table.trim_end()
        )
    }

    /// Build the Master Planner's prompt for Swarm mode with planner and worker information
    fn build_swarm_master_planner_prompt(session_id: &str, user_prompt: &str, planner_count: u8, workers_per_planner: &[AgentConfig]) -> String {
        let workers_per = workers_per_planner.len();
        let total_workers = planner_count as usize * workers_per;

        // Build planner table
        let mut planner_table = String::new();
        let domains = ["backend", "frontend", "testing", "infrastructure", "documentation", "security", "performance", "integration"];

        for i in 0..planner_count {
            let index = i + 1;
            let domain = domains.get(i as usize).unwrap_or(&"general");
            planner_table.push_str(&format!(
                "| Planner {} | {} | {} workers |\n",
                index, domain, workers_per
            ));
        }

        // Build worker info
        let mut worker_info = String::new();
        for (i, worker_config) in workers_per_planner.iter().enumerate() {
            let index = i + 1;
            let role_label = worker_config.role.as_ref()
                .map(|r| r.label.clone())
                .unwrap_or_else(|| format!("Worker {}", index));
            worker_info.push_str(&format!(
                "| {} | {} | {} |\n",
                index, role_label, worker_config.cli
            ));
        }

        // Determine phase 0 based on whether a task was provided
        let phase0 = if user_prompt.trim().is_empty() {
            String::from(r#"## PHASE 0: Gather Task (FIRST STEP)

**No task was provided.** You must first ask the user what they want to work on.

Ask the user: "What would you like me to help you with today? You can:
- Provide a GitHub issue number (e.g., #42 or just 42)
- Describe a feature you want to implement
- Describe a bug you want to fix
- Describe code you want to refactor"

**If user provides a GitHub Issue number:**
1. Fetch issue details using: gh issue view <number> --json number,title,body,labels,state
2. Extract requirements and acceptance criteria from the issue body

**Once you have the task, proceed to PHASE 1.**

---

"#)
        } else if user_prompt.trim().starts_with('#') || user_prompt.trim().parse::<u32>().is_ok() {
            let issue_num = user_prompt.trim().trim_start_matches('#');
            format!(r#"## PHASE 0: Fetch GitHub Issue

The user wants to work on GitHub issue: **#{}**

**Fetch the issue details now:**
```bash
gh issue view {} --json number,title,body,labels,state
```

Extract from the response:
- Issue title and full description
- Acceptance criteria (look for checkboxes in the body)
- Labels (bug, feature, enhancement, etc.)

**Once you have the full context, proceed to PHASE 1.**

---

"#, issue_num, issue_num)
        } else {
            format!(r#"## PHASE 0: Task Provided

The user wants to work on:

**{}**

**Proceed directly to PHASE 1.**

---

"#, user_prompt)
        };

        format!(
r#"# Master Planner - Swarm Multi-Agent Investigation

You are the **Master Planner** orchestrating a Swarm investigation to create a detailed implementation plan.

## Session Info

- **Session ID**: {session_id}
- **Mode**: Swarm (hierarchical)
- **Plan Output**: `.hive-manager/{session_id}/plan.md`

## Swarm Configuration

- **Planners**: {planner_count}
- **Workers per Planner**: {workers_per}
- **Total Workers**: {total_workers}

### Planners (Domains)

| Planner | Domain | Workers |
|---------|--------|---------|
{planner_table}

### Worker Roles (per Planner)

| # | Role | CLI |
|---|------|-----|
{worker_info}

**IMPORTANT**: Your plan MUST create **{planner_count} domain-level tasks** - one for each Planner!
Each Planner will break their domain task into {workers_per} worker subtasks.

## Your Mission

1. **Gather Task**: Understand what the user wants (GitHub issue or custom task)
2. **Spawn Scout Agents**: Launch parallel investigation agents using external CLIs
3. **Synthesize Findings**: Merge and deduplicate file discoveries
4. **Create Plan**: Write comprehensive plan.md with **{planner_count} domain tasks** (one per Planner)
5. **Wait for Approval**: User will review and may request refinements

---

{phase0}## PHASE 1: Parallel Investigation

Spawn 3 scout agents to investigate the codebase in parallel:

```bash
# Scout 1 - Code Structure (Gemini)
gemini -y -i "Analyze the codebase structure for: [TASK]. List relevant files by priority."

# Scout 2 - Implementation Patterns (Claude Subagent via Task tool)
# Use Claude's Task tool with Explore agent

# Scout 3 - Related Code (Codex if available, or another Gemini)
codex --dangerously-bypass-approvals-and-sandbox -m gpt-5.3-codex "Find code related to: [TASK]"
```

---

## PHASE 2: Synthesize & Partition

Merge findings from all scouts:
1. Deduplicate file lists
2. **Partition into {planner_count} domains** - one per Planner
3. Prioritize by impact (HIGH/MEDIUM/LOW)

---

## PHASE 3: Write Plan

Write to `.hive-manager/{session_id}/plan.md`:

```markdown
# Implementation Plan

## Summary
[Brief description of the task and approach]

## Investigation Results
- Scouts Used: 3
- Files Identified: [count]
- Consensus Level: [HIGH/MEDIUM/LOW]

## Domain Tasks (for Planners)

### Domain 1: [Domain Name]
- [ ] [PRIORITY] Task description -> Planner 1
- Files: [list of files in this domain]
- Workers: {workers_per} available

### Domain 2: [Domain Name]
- [ ] [PRIORITY] Task description -> Planner 2
- Files: [list of files in this domain]
- Workers: {workers_per} available

[... repeat for all {planner_count} planners ...]

## Files to Modify
| File | Domain | Priority | Changes Needed |
|------|--------|----------|----------------|

## Cross-Domain Dependencies
[Note any dependencies between domains]

## Risks
[List potential risks and mitigation strategies]
```

---

## Quick Reference

1. Gather task (ask user or fetch GitHub issue)
2. Launch ALL 3 scout agents in PARALLEL
3. Synthesize findings and partition into {planner_count} domains
4. Write plan to `.hive-manager/{session_id}/plan.md`
5. Say "PLAN READY FOR REVIEW""#,
            session_id = session_id,
            phase0 = phase0,
            planner_count = planner_count,
            workers_per = workers_per,
            total_workers = total_workers,
            planner_table = planner_table.trim_end(),
            worker_info = worker_info.trim_end()
        )
    }

    /// Build a minimal smoke test prompt that creates a simple plan without real investigation
    fn build_smoke_test_prompt(session_id: &str, workers: &[AgentConfig]) -> String {
        // Build worker table and task list based on configured workers
        let mut worker_table = String::new();
        let mut task_list = String::new();
        let mut dependencies = String::new();

        for (i, worker_config) in workers.iter().enumerate() {
            let index = i + 1;
            let role_label = worker_config.role.as_ref()
                .map(|r| r.label.clone())
                .unwrap_or_else(|| format!("Worker {}", index));
            let cli = &worker_config.cli;

            worker_table.push_str(&format!(
                "| Worker {} | {} | {} |\n",
                index, role_label, cli
            ));

            let priority = if index == 1 { "HIGH" } else if index == 2 { "MEDIUM" } else { "LOW" };
            task_list.push_str(&format!(
                "- [ ] [{}] Smoke test task {}: Verify {} worker functionality -> Worker {}\n",
                priority, index, role_label, index
            ));

            if index > 1 {
                dependencies.push_str(&format!("- Task {} depends on Task {} completing.\n", index, index - 1));
            }
        }

        if dependencies.is_empty() {
            dependencies = "None - single worker smoke test.".to_string();
        }

        format!(
r#"# Smoke Test - Quick Flow Validation

You are running a **SMOKE TEST** to validate the Hive Manager flow.

## Configured Workers

The user has configured **{worker_count} workers** for this session:

| Worker | Role | CLI |
|--------|------|-----|
{worker_table}

## Your Task

Create a minimal test plan immediately. Do NOT spawn any investigation agents.
Do NOT analyze the codebase. Just create a simple plan to test the flow.

**IMPORTANT**: Create exactly **{worker_count} tasks** - one for each configured worker!

## Write This Plan Now

Write the following to `.hive-manager/{session_id}/plan.md`:

```markdown
# Smoke Test Plan

## Summary
This is a smoke test to validate the planning flow works correctly.
Testing {worker_count} workers as configured by the user.

## Investigation Results
- Scouts Used: 0 (smoke test - skipped)
- Files Identified: 0
- Consensus Level: N/A

## Tasks
{task_list}
## Files to Modify
| File | Priority | Changes Needed |
|------|----------|----------------|
| (smoke test - no real files) | N/A | N/A |

## Dependencies
{dependencies}
## Risks
None - this is a smoke test.

## Notes
Smoke test completed successfully. The planning phase flow is working.
Testing all {worker_count} configured workers.
```

After writing the plan, say: **"PLAN READY FOR REVIEW"**

This tests that:
1. Master Planner can write to the plan file
2. User can see and approve the plan
3. Flow continues to spawn Queen and all {worker_count} Workers"#,
            session_id = session_id,
            worker_count = workers.len(),
            worker_table = worker_table.trim_end(),
            task_list = task_list.trim_end(),
            dependencies = dependencies.trim_end()
        )
    }

    /// Build a smoke test prompt for Swarm mode that accounts for planners AND workers
    fn build_swarm_smoke_test_prompt(session_id: &str, planner_count: u8, workers_per_planner: &[AgentConfig]) -> String {
        let workers_per = workers_per_planner.len();
        let total_workers = planner_count as usize * workers_per;

        // Build planner table
        let mut planner_table = String::new();
        let mut domain_tasks = String::new();

        let domains = ["backend", "frontend", "testing", "infrastructure", "documentation", "security", "performance", "integration"];

        for i in 0..planner_count {
            let index = i + 1;
            let domain = domains.get(i as usize).unwrap_or(&"general");
            planner_table.push_str(&format!(
                "| Planner {} | {} | {} workers |\n",
                index, domain, workers_per
            ));

            let priority = if index == 1 { "HIGH" } else if index == 2 { "MEDIUM" } else { "LOW" };
            domain_tasks.push_str(&format!(
                "- [ ] [{}] Domain {}: {} smoke test tasks (will be broken into {} worker tasks)\n",
                priority, index, domain, workers_per
            ));
        }

        // Build worker breakdown per planner
        let mut worker_breakdown = String::new();
        for pi in 0..planner_count {
            let planner_index = pi + 1;
            let domain = domains.get(pi as usize).unwrap_or(&"general");
            worker_breakdown.push_str(&format!("\n### Planner {} - {} Domain\n\n", planner_index, domain));

            for (wi, worker_config) in workers_per_planner.iter().enumerate() {
                let worker_index = wi + 1;
                let role_label = worker_config.role.as_ref()
                    .map(|r| r.label.clone())
                    .unwrap_or_else(|| format!("Worker {}", worker_index));
                worker_breakdown.push_str(&format!(
                    "- Worker {}.{}: {} ({})\n",
                    planner_index, worker_index, role_label, worker_config.cli
                ));
            }
        }

        format!(
r#"# Swarm Smoke Test - Quick Flow Validation

You are running a **SMOKE TEST** to validate the Swarm Manager flow.

## Swarm Configuration

- **Planners**: {planner_count}
- **Workers per Planner**: {workers_per}
- **Total Workers**: {total_workers}

### Planners

| Planner | Domain | Workers |
|---------|--------|---------|
{planner_table}

### Worker Breakdown
{worker_breakdown}

## Your Task

Create a minimal test plan immediately. Do NOT spawn any investigation agents.
Do NOT analyze the codebase. Just create a simple plan to test the Swarm flow.

**IMPORTANT**: Create exactly **{planner_count} domain tasks** - one for each configured planner!
Each planner will then break their domain task into {workers_per} worker tasks.

## Write This Plan Now

Write the following to `.hive-manager/{session_id}/plan.md`:

```markdown
# Swarm Smoke Test Plan

## Summary
This is a smoke test to validate the Swarm planning flow works correctly.
Testing {planner_count} planners, each with {workers_per} workers ({total_workers} total workers).

## Investigation Results
- Scouts Used: 0 (smoke test - skipped)
- Files Identified: 0
- Consensus Level: N/A

## Domain Tasks (for Planners)
{domain_tasks}
## Planner → Worker Breakdown

Each Planner spawns their workers sequentially and assigns subtasks:
{worker_breakdown}

## Files to Modify
| File | Priority | Changes Needed |
|------|----------|----------------|
| (smoke test - no real files) | N/A | N/A |

## Dependencies
- Planners work sequentially (Planner 1 completes, commit, then Planner 2)
- Workers within each Planner work sequentially
- Queen commits between each Planner completion

## Risks
None - this is a smoke test.

## Notes
Swarm smoke test completed successfully. The planning phase flow is working.
Testing {planner_count} planners with {workers_per} workers each = {total_workers} total workers.
```

After writing the plan, say: **"PLAN READY FOR REVIEW"**

This tests that:
1. Master Planner can write to the plan file
2. User can see and approve the plan
3. Flow continues to spawn Queen who spawns {planner_count} Planners sequentially
4. Each Planner spawns {workers_per} Workers sequentially
5. Queen commits between each Planner completion"#,
            session_id = session_id,
            planner_count = planner_count,
            workers_per = workers_per,
            total_workers = total_workers,
            planner_table = planner_table.trim_end(),
            domain_tasks = domain_tasks.trim_end(),
            worker_breakdown = worker_breakdown.trim_end()
        )
    }

    /// Build the Queen's master prompt with worker information
    fn build_queen_master_prompt(cli: &str, session_id: &str, workers: &[AgentConfig], user_prompt: Option<&str>, has_plan: bool) -> String {
        let mut worker_list = String::new();
        for (i, worker_config) in workers.iter().enumerate() {
            let index = i + 1;
            let worker_id = format!("{}-worker-{}", session_id, index);
            let role_label = worker_config.role.as_ref()
                .map(|r| format!("Worker {} ({})", index, r.label))
                .unwrap_or_else(|| format!("Worker {}", index));
            worker_list.push_str(&format!("| {} | {} | {} |\n", worker_id, role_label, worker_config.cli));
        }

        let plan_section = if has_plan {
            format!(
r#"## Implementation Plan

**IMPORTANT**: A plan has been generated for this session. Read it first:
```
.hive-manager/{}/plan.md
```

Follow the plan's task breakdown when assigning work to workers."#,
                session_id
            )
        } else {
            String::new()
        };

        let hardening = if CliRegistry::needs_role_hardening(cli) {
            r#"
WARNING: CRITICAL ROLE CONSTRAINTS

You are the QUEEN - the top-level coordinator. You do NOT implement.

### You ARE allowed to:
- Read plan.md, coordination.log, worker status files
- Write/Edit ONLY: Planner task files, coordination.log
- Run git commands: commit, push, branch, PR creation
- Spawn investigation agents for planning (not implementation)

### You are PROHIBITED from:
- Editing application source code (*.rs, *.ts, *.svelte, etc.)
- Running implementation commands (cargo build, npm run, tests)
- Fixing bugs or implementing features directly
- Bypassing Planners to assign tasks directly to Workers

If you find yourself about to edit code, STOP. Write a task file for a Planner instead.
"#
        } else {
            ""
        };

        let branch_protocol = r#"
## Branch Protocol (MANDATORY)

⚠️ BEFORE assigning ANY tasks to workers:

1. **Check if this is a smoke test** - If yes, skip branch creation
2. **If NOT a smoke test**:
   - FIRST create a new feature branch: `git checkout -b feat/<descriptive-name>`
   - Push the branch: `git push -u origin <branch-name>`
   - THEN assign tasks to workers

### Why This Matters
- Workers will commit to this branch
- Prevents accidental commits to main
- Ensures clean PR workflow

### Example
```bash
# Queen does this FIRST
git checkout -b feat/add-authentication
git push -u origin feat/add-authentication

# THEN assigns tasks to workers
```

Do NOT assign worker tasks until the branch exists!
"#;

        format!(
            r#"# Queen Agent - Hive Manager Session

You are the **Queen** orchestrating a multi-agent Hive session. You have full Claude Code capabilities plus coordination tools.
{hardening}
{branch_protocol}
## Session Info
- **Session ID**: {session_id}
- **Prompts Directory**: `.hive-manager/{session_id}/prompts/`
- **Tasks Directory**: `.hive-manager/{session_id}/tasks/`
- **Tools Directory**: `.hive-manager/{session_id}/tools/`
- **Conversation Files**: `.hive-manager/{session_id}/conversations/queen.md`, `.hive-manager/{session_id}/conversations/shared.md`, `.hive-manager/{session_id}/conversations/worker-N.md`

{plan_section}

## Your Workers

| ID | Role | CLI |
|----|------|-----|
{worker_list}

## Your Tools

### Claude Code Tools (Native)
You have full access to all Claude Code tools:
- **Read/Write/Edit** - File operations
- **Bash** - Run shell commands, git operations
- **Glob/Grep** - Search files and content
- **Task** - Spawn subagents for complex investigation (NOT for spawning workers)
- **WebFetch/WebSearch** - Access web resources

### Claude Code Commands
You can use any /commands in `~/.claude/commands/`

### Hive-Specific Tools

Tool documentation is in `.hive-manager/{session_id}/tools/`. Read these files for detailed usage:

| Tool | File | Purpose |
|------|------|---------|
| Spawn Worker | `spawn-worker.md` | Spawn new workers via HTTP API (visible terminal windows) |
| List Workers | `list-workers.md` | Get list of all workers and their status |
| Submit Learning | `submit-learning.md` | Record a learning via HTTP API |
| List Learnings | `list-learnings.md` | Get all learnings for this session |
| Delete Learning | `delete-learning.md` | Remove a learning by ID |

**Quick Reference - Spawn Worker:**
```bash
curl -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
  -H "Content-Type: application/json" \
  -d '{{"role_type": "backend", "cli": "{cli}"}}'
```

### Task Assignment
To assign tasks to existing workers, update their task files:

```
Edit: .hive-manager/{session_id}/tasks/worker-N-task.md
Change Status: STANDBY -> ACTIVE
Add task instructions
```

Workers poll their task files and will start when they see ACTIVE status.

## Inter-Agent Communication

Use these exact conversation and heartbeat endpoints:

```bash
# Check Queen inbox
curl -s "http://localhost:18800/api/sessions/{session_id}/conversations/queen?since=<last_check_ts>"

# Message a worker
curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/conversations/worker-N/append" \
  -H "Content-Type: application/json" \
  -d '{{"from":"queen","content":"Your message"}}'

# Broadcast to all agents
curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/conversations/shared/append" \
  -H "Content-Type: application/json" \
  -d '{{"from":"queen","content":"Announcement"}}'

# Heartbeat (every 60-90s)
curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/heartbeat" \
  -H "Content-Type: application/json" \
  -d '{{"agent_id":"queen","status":"working","summary":"Monitoring workers"}}'

# Inspect active sessions and heartbeat state
curl -s "http://localhost:18800/api/sessions/active"
```

Check your inbox between subtasks. Read `shared.md` for broadcasts before assigning new work.

## Learning Curation Protocol

Workers record learnings during task completion. Your curation responsibilities:

1. **Review learnings periodically**:
   ```bash
   curl "http://localhost:18800/api/sessions/{session_id}/learnings"
   ```

2. **Review current project DNA**:
   ```bash
   curl "http://localhost:18800/api/sessions/{session_id}/project-dna"
   ```

3. **Curate useful learnings** into the session-scoped `project-dna.md` via the API:
   - Group by theme/topic
   - Remove duplicates
   - Improve clarity where needed
   - Capture architectural decisions and project conventions

### Session-Scoped Lessons Structure
```
.hive-manager/{session_id}/lessons/
├── learnings.jsonl      # Raw learnings for this session (append-only)
└── project-dna.md       # Curated patterns, conventions, insights
```

### Curation Process
1. Review raw learnings via `GET /api/sessions/{session_id}/learnings`
2. Review current project DNA via `GET /api/sessions/{session_id}/project-dna`
3. Synthesize insights into `project-dna.md` sections:
   - **Patterns That Work** - Successful approaches
   - **Patterns That Failed** - What to avoid
   - **Code Conventions** - Project-specific standards
   - **Architecture Notes** - Key design decisions
4. Delete outdated or duplicate learnings via `DELETE /api/sessions/{{session_id}}/learnings/{{learning_id}}`

### When to Curate
- After each major task phase completes
- Before creating a PR
- When learnings count exceeds 10

## Coordination Protocol

1. **Read the plan** - Check `.hive-manager/{session_id}/plan.md` if it exists
2. **Spawn workers** - Use the spawn-worker tool to create workers as needed
3. **Assign tasks** - Update worker task files with specific assignments
4. **Monitor progress** - Watch for workers to mark tasks COMPLETED
5. **Spawn next worker** - When a task completes, spawn the next worker if needed
6. **Review & integrate** - Review worker output and coordinate integration
7. **Commit & push** - You handle final commits (workers don't push)

After your orchestration objective is complete, transition to `idle` heartbeat status and continue checking your conversation file on heartbeat cadence.

## Your Task

{task}"#,
            hardening = hardening,
            branch_protocol = branch_protocol,
            session_id = session_id,
            cli = cli,
            plan_section = plan_section,
            worker_list = worker_list,
            task = user_prompt.unwrap_or("Read the plan and begin coordinating workers.")
        )
    }

    /// Build a worker's role prompt
    fn build_worker_prompt(index: u8, config: &AgentConfig, queen_id: &str, session_id: &str) -> String {
        let role_name = config.role.as_ref()
            .map(|r| r.label.clone())
            .unwrap_or_else(|| format!("Worker {}", index));

        let role_description = config.role.as_ref()
            .map(|r| match r.role_type.to_lowercase().as_str() {
                "backend" => "Server-side logic, APIs, databases, and backend infrastructure.",
                "frontend" => "UI components, state management, styling, and user experience.",
                "coherence" => "Code consistency, API contracts, and cross-component integration.",
                "simplify" => "Code simplification, refactoring, and reducing complexity.",
                "reviewer" => "Deep code review: edge cases, security, performance, architecture, breaking changes.",
                "reviewer-quick" => "Quick code review: obvious bugs, code style, simple improvements.",
                "resolver" => "Address all reviewer findings: fix HIGH/MEDIUM issues, document skipped items with rationale.",
                "tester" => "Run test suite, fix failures, document difficulties that couldn't be resolved.",
                "code-quality" => "Resolve PR comments from external reviewers, ensure code meets quality standards.",
                _ => "General development tasks as assigned.",
            })
            .unwrap_or("General development tasks as assigned.");

        let task_file = format!(".hive-manager/{}/tasks/worker-{}-task.md", session_id, index);
        let polling_instructions = get_polling_instructions(&config.cli, &task_file);

        format!(
r#"# Worker {index} ({role_name}) - Hive Session

You are a **Worker** in a multi-agent Hive session, coordinated by the Queen.

## Your Role: EXECUTOR

You have full implementation authority within your specialization.

## Your Specialization

{role_description}

## Your Tools

You have full access to Claude Code tools:
- **Read/Write/Edit** - File operations
- **Bash** - Run shell commands
- **Glob/Grep** - Search files and content
- **Task** - Spawn subagents if needed

## Task File (File-Based Coordination)

Your task assignments are in: `{task_file}`

## Conversation Files (Session-Scoped)

- Your inbox file: `.hive-manager/{session_id}/conversations/worker-{index}.md`
- Queen channel: `.hive-manager/{session_id}/conversations/queen.md`
- Shared broadcasts: `.hive-manager/{session_id}/conversations/shared.md`

**Workflow:**
1. Read your task file to check your current status
2. If Status is `STANDBY` - wait and periodically re-check the file
3. If Status is `ACTIVE` - execute the task described in the file
4. When done, update the task file: change Status to `COMPLETED` and add your results
5. If blocked, change Status to `BLOCKED` and describe the issue

## Important Rules

1. **Stay in your lane** - Focus on your specialization ({role_name})
2. **Don't push to git** - Only the Queen commits and pushes
3. **Update your task file** - Always update status when done or blocked
4. **Ask for clarification** - If task is unclear, note it in the task file

## Coordinator

- **Queen**: {queen_id}

## Inter-Agent Communication

Use these exact conversation and heartbeat endpoints:

```bash
# Check your inbox
curl -s "http://localhost:18800/api/sessions/{session_id}/conversations/worker-{index}?since=<last_check_ts>"

# Send update or question to Queen
curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/conversations/queen/append" \
  -H "Content-Type: application/json" \
  -d '{{"from":"worker-{index}","content":"Status update or blocker"}}'

# Read shared broadcast stream
curl -s "http://localhost:18800/api/sessions/{session_id}/conversations/shared?since=<last_check_ts>"

# Heartbeat (every 60-90s)
curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/heartbeat" \
  -H "Content-Type: application/json" \
  -d '{{"agent_id":"worker-{index}","status":"working","summary":"Current task focus"}}'

# Inspect active sessions and heartbeat state
curl -s "http://localhost:18800/api/sessions/active"
```

Check your conversation file between subtasks. Report progress to `queen.md` after milestones. Read `shared.md` for broadcasts.

## Learnings Protocol (MANDATORY)

Before marking your task COMPLETED, submit what you learned:

```bash
curl -X POST "http://localhost:18800/api/sessions/{session_id}/learnings" \
  -H "Content-Type: application/json" \
  -d '{{
    "session": "{session_id}",
    "task": "Brief task description",
    "outcome": "success|partial|failed",
    "keywords": ["keyword1", "keyword2"],
    "insight": "What you learned - be specific and actionable",
    "files_touched": ["path/to/file.rs"]
  }}'
```

Even if you learned nothing notable, submit with insight "No significant learnings for this task."

## Project Context

Review `.ai-docs/project-dna.md` for patterns and conventions learned from previous sessions.

## Task Coordination
Read {task_file}. Begin work only when Status is ACTIVE.
Use the interactive interface to monitor your task file.

After completing your task, transition to IDLE state. Continue checking your conversation file on heartbeat cadence.{polling_instructions}"#,
            index = index,
            role_name = role_name,
            role_description = role_description,
            queen_id = queen_id,
            session_id = session_id,
            task_file = task_file,
            polling_instructions = polling_instructions
        )
    }

    /// Build a planner's prompt with HTTP API for spawning workers sequentially
    fn build_planner_prompt_with_http(cli: &str, index: u8, config: &PlannerConfig, queen_id: &str, session_id: &str) -> String {
        let worker_count = config.workers.len();

        // Build worker info section
        let mut worker_info = String::new();
        for (i, worker_config) in config.workers.iter().enumerate() {
            let worker_index = i + 1;
            let role_label = worker_config.role.as_ref()
                .map(|r| r.label.clone())
                .unwrap_or_else(|| format!("Worker {}", worker_index));
            let cli_name = &worker_config.cli;
            worker_info.push_str(&format!("| {} | {} | {} |\n", worker_index, role_label, cli_name));
        }

        let hardening = if CliRegistry::needs_role_hardening(cli) {
            r#"
WARNING: CRITICAL ROLE CONSTRAINTS

You are a PLANNER - you coordinate Workers in your domain. You do NOT implement.

### You ARE allowed to:
- Read any file in your domain for context
- Spawn workers via HTTP API (use curl)
- Write/Edit ONLY: Worker task files in your domain
- Read worker task files to monitor COMPLETED/BLOCKED status
- Report domain completion to Queen

### You are PROHIBITED from:
- Editing application source code directly
- Running implementation commands
- Completing worker tasks yourself
- "Helping" by doing a worker's job
- Using Task tool to spawn subagents (use HTTP API instead for visible windows)

If a worker is blocked, reassign or escalate to Queen. Do NOT fix it yourself.
"#
        } else {
            ""
        };

        format!(
            r#"# Planner {index} - {domain} Domain

You are a **Planner** in a multi-agent Swarm session, managing the {domain} domain.
{hardening}
## Session Info

- **Session ID**: {session_id}
- **Queen**: {queen_id}
- **Your ID**: {session_id}-planner-{index}
- **Tools Directory**: `.hive-manager/{session_id}/tools/`

## Your Domain

{domain}

## Workers to Spawn

You will spawn {worker_count} workers SEQUENTIALLY. Each worker runs in its own visible terminal window.

| # | Role | CLI |
|---|------|-----|
{worker_info}

## HTTP API for Spawning Workers

Read `.hive-manager/{session_id}/tools/spawn-worker.md` for detailed documentation.

**Quick Reference:**
```bash
# Spawn a worker
curl -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
  -H "Content-Type: application/json" \
  -d '{{"role_type": "ROLE", "cli": "{cli}", "initial_task": "TASK", "parent_id": "{session_id}-planner-{index}"}}'
```

## SEQUENTIAL SPAWNING PROTOCOL (CRITICAL)

You MUST spawn workers ONE AT A TIME and wait for completion:

1. **Spawn Worker 1** via HTTP API with initial task
2. **Wait for Worker 1** to signal `[COMPLETED]` in their task file
3. **Spawn Worker 2** via HTTP API with initial task
4. **Wait for Worker 2** to signal `[COMPLETED]` in their task file
5. Continue until all {worker_count} workers are done
6. Signal `[DOMAIN_COMPLETE]` to Queen

### Monitoring Worker Completion

Check worker task files for status:
```bash
# Read worker task file to check status
cat .hive-manager/{session_id}/tasks/worker-N-task.md | grep "Status:"
```

Look for:
- `Status: COMPLETED` - Worker finished successfully
- `Status: BLOCKED` - Worker needs help (escalate to you or Queen)

## Protocol Summary

1. Receive domain task from Queen
2. Break down into worker subtasks
3. Spawn Worker 1 with task → wait for completion
4. Spawn Worker 2 with task → wait for completion
5. ... repeat for all workers
6. Verify integration works
7. Report `[DOMAIN_COMPLETE]` to Queen

## Your Current Task

Awaiting task assignment from the Queen."#,
            index = index,
            domain = config.domain,
            session_id = session_id,
            cli = cli,
            hardening = hardening,
            worker_info = worker_info,
            worker_count = worker_count,
            queen_id = queen_id
        )
    }

    /// Build the Queen's master prompt for Swarm mode with sequential planner spawning
    fn build_swarm_queen_prompt(cli: &str, session_id: &str, planners: &[PlannerConfig], user_prompt: Option<&str>) -> String {
        let planner_count = planners.len();

        // Build planner info section (what Queen will spawn)
        let mut planner_info = String::new();
        for (i, planner_config) in planners.iter().enumerate() {
            let index = i + 1;
            let worker_count = planner_config.workers.len();
            planner_info.push_str(&format!("| {} | {} | {} workers |\n",
                index, planner_config.domain, worker_count));
        }

        let hardening = if CliRegistry::needs_role_hardening(cli) {
            r#"
WARNING: CRITICAL ROLE CONSTRAINTS

You are the QUEEN - the top-level coordinator. You do NOT implement.

### You ARE allowed to:
- Read plan.md, coordination.log, planner status files
- Spawn planners via HTTP API (use curl)
- Run git commands: commit, push, branch, PR creation
- Coordinate cross-domain integration

### You are PROHIBITED from:
- Editing application source code (*.rs, *.ts, *.svelte, etc.)
- Running implementation commands (cargo build, npm run, tests)
- Fixing bugs or implementing features directly
- Spawning workers directly (Planners spawn workers)
- Using Task tool to spawn subagents (use HTTP API for visible terminal windows)

If you find yourself about to edit code, STOP. Assign work to a Planner instead.
"#
        } else {
            ""
        };

        format!(
r#"# Queen Agent - Swarm Session

You are the **Queen** orchestrating a multi-agent Swarm session. You spawn and coordinate Planners who each manage their own domain.
{hardening}

## Session Info

- **Session ID**: {session_id}
- **Mode**: Swarm (hierarchical with sequential spawning)
- **Prompts Directory**: `.hive-manager/{session_id}/prompts/`
- **Tools Directory**: `.hive-manager/{session_id}/tools/`

## Planners to Spawn

You will spawn {planner_count} planners SEQUENTIALLY. Each planner spawns their own workers.

| # | Domain | Workers |
|---|--------|---------|
{planner_info}

## HTTP API for Spawning Planners

Read `.hive-manager/{session_id}/tools/spawn-planner.md` for detailed documentation.

**Quick Reference:**
```bash
# Spawn a planner
curl -X POST "http://localhost:18800/api/sessions/{session_id}/planners" \
  -H "Content-Type: application/json" \
  -d '{{"domain": "DOMAIN", "cli": "{cli}", "worker_count": N}}'
```

## Your Tools

### Claude Code Tools (Native)
You have full access to all Claude Code tools:
- **Read/Write/Edit** - File operations
- **Bash** - Run shell commands, git operations, curl for HTTP API
- **Glob/Grep** - Search files and content
- **Task** - Spawn subagents for complex investigation (NOT for spawning planners/workers)
- **WebFetch/WebSearch** - Access web resources

### Swarm-Specific Tools (HTTP API)

Tool documentation is in `.hive-manager/{session_id}/tools/`. Read these files for detailed usage:

| Tool | File | Purpose |
|------|------|---------|
| Spawn Planner | `spawn-planner.md` | Spawn planners via HTTP API (visible terminal windows) |
| List Planners | `list-planners.md` | Get list of all planners and their status |
| Spawn Worker | `spawn-worker.md` | Reference only - Planners use this to spawn workers |
| List Workers | `list-workers.md` | Get list of all workers and their status |
| Submit Learning | `submit-learning.md` | Record a learning via HTTP API |
| List Learnings | `list-learnings.md` | Get all learnings for this session |
| Delete Learning | `delete-learning.md` | Remove a learning by ID |

## Learning Curation Protocol

Workers and planners record learnings during task completion. Your curation responsibilities:

1. **Review learnings periodically**:
   ```bash
   curl "http://localhost:18800/api/sessions/{session_id}/learnings"
   ```

2. **Review current project DNA**:
   ```bash
   curl "http://localhost:18800/api/sessions/{session_id}/project-dna"
   ```

3. **Curate useful learnings** into the session-scoped `project-dna.md` via the API:
   - Group by theme/topic
   - Remove duplicates
   - Improve clarity where needed
   - Capture architectural decisions and project conventions

### Session-Scoped Lessons Structure
```
.hive-manager/{session_id}/lessons/
├── learnings.jsonl      # Raw learnings for this session (append-only)
└── project-dna.md       # Curated patterns, conventions, insights
```

### Curation Process
1. Review raw learnings via `GET /api/sessions/{session_id}/learnings`
2. Review current project DNA via `GET /api/sessions/{session_id}/project-dna`
3. Synthesize insights into `project-dna.md` sections:
   - **Patterns That Work** - Successful approaches
   - **Patterns That Failed** - What to avoid
   - **Code Conventions** - Project-specific standards
   - **Architecture Notes** - Key design decisions
4. Delete outdated or duplicate learnings via `DELETE /api/sessions/{{session_id}}/learnings/{{learning_id}}`

### When to Curate
- After each planner completes its domain
- Before creating a PR
- When learnings count exceeds 10

## SEQUENTIAL SPAWNING PROTOCOL WITH COMMITS (CRITICAL)

You MUST spawn planners ONE AT A TIME and COMMIT between each:

### Protocol:

1. **Spawn Planner 1** via HTTP API with domain task
2. **Wait for Planner 1** to signal `[DOMAIN_COMPLETE]`
3. **COMMIT** changes with message: "feat(DOMAIN): [description of domain work]"
4. **Spawn Planner 2** via HTTP API with domain task
5. **Wait for Planner 2** to signal `[DOMAIN_COMPLETE]`
6. **COMMIT** changes with message: "feat(DOMAIN): [description of domain work]"
7. Continue for all {planner_count} planners
8. **Final integration commit** and push

### Monitoring Planner Completion

Check planner status via HTTP API or look for signals:
```bash
# List planners
curl "http://localhost:18800/api/sessions/{session_id}/planners"

# Check coordination log for [DOMAIN_COMPLETE] signals
cat .hive-manager/{session_id}/coordination/coordination.log | grep "DOMAIN_COMPLETE"
```

### Git Commit Pattern

After each planner completes:
```bash
git add -A
git commit -m "feat(DOMAIN): Brief description of what this domain accomplished"
```

## Protocol Summary

1. Analyze task → identify domains
2. For each planner (sequentially):
   a. Spawn planner with domain task
   b. Wait for `[DOMAIN_COMPLETE]` signal
   c. **COMMIT** domain changes
3. Run integration tests
4. Final commit and push

## Your Task

{task}"#,
            hardening = hardening,
            session_id = session_id,
            cli = cli,
            planner_info = planner_info,
            planner_count = planner_count,
            task = user_prompt.unwrap_or("Awaiting instructions from the operator.")
        )
    }

    /// Write a prompt file to the session's prompts directory
    fn write_prompt_file(project_path: &PathBuf, session_id: &str, filename: &str, content: &str) -> Result<PathBuf, String> {
        let prompts_dir = project_path.join(".hive-manager").join(session_id).join("prompts");
        std::fs::create_dir_all(&prompts_dir)
            .map_err(|e| format!("Failed to create prompts directory: {}", e))?;

        let file_path = prompts_dir.join(filename);
        std::fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write prompt file: {}", e))?;

        Ok(file_path)
    }

    /// Write a tool documentation file to the session's tools directory
    fn write_tool_file(project_path: &PathBuf, session_id: &str, filename: &str, content: &str) -> Result<PathBuf, String> {
        let tools_dir = project_path.join(".hive-manager").join(session_id).join("tools");
        std::fs::create_dir_all(&tools_dir)
            .map_err(|e| format!("Failed to create tools directory: {}", e))?;

        let file_path = tools_dir.join(filename);
        std::fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write tool file: {}", e))?;

        Ok(file_path)
    }

    /// Write all standard tool documentation files for a session
    fn write_tool_files(project_path: &PathBuf, session_id: &str, default_cli: &str) -> Result<(), String> {
        // Spawn Worker tool
        let spawn_worker_tool = format!(r#"# Spawn Worker Tool

Spawn a new worker agent in a visible terminal window.

## HTTP API

**Endpoint:** `POST http://localhost:18800/api/sessions/{session_id}/workers`

**Headers:**
```
Content-Type: application/json
```

**Request Body:**
```json
{{
  "role_type": "backend",
  "cli": "{default_cli}",
  "initial_task": "Optional task description"
}}
```

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| role_type | string | Yes | Worker role: backend, frontend, coherence, simplify, reviewer, resolver, tester, code-quality |
| cli | string | No | CLI to use: {default_cli} (default), gemini, codex, opencode, cursor, droid, qwen |
| label | string | No | Custom label for the worker |
| initial_task | string | No | Initial task/prompt for the worker |
| parent_id | string | No | Parent agent ID (defaults to Queen) |

## Example Usage

```bash
# Spawn a backend worker with {default_cli}
curl -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
  -H "Content-Type: application/json" \
  -d '{{"role_type": "backend", "cli": "{default_cli}"}}'

# Spawn a frontend worker with an initial task
curl -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
  -H "Content-Type: application/json" \
  -d '{{"role_type": "frontend", "cli": "{default_cli}", "initial_task": "Implement the login form UI"}}'

# Spawn a reviewer worker
curl -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
  -H "Content-Type: application/json" \
  -d '{{"role_type": "reviewer", "cli": "{default_cli}"}}'
```

## Response

```json
{{
  "worker_id": "{session_id}-worker-N",
  "role": "Backend",
  "cli": "{default_cli}",
  "status": "Running",
  "task_file": ".hive-manager/{session_id}/tasks/worker-N-task.md"
}}
```

## Notes

- Workers spawn in a new Windows Terminal tab (visible window)
- Each worker gets a task file you can update to assign work
- Workers poll their task files for ACTIVE status
- Use this to spawn workers sequentially as tasks complete
"#, session_id = session_id, default_cli = default_cli);

        Self::write_tool_file(project_path, session_id, "spawn-worker.md", &spawn_worker_tool)?;

        // List Workers tool
        let list_workers_tool = format!(r#"# List Workers Tool

Get a list of all workers in the current session.

## HTTP API

**Endpoint:** `GET http://localhost:18800/api/sessions/{session_id}/workers`

## Example Usage

```bash
curl "http://localhost:18800/api/sessions/{session_id}/workers"
```

## Response

```json
{{
  "session_id": "{session_id}",
  "workers": [
    {{
      "id": "{session_id}-worker-1",
      "role": "Backend",
      "cli": "{default_cli}",
      "status": "Running",
      "task_file": ".hive-manager/{session_id}/tasks/worker-1-task.md"
    }}
  ],
  "count": 1
}}
```
"#, session_id = session_id, default_cli = default_cli);

        Self::write_tool_file(project_path, session_id, "list-workers.md", &list_workers_tool)?;

        // Submit Learning tool
        let submit_learning_tool = r#"# Submit Learning Tool

Submit a learning from your work session.

## HTTP API

**Endpoint:** `POST http://localhost:18800/api/sessions/{{session_id}}/learnings`

**Headers:**
```
Content-Type: application/json
```

**Request Body:**
```json
{
  "session": "{{session_id}}",
  "task": "Description of the task you completed",
  "insight": "What you learned or discovered",
  "outcome": "success|partial|failed",
  "keywords": ["keyword1", "keyword2"],
  "files_touched": ["path/to/file.rs"]
}
```

## Required Fields

| Field | Type | Description |
|-------|------|-------------|
| session | string | Current session ID |
| task | string | What task was being performed |
| insight | string | The learning or discovery |
| outcome | string | Category: success, partial, failed |
| keywords | string[] | Relevant keywords for filtering |
| files_touched | string[] | Files involved in this learning |

## Example

```bash
curl -X POST "http://localhost:18800/api/sessions/{{session_id}}/learnings" \
  -H "Content-Type: application/json" \
  -d '{"session": "{{session_id}}", "task": "Implemented DELETE endpoint", "insight": "JSONL files need atomic rewrite via temp-file+rename", "outcome": "success", "keywords": ["jsonl", "atomic-write"], "files_touched": ["src/storage/mod.rs"]}'
```
"#;

        Self::write_tool_file(project_path, session_id, "submit-learning.md", submit_learning_tool)?;

        // List Learnings tool
        let list_learnings_tool = r#"# List Learnings Tool

List all learnings recorded for this session.

## HTTP API

**Endpoint:** `GET http://localhost:18800/api/sessions/{{session_id}}/learnings`

## Query Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| category | string | Filter by outcome category (e.g., "success", "partial") |
| keywords | string | Comma-separated keyword filter (e.g., "api,rust") |

## Example

```bash
# List all learnings
curl "http://localhost:18800/api/sessions/{{session_id}}/learnings"

# Filter by category
curl "http://localhost:18800/api/sessions/{{session_id}}/learnings?category=success"

# Filter by keywords
curl "http://localhost:18800/api/sessions/{{session_id}}/learnings?keywords=api,rust"
```
"#;

        Self::write_tool_file(project_path, session_id, "list-learnings.md", list_learnings_tool)?;

        // Delete Learning tool
        let delete_learning_tool = r#"# Delete Learning Tool

Delete a specific learning by ID.

## HTTP API

**Endpoint:** `DELETE http://localhost:18800/api/sessions/{{session_id}}/learnings/{learning_id}`

## Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| learning_id | string | UUID of the learning to delete |

## Example

```bash
curl -X DELETE "http://localhost:18800/api/sessions/{{session_id}}/learnings/abc-123-def"
```

## Response

- **204 No Content** - Learning deleted successfully
- **404 Not Found** - Learning ID not found
"#;

        Self::write_tool_file(project_path, session_id, "delete-learning.md", delete_learning_tool)?;

        Ok(())
    }

    /// Write tool documentation files for Swarm mode (includes planner tools)
    fn write_swarm_tool_files(project_path: &PathBuf, session_id: &str, planner_count: u8, default_cli: &str) -> Result<(), String> {
        // First write standard worker tools
        Self::write_tool_files(project_path, session_id, default_cli)?;

        // Spawn Planner tool
        let spawn_planner_tool = format!(r#"# Spawn Planner Tool

Spawn a new planner agent in a visible terminal window. Planners manage a domain and spawn workers.

## HTTP API

**Endpoint:** `POST http://localhost:18800/api/sessions/{session_id}/planners`

**Headers:**
```
Content-Type: application/json
```

**Request Body:**
```json
{{
  "domain": "backend",
  "cli": "{default_cli}",
  "worker_count": 2
}}
```

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| domain | string | Yes | Domain for this planner: backend, frontend, testing, infra, etc. |
| cli | string | No | CLI to use: {default_cli} (default), gemini, codex, opencode, cursor, droid, qwen |
| model | string | No | Model to use (e.g., "opus-4-6" for {default_cli}) |
| label | string | No | Custom label for the planner |
| worker_count | number | No | Number of workers this planner will manage (default: 1) |
| workers | array | No | Pre-defined worker configurations |

## Example Usage

```bash
# Spawn a backend planner with 2 workers
curl -X POST "http://localhost:18800/api/sessions/{session_id}/planners" \
  -H "Content-Type: application/json" \
  -d '{{"domain": "backend", "cli": "{default_cli}", "worker_count": 2}}'

# Spawn a frontend planner with specific workers
curl -X POST "http://localhost:18800/api/sessions/{session_id}/planners" \
  -H "Content-Type: application/json" \
  -d '{{
    "domain": "frontend",
    "cli": "{default_cli}",
    "workers": [
      {{"role_type": "ui", "label": "UI Developer"}},
      {{"role_type": "styling", "label": "CSS Specialist"}}
    ]
  }}'
```

## Response

```json
{{
  "planner_id": "{session_id}-planner-N",
  "planner_index": N,
  "domain": "backend",
  "cli": "{default_cli}",
  "status": "Running",
  "worker_count": 2,
  "prompt_file": ".hive-manager/{session_id}/prompts/planner-N-prompt.md",
  "tools_dir": ".hive-manager/{session_id}/tools/"
}}
```

## Sequential Spawning Protocol

1. Spawn Planner 1 → Wait for completion signal
2. **COMMIT changes** with message describing Planner 1's domain work
3. Spawn Planner 2 → Wait for completion signal
4. **COMMIT changes** with message describing Planner 2's domain work
5. Continue for all {planner_count} planners
6. Final integration commit and push

## Notes

- Planners spawn in a new Windows Terminal tab (visible window)
- Each planner knows how to spawn its own workers sequentially
- Wait for `[DOMAIN_COMPLETE]` signal from planner before committing and spawning next
- Commit between each planner to create clean git history
"#, session_id = session_id, planner_count = planner_count, default_cli = default_cli);

        Self::write_tool_file(project_path, session_id, "spawn-planner.md", &spawn_planner_tool)?;

        // List Planners tool
        let list_planners_tool = format!(r#"# List Planners Tool

Get a list of all planners in the current Swarm session.

## HTTP API

**Endpoint:** `GET http://localhost:18800/api/sessions/{session_id}/planners`

## Example Usage

```bash
curl "http://localhost:18800/api/sessions/{session_id}/planners"
```

## Response

```json
{{
  "session_id": "{session_id}",
  "planners": [
    {{
      "id": "{session_id}-planner-1",
      "index": 1,
      "cli": "{default_cli}",
      "label": "Backend Planner",
      "status": "Running",
      "prompt_file": ".hive-manager/{session_id}/prompts/planner-1-prompt.md"
    }}
  ],
  "count": 1
}}
```
"#, session_id = session_id, default_cli = default_cli);

        Self::write_tool_file(project_path, session_id, "list-planners.md", &list_planners_tool)?;

        Ok(())
    }

    /// Write a task file for a worker (STANDBY by default)
    fn write_task_file(project_path: &PathBuf, session_id: &str, worker_index: u8, initial_task: Option<&str>) -> Result<PathBuf, String> {
        let tasks_dir = project_path.join(".hive-manager").join(session_id).join("tasks");
        std::fs::create_dir_all(&tasks_dir)
            .map_err(|e| format!("Failed to create tasks directory: {}", e))?;

        let filename = format!("worker-{}-task.md", worker_index);
        let file_path = tasks_dir.join(&filename);

        let (status, task_content) = if let Some(task) = initial_task {
            ("ACTIVE", task.to_string())
        } else {
            ("STANDBY", "Awaiting task assignment. Monitor this file for updates.".to_string())
        };

        let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let content = format!(
"# Task Assignment - Worker {worker_index}

## Status: {status}

## Role Constraints

- **EXECUTOR**: You have full authority to implement and fix issues.
- **SCOPE**: Stay within your assigned domain/specialization.
- **GIT**: Do NOT push or commit. Provide your changes for the Queen to integrate.

## Instructions

{task_content}

## Completion Protocol

When task is complete, update this file:
1. Change Status to: COMPLETED
2. Add a summary under a new Result section

If blocked, change Status to: BLOCKED and describe the issue.

---
Last updated: {timestamp}
",
            worker_index = worker_index,
            status = status,
            task_content = task_content,
            timestamp = timestamp
        );

        std::fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write task file: {}", e))?;

        Ok(file_path)
    }

    /// Write a task file with a specific status (used for sequential spawning)
    fn write_task_file_with_status(project_path: &PathBuf, session_id: &str, worker_index: u8, initial_task: Option<&str>, status: &str) -> Result<PathBuf, String> {
        let tasks_dir = project_path.join(".hive-manager").join(session_id).join("tasks");
        std::fs::create_dir_all(&tasks_dir)
            .map_err(|e| format!("Failed to create tasks directory: {}", e))?;

        let filename = format!("worker-{}-task.md", worker_index);
        let file_path = tasks_dir.join(&filename);

        let task_content = if let Some(task) = initial_task {
            task.to_string()
        } else {
            "Awaiting task assignment. Monitor this file for updates.".to_string()
        };

        let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let content = format!(
"# Task Assignment - Worker {worker_index}

## Status: {status}

## Role Constraints

- **EXECUTOR**: You have full authority to implement and fix issues.
- **SCOPE**: Stay within your assigned domain/specialization.
- **GIT**: Do NOT push or commit. Provide your changes for the Queen to integrate.

## Instructions

{task_content}

## Completion Protocol

When task is complete, update this file:
1. Change Status to: COMPLETED
2. Add a summary under a new Result section

If blocked, change Status to: BLOCKED and describe the issue.

---
Last updated: {timestamp}
",
            worker_index = worker_index,
            status = status,
            task_content = task_content,
            timestamp = timestamp
        );

        std::fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write task file: {}", e))?;

        Ok(file_path)
    }
    fn launch_solo_internal(
        &self,
        project_path: PathBuf,
        task_description: Option<String>,
        cli: String,
        model: Option<String>,
        flags: Vec<String>,
    ) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();
        let cwd = project_path.to_str().unwrap_or(".");
        let solo_config = AgentConfig {
            cli: cli.clone(),
            model: model.clone(),
            flags,
            label: None,
            role: None,
            initial_prompt: task_description.clone(),
        };
        let (cmd, args) = Self::build_solo_command(&solo_config, task_description.as_deref());
        let solo_id = format!("{}-worker-1", session_id);

        {
            let pty_manager = self.pty_manager.read();
            pty_manager
                .create_session(
                    solo_id.clone(),
                    AgentRole::Worker { index: 1, parent: None },
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn solo agent: {}", e))?;
        }

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Solo {
                cli: cli.clone(),
                model: model.clone(),
            },
            project_path,
            state: SessionState::Running,
            created_at: Utc::now(),
            agents: vec![AgentInfo {
                id: solo_id,
                role: AgentRole::Worker { index: 1, parent: None },
                status: AgentStatus::Running,
                config: solo_config.clone(),
                parent_id: None,
            }],
            default_cli: cli,
            default_model: model,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        self.init_session_storage(&session);
        Ok(session)
    }

    pub fn launch_solo(&self, config: HiveLaunchConfig) -> Result<Session, String> {
        let project_path = PathBuf::from(&config.project_path);
        let task_description = config
            .prompt
            .clone()
            .or_else(|| config.queen_config.initial_prompt.clone());

        self.launch_solo_internal(
            project_path,
            task_description,
            config.queen_config.cli.clone(),
            config.queen_config.model.clone(),
            config.queen_config.flags.clone(),
        )
    }

    pub fn launch_hive_v2(&self, config: HiveLaunchConfig) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();
        let mut agents = Vec::new();
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();

        // If with_planning is true, spawn Master Planner first
        if config.with_planning {
            return self.launch_planning_phase(session_id, config);
        }

        // Solo mode: skip orchestration and launch one agent directly.
        if config.workers.is_empty() {
            return self.launch_solo(config);
        }

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent
            let queen_id = format!("{}-queen", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config);

            // Check if plan.md exists (from previous planning phase)
            let plan_path = project_path.join(".hive-manager").join(&session_id).join("plan.md");
            let has_plan = plan_path.exists();

            // Write Queen prompt to file and pass to CLI
            let master_prompt = Self::build_queen_master_prompt(&config.queen_config.cli, &session_id, &config.workers, config.prompt.as_deref(), has_plan);
            let prompt_file = Self::write_prompt_file(&project_path, &session_id, "queen-prompt.md", &master_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            // Write tool documentation files
            Self::write_tool_files(&project_path, &session_id, &config.queen_config.cli)?;

            tracing::info!("Launching Queen agent (v2): {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    queen_id.clone(),
                    AgentRole::Queen,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Queen: {}", e))?;

            agents.push(AgentInfo {
                id: queen_id.clone(),
                role: AgentRole::Queen,
                status: AgentStatus::Running,
                config: config.queen_config.clone(),
                parent_id: None,
            });

            // Create Worker agents
            for (i, worker_config) in config.workers.iter().enumerate() {
                let index = (i + 1) as u8;
                let worker_id = format!("{}-worker-{}", session_id, index);
                let (cmd, mut args) = Self::build_command(worker_config);

                // Write task file for this worker (STANDBY or with initial task)
                Self::write_task_file(&project_path, &session_id, index, worker_config.initial_prompt.as_deref())?;

                // Write worker prompt to file and pass to CLI
                let worker_prompt = Self::build_worker_prompt(index, worker_config, &queen_id, &session_id);
                let filename = format!("worker-{}-prompt.md", index);
                let prompt_file = Self::write_prompt_file(&project_path, &session_id, &filename, &worker_prompt)?;
                let prompt_path = prompt_file.to_string_lossy().to_string();
                Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

                tracing::info!("Launching Worker {} agent (v2): {} {:?} in {:?}", index, cmd, args, cwd);

                pty_manager
                    .create_session(
                        worker_id.clone(),
                        AgentRole::Worker { index, parent: Some(queen_id.clone()) },
                        &cmd,
                        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                        Some(cwd),
                        120,
                        30,
                    )
                    .map_err(|e| format!("Failed to spawn Worker {}: {}", index, e))?;

                agents.push(AgentInfo {
                    id: worker_id,
                    role: AgentRole::Worker { index, parent: Some(queen_id.clone()) },
                    status: AgentStatus::Running,
                    config: worker_config.clone(),
                    parent_id: Some(queen_id.clone()),
                });
            }
        }

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Hive { worker_count: config.workers.len() as u8 },
            project_path,
            state: SessionState::Running,
            created_at: Utc::now(),
            agents,
            default_cli: config.queen_config.cli.clone(),
            default_model: config.queen_config.model.clone(),
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        // Initialize session storage
        self.init_session_storage(&session);
        self.ensure_task_watcher(&session.id, &session.project_path);

        Ok(session)
    }

    pub fn launch_fusion(&self, config: FusionLaunchConfig) -> Result<Session, String> {
        tracing::info!("launch_fusion called: with_planning={}, variants={}, task={}",
            config.with_planning, config.variants.len(), &config.task_description);

        if config.variants.is_empty() {
            return Err("Fusion launch requires at least one variant".to_string());
        }

        if config.with_planning {
            let session_id = Uuid::new_v4().to_string();
            return self.launch_fusion_planning_phase(session_id, config);
        }

        let session_id = Uuid::new_v4().to_string();
        let project_path = PathBuf::from(&config.project_path);
        let default_cli = if config.default_cli.trim().is_empty() {
            "claude".to_string()
        } else {
            config.default_cli.trim().to_string()
        };

        let mut seen_slugs: HashMap<String, u16> = HashMap::new();
        let mut variants = Vec::new();

        for (idx, variant) in config.variants.iter().enumerate() {
            let index = (idx + 1) as u8;
            let name = if variant.name.trim().is_empty() {
                format!("variant-{}", index)
            } else {
                variant.name.trim().to_string()
            };
            let slug = Self::unique_variant_slug(&name, &mut seen_slugs);
            let branch = format!("fusion/{}/{}", session_id, slug);
            let worktree_path = project_path
                .join(".hive-fusion")
                .join(&session_id)
                .join(format!("variant-{}", slug))
                .to_string_lossy()
                .to_string();
            let task_file = project_path
                .join(".hive-manager")
                .join(&session_id)
                .join("tasks")
                .join(format!("fusion-variant-{}-task.md", index))
                .to_string_lossy()
                .to_string();

            variants.push(FusionVariantMetadata {
                index,
                name,
                slug,
                branch,
                worktree_path,
                task_file,
                agent_id: format!("{}-fusion-{}", session_id, index),
            });
        }

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Fusion {
                variants: variants.iter().map(|v| v.name.clone()).collect(),
            },
            project_path: project_path.clone(),
            state: SessionState::Starting,
            created_at: Utc::now(),
            agents: Vec::new(),
            default_cli: default_cli.clone(),
            default_model: config.default_model.clone(),
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session);
        }
        self.emit_session_update(&session_id);

        let base_branch = format!("fusion/{}/base", session_id);
        Self::run_git_in_dir(&project_path, &["branch", &base_branch, "HEAD"])?;

        for (variant_idx, variant) in variants.iter().enumerate() {
            {
                let mut sessions = self.sessions.write();
                if let Some(s) = sessions.get_mut(&session_id) {
                    s.state = SessionState::SpawningFusionVariant(variant.index);
                }
            }
            self.emit_session_update(&session_id);

            let worktree_path = PathBuf::from(&variant.worktree_path);
            if let Some(parent) = worktree_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create worktree parent dir: {}", e))?;
            }

            Self::run_git_in_dir(
                &project_path,
                &[
                    "worktree",
                    "add",
                    &variant.worktree_path,
                    "-b",
                    &variant.branch,
                    &base_branch,
                ],
            )?;

            Self::write_fusion_variant_task_file(
                &project_path,
                &session_id,
                variant.index,
                &variant.name,
                &config.task_description,
            )?;

            let source_variant = &config.variants[variant_idx];
            let cli = if source_variant.cli.trim().is_empty() {
                default_cli.clone()
            } else {
                source_variant.cli.trim().to_string()
            };
            let variant_agent_config = AgentConfig {
                cli: cli.clone(),
                model: source_variant.model.clone().or(config.default_model.clone()),
                flags: source_variant.flags.clone(),
                label: Some(format!("Fusion {}", variant.name)),
                role: None,
                initial_prompt: Some(config.task_description.clone()),
            };

            let worker_prompt = Self::build_fusion_worker_prompt(
                &session_id,
                variant.index,
                &variant.name,
                &variant.branch,
                &variant.worktree_path,
                &config.task_description,
                &cli,
            );
            let prompt_filename = format!("fusion-worker-{}-prompt.md", variant.index);
            let prompt_file = Self::write_prompt_file(&project_path, &session_id, &prompt_filename, &worker_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();

            let (cmd, mut args) = Self::build_command(&variant_agent_config);
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!(
                "Launching Fusion variant {} ({}) on branch {} in {}",
                variant.index,
                variant.name,
                variant.branch,
                variant.worktree_path
            );

            {
                let pty_manager = self.pty_manager.read();
                pty_manager
                    .create_session(
                        variant.agent_id.clone(),
                        AgentRole::Fusion {
                            variant: variant.name.clone(),
                        },
                        &cmd,
                        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                        Some(&variant.worktree_path),
                        120,
                        30,
                    )
                    .map_err(|e| format!("Failed to spawn Fusion variant {}: {}", variant.name, e))?;
            }

            let agent_info = AgentInfo {
                id: variant.agent_id.clone(),
                role: AgentRole::Fusion {
                    variant: variant.name.clone(),
                },
                status: AgentStatus::Running,
                config: variant_agent_config,
                parent_id: None,
            };

            {
                let mut sessions = self.sessions.write();
                if let Some(s) = sessions.get_mut(&session_id) {
                    s.agents.push(agent_info);
                    s.state = SessionState::WaitingForFusionVariants;
                }
            }
            self.emit_session_update(&session_id);
        }

        let evaluation_dir = project_path
            .join(".hive-manager")
            .join(&session_id)
            .join("evaluation");
        std::fs::create_dir_all(&evaluation_dir)
            .map_err(|e| format!("Failed to create fusion evaluation directory: {}", e))?;

        let decision_file = project_path
            .join(".hive-manager")
            .join(&session_id)
            .join("evaluation")
            .join("decision.md")
            .to_string_lossy()
            .to_string();

        let metadata = FusionSessionMetadata {
            base_branch,
            variants: variants.clone(),
            judge_config: config.judge_config,
            task_description: config.task_description,
            decision_file,
        };
        Self::write_fusion_metadata(&project_path, &session_id, &metadata)?;

        let session = self
            .get_session(&session_id)
            .ok_or_else(|| "Failed to read fusion session after launch".to_string())?;
        self.init_session_storage(&session);
        self.update_session_storage(&session_id);
        self.ensure_task_watcher(&session_id, &project_path);

        Ok(session)
    }

    /// Launch the planning phase - spawns Master Planner only
    fn launch_planning_phase(&self, session_id: String, config: HiveLaunchConfig) -> Result<Session, String> {
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();
        let mut agents = Vec::new();

        // Build the appropriate prompt based on mode
        let planner_prompt = if config.smoke_test {
            tracing::info!("Running in SMOKE TEST mode - skipping real investigation");
            Self::build_smoke_test_prompt(&session_id, &config.workers)
        } else {
            // Pass workers info to Master Planner so it knows how many tasks to create
            let prompt = config.prompt.as_deref().unwrap_or("");
            Self::build_master_planner_prompt(&session_id, prompt, &config.workers)
        };

        {
            let pty_manager = self.pty_manager.read();

            // Create Master Planner agent
            let planner_id = format!("{}-master-planner", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config); // Use queen config for planner

            // Write Master Planner prompt to file
            let prompt_file = Self::write_prompt_file(&project_path, &session_id, "master-planner-prompt.md", &planner_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!("Launching Master Planner: {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    planner_id.clone(),
                    AgentRole::MasterPlanner,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Master Planner: {}", e))?;

            agents.push(AgentInfo {
                id: planner_id,
                role: AgentRole::MasterPlanner,
                status: AgentStatus::Running,
                config: config.queen_config.clone(),
                parent_id: None,
            });
        }

        // Store the pending config for later continuation
        let pending_config_path = project_path.join(".hive-manager").join(&session_id).join("pending-config.json");
        std::fs::create_dir_all(pending_config_path.parent().unwrap())
            .map_err(|e| format!("Failed to create session directory: {}", e))?;
        let config_json = serde_json::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        std::fs::write(&pending_config_path, config_json)
            .map_err(|e| format!("Failed to write pending config: {}", e))?;

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Hive { worker_count: config.workers.len() as u8 },
            project_path,
            state: SessionState::Planning,
            created_at: Utc::now(),
            agents,
            default_cli: config.queen_config.cli.clone(),
            default_model: config.queen_config.model.clone(),
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        self.init_session_storage(&session);
        self.ensure_task_watcher(&session.id, &session.project_path);

        Ok(session)
    }

    /// Launch the planning phase for Fusion - spawns Master Planner only
    fn launch_fusion_planning_phase(&self, session_id: String, config: FusionLaunchConfig) -> Result<Session, String> {
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();
        let mut agents = Vec::new();

        let planner_prompt = Self::build_fusion_master_planner_prompt(
            &session_id,
            &config.task_description,
            &config.variants,
        );

        {
            let pty_manager = self.pty_manager.read();

            let planner_id = format!("{}-master-planner", session_id);
            let queen_cfg = config.queen_config.as_ref().unwrap_or(&config.judge_config);
            let (cmd, mut args) = Self::build_command(queen_cfg);

            let prompt_file = Self::write_prompt_file(&project_path, &session_id, "master-planner-prompt.md", &planner_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!("Launching Master Planner (fusion): {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    planner_id.clone(),
                    AgentRole::MasterPlanner,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Master Planner: {}", e))?;

            agents.push(AgentInfo {
                id: planner_id,
                role: AgentRole::MasterPlanner,
                status: AgentStatus::Running,
                config: queen_cfg.clone(),
                parent_id: None,
            });
        }

        // Store the pending Fusion config for later continuation
        let pending_config_path = project_path.join(".hive-manager").join(&session_id).join("pending-fusion-config.json");
        std::fs::create_dir_all(pending_config_path.parent().unwrap())
            .map_err(|e| format!("Failed to create session directory: {}", e))?;
        let config_json = serde_json::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        std::fs::write(&pending_config_path, config_json)
            .map_err(|e| format!("Failed to write pending config: {}", e))?;

        let variant_names: Vec<String> = config.variants.iter().map(|v| v.name.clone()).collect();
        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Fusion { variants: variant_names },
            project_path: project_path.clone(),
            state: SessionState::Planning,
            created_at: Utc::now(),
            agents,
            default_cli: if config.default_cli.trim().is_empty() { "claude".to_string() } else { config.default_cli.trim().to_string() },
            default_model: config.default_model.clone(),
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        self.init_session_storage(&session);
        self.ensure_task_watcher(&session.id, &session.project_path);

        Ok(session)
    }

    /// Continue a Fusion session after planning phase - spawns Queen + Variants
    fn continue_fusion_after_planning(&self, session_id: &str, session: &Session) -> Result<Session, String> {
        let cwd = session.project_path.to_str().unwrap_or(".");

        // Load the pending Fusion config
        let pending_config_path = session.project_path.join(".hive-manager").join(session_id).join("pending-fusion-config.json");
        let config_json = std::fs::read_to_string(&pending_config_path)
            .map_err(|e| format!("Failed to read pending fusion config: {}", e))?;
        let config: FusionLaunchConfig = serde_json::from_str(&config_json)
            .map_err(|e| format!("Failed to parse pending fusion config: {}", e))?;

        // Clean up Master Planner PTY before spawning Queen
        let planner_id = format!("{}-master-planner", session_id);
        if let Err(e) = self.stop_agent(session_id, &planner_id) {
            tracing::warn!("Failed to stop Master Planner {}: {}", planner_id, e);
        } else {
            tracing::info!("Stopped Master Planner {} before spawning Fusion Queen", planner_id);
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.agents.retain(|a| a.id != planner_id);
            }
        }

        let default_cli = if config.default_cli.trim().is_empty() {
            "claude".to_string()
        } else {
            config.default_cli.trim().to_string()
        };

        // Build variant metadata (same logic as launch_fusion)
        let mut seen_slugs: HashMap<String, u16> = HashMap::new();
        let mut variants = Vec::new();

        for (idx, variant) in config.variants.iter().enumerate() {
            let index = (idx + 1) as u8;
            let name = if variant.name.trim().is_empty() {
                format!("variant-{}", index)
            } else {
                variant.name.trim().to_string()
            };
            let slug = Self::unique_variant_slug(&name, &mut seen_slugs);
            let branch = format!("fusion/{}/{}", session_id, slug);
            let worktree_path = session.project_path
                .join(".hive-fusion")
                .join(session_id)
                .join(format!("variant-{}", slug))
                .to_string_lossy()
                .to_string();
            let task_file = session.project_path
                .join(".hive-manager")
                .join(session_id)
                .join("tasks")
                .join(format!("fusion-variant-{}-task.md", index))
                .to_string_lossy()
                .to_string();

            variants.push(FusionVariantMetadata {
                index,
                name,
                slug,
                branch,
                worktree_path,
                task_file,
                agent_id: format!("{}-fusion-{}", session_id, index),
            });
        }

        // Create git base branch and worktrees
        let base_branch = format!("fusion/{}/base", session_id);
        Self::run_git_in_dir(&session.project_path, &["branch", &base_branch, "HEAD"])?;

        let mut new_agents = Vec::new();

        // Spawn Queen agent
        let queen_cfg = config.queen_config.as_ref().unwrap_or(&config.judge_config).clone();
        {
            let pty_manager = self.pty_manager.read();

            let queen_id = format!("{}-queen", session_id);
            let (cmd, mut args) = Self::build_command(&queen_cfg);

            let queen_prompt = Self::build_fusion_queen_prompt(
                &queen_cfg.cli,
                session_id,
                &variants,
                &config.task_description,
            );
            let prompt_file = Self::write_prompt_file(&session.project_path, session_id, "fusion-queen-prompt.md", &queen_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            // Write tool docs for Queen
            Self::write_tool_files(&session.project_path, session_id, &queen_cfg.cli)?;

            tracing::info!("Launching Fusion Queen: {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    queen_id.clone(),
                    AgentRole::Queen,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Fusion Queen: {}", e))?;

            new_agents.push(AgentInfo {
                id: queen_id,
                role: AgentRole::Queen,
                status: AgentStatus::Running,
                config: queen_cfg,
                parent_id: None,
            });
        }

        // Spawn variants (same logic as launch_fusion)
        for (variant_idx, variant) in variants.iter().enumerate() {
            let worktree_path = PathBuf::from(&variant.worktree_path);
            if let Some(parent) = worktree_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create worktree parent dir: {}", e))?;
            }

            Self::run_git_in_dir(
                &session.project_path,
                &[
                    "worktree", "add",
                    &variant.worktree_path,
                    "-b", &variant.branch,
                    &base_branch,
                ],
            )?;

            Self::write_fusion_variant_task_file(
                &session.project_path,
                session_id,
                variant.index,
                &variant.name,
                &config.task_description,
            )?;

            let source_variant = &config.variants[variant_idx];
            let cli = if source_variant.cli.trim().is_empty() {
                default_cli.clone()
            } else {
                source_variant.cli.trim().to_string()
            };
            let variant_agent_config = AgentConfig {
                cli: cli.clone(),
                model: source_variant.model.clone().or(config.default_model.clone()),
                flags: source_variant.flags.clone(),
                label: Some(format!("Fusion {}", variant.name)),
                role: None,
                initial_prompt: Some(config.task_description.clone()),
            };

            let worker_prompt = Self::build_fusion_worker_prompt(
                session_id,
                variant.index,
                &variant.name,
                &variant.branch,
                &variant.worktree_path,
                &config.task_description,
                &cli,
            );
            let prompt_filename = format!("fusion-worker-{}-prompt.md", variant.index);
            let prompt_file = Self::write_prompt_file(&session.project_path, session_id, &prompt_filename, &worker_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();

            let (cmd, mut args) = Self::build_command(&variant_agent_config);
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!(
                "Launching Fusion variant {} ({}) on branch {} in {}",
                variant.index, variant.name, variant.branch, variant.worktree_path
            );

            {
                let pty_manager = self.pty_manager.read();
                pty_manager
                    .create_session(
                        variant.agent_id.clone(),
                        AgentRole::Fusion { variant: variant.name.clone() },
                        &cmd,
                        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                        Some(&variant.worktree_path),
                        120,
                        30,
                    )
                    .map_err(|e| format!("Failed to spawn Fusion variant {}: {}", variant.name, e))?;
            }

            new_agents.push(AgentInfo {
                id: variant.agent_id.clone(),
                role: AgentRole::Fusion { variant: variant.name.clone() },
                status: AgentStatus::Running,
                config: variant_agent_config,
                parent_id: None,
            });
        }

        // Create evaluation directory
        let evaluation_dir = session.project_path
            .join(".hive-manager")
            .join(session_id)
            .join("evaluation");
        std::fs::create_dir_all(&evaluation_dir)
            .map_err(|e| format!("Failed to create fusion evaluation directory: {}", e))?;

        let decision_file = session.project_path
            .join(".hive-manager")
            .join(session_id)
            .join("evaluation")
            .join("decision.md")
            .to_string_lossy()
            .to_string();

        let metadata = FusionSessionMetadata {
            base_branch,
            variants: variants.clone(),
            judge_config: config.judge_config.clone(),
            task_description: config.task_description,
            decision_file,
        };
        Self::write_fusion_metadata(&session.project_path, session_id, &metadata)?;

        // Update session with new agents and Running state
        let updated_session = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.agents.extend(new_agents);
                s.state = SessionState::WaitingForFusionVariants;
                s.clone()
            } else {
                return Err("Session disappeared".to_string());
            }
        };

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: updated_session.clone(),
            });
        }

        self.update_session_storage(session_id);
        self.ensure_task_watcher(session_id, &updated_session.project_path);

        // Clean up pending config
        let _ = std::fs::remove_file(&pending_config_path);

        Ok(updated_session)
    }

    /// Launch the planning phase for Swarm - spawns Master Planner only
    fn launch_swarm_planning_phase(&self, session_id: String, config: SwarmLaunchConfig) -> Result<Session, String> {
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();
        let mut agents = Vec::new();

        // Build the appropriate prompt based on mode
        let planner_count = if config.planners.is_empty() { config.planner_count } else { config.planners.len() as u8 };
        let planner_prompt = if config.smoke_test {
            tracing::info!("Running in SMOKE TEST mode (swarm) - {} planners, {} workers each", planner_count, config.workers_per_planner.len());
            Self::build_swarm_smoke_test_prompt(&session_id, planner_count, &config.workers_per_planner)
        } else {
            // Pass planners and workers info to Master Planner so it knows the full scope
            let prompt = config.prompt.as_deref().unwrap_or("");
            Self::build_swarm_master_planner_prompt(&session_id, prompt, planner_count, &config.workers_per_planner)
        };

        {
            let pty_manager = self.pty_manager.read();

            // Create Master Planner agent
            let planner_id = format!("{}-master-planner", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config); // Use queen config for planner

            // Write Master Planner prompt to file
            let prompt_file = Self::write_prompt_file(&project_path, &session_id, "master-planner-prompt.md", &planner_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!("Launching Master Planner (swarm): {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    planner_id.clone(),
                    AgentRole::MasterPlanner,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Master Planner: {}", e))?;

            agents.push(AgentInfo {
                id: planner_id,
                role: AgentRole::MasterPlanner,
                status: AgentStatus::Running,
                config: config.queen_config.clone(),
                parent_id: None,
            });
        }

        // Store the pending Swarm config for later continuation
        let pending_config_path = project_path.join(".hive-manager").join(&session_id).join("pending-swarm-config.json");
        std::fs::create_dir_all(pending_config_path.parent().unwrap())
            .map_err(|e| format!("Failed to create session directory: {}", e))?;
        let config_json = serde_json::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        std::fs::write(&pending_config_path, config_json)
            .map_err(|e| format!("Failed to write pending config: {}", e))?;

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Swarm { planner_count: if config.planners.is_empty() { config.planner_count } else { config.planners.len() as u8 } },
            project_path,
            state: SessionState::Planning,
            created_at: Utc::now(),
            agents,
            default_cli: config.queen_config.cli.clone(),
            default_model: config.queen_config.model.clone(),
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        self.init_session_storage(&session);
        self.ensure_task_watcher(&session.id, &session.project_path);

        Ok(session)
    }

    /// Spawn the next worker sequentially
    async fn spawn_next_worker(&self, session_id: &str, worker_index: usize, config: &HiveLaunchConfig, queen_id: &str) -> Result<(), SessionError> {
        let session = self.get_session(session_id)
            .ok_or_else(|| SessionError::NotFound(format!("Session not found: {}", session_id)))?;

        if worker_index >= config.workers.len() {
            // All workers spawned - session complete
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.state = SessionState::Running;
            }
            return Ok(());
        }

        let worker_config = &config.workers[worker_index];
        let index = (worker_index + 1) as u8;
        let cwd = session.project_path.to_str().unwrap_or(".");

        // Update state to spawning this worker
        {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.state = SessionState::SpawningWorker(index);
            }
        }

        let pty_manager = self.pty_manager.read();
        let worker_id = format!("{}-worker-{}", session_id, index);

        // 1. Write task file FIRST (Status: ACTIVE since it's their turn)
        Self::write_task_file_with_status(&session.project_path, session_id, index, worker_config.initial_prompt.as_deref(), "ACTIVE")?;

        // 2. Write worker prompt to file
        let worker_prompt = Self::build_worker_prompt(index, worker_config, queen_id, session_id);
        let filename = format!("worker-{}-prompt.md", index);
        let prompt_file = Self::write_prompt_file(&session.project_path, session_id, &filename, &worker_prompt)?;
        let prompt_path = prompt_file.to_string_lossy().to_string();

        // 3. Build command with prompt
        let (cmd, mut args) = Self::build_command(worker_config);
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

        // 4. Spawn the worker
        pty_manager
            .create_session(
                worker_id.clone(),
                AgentRole::Worker { index, parent: Some(queen_id.to_string()) },
                &cmd,
                &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                Some(cwd),
                120,
                30,
            )
            .map_err(|e| SessionError::SpawnError(format!("Failed to spawn Worker {}: {}", index, e)))?;

        // 5. Add worker to session
        let mut sessions = self.sessions.write();
        if let Some(s) = sessions.get_mut(session_id) {
            s.agents.push(AgentInfo {
                id: worker_id,
                role: AgentRole::Worker { index, parent: Some(queen_id.to_string()) },
                status: AgentStatus::Running,
                config: worker_config.clone(),
                parent_id: Some(queen_id.to_string()),
            });
            s.state = SessionState::WaitingForWorker(index);
        }

        Ok(())
    }

    /// Called when worker-completed event received
    pub async fn on_worker_completed(&self, session_id: &str, worker_id: u8) -> Result<(), SessionError> {
        let session = self.get_session(session_id)
            .ok_or_else(|| SessionError::NotFound(format!("Session not found: {}", session_id)))?;

        // Verify we're in sequential mode and this is the expected worker
        if session.state != SessionState::WaitingForWorker(worker_id) {
            tracing::warn!("Worker {} completed but session in state {:?}", worker_id, session.state);
            return Ok(());
        }

        // Load config - if it doesn't exist, workers may have been spawned via HTTP API
        let pending_config_path = session.project_path.join(".hive-manager").join(session_id).join("pending-config.json");
        if !pending_config_path.exists() {
            tracing::info!("No pending config found for session {} - workers may have been spawned via HTTP API", session_id);
            return Ok(());
        }

        let config_json = std::fs::read_to_string(&pending_config_path)
            .map_err(|e| SessionError::ConfigError(format!("Failed to read pending config: {}", e)))?;
        let config: HiveLaunchConfig = serde_json::from_str(&config_json)
            .map_err(|e| SessionError::ConfigError(format!("Failed to parse pending config: {}", e)))?;

        // Get queen_id
        let queen_id = format!("{}-queen", session_id);

        // 1. Terminate the completed worker's PTY
        self.terminate_worker(session_id, worker_id)?;

        // 2. Spawn next worker
        let next_worker_index = worker_id as usize;
        self.spawn_next_worker(session_id, next_worker_index, &config, &queen_id).await?;

        Ok(())
    }

    pub async fn on_fusion_variant_completed(&self, session_id: &str, variant_index: u8) -> Result<(), SessionError> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| SessionError::NotFound(format!("Session not found: {}", session_id)))?;

        if !matches!(session.session_type, SessionType::Fusion { .. }) {
            return Ok(());
        }

        let metadata = Self::read_fusion_metadata(&session.project_path, session_id)
            .map_err(SessionError::ConfigError)?;
        let variant = metadata
            .variants
            .iter()
            .find(|v| v.index == variant_index)
            .ok_or_else(|| SessionError::ConfigError(format!("Unknown fusion variant index: {}", variant_index)))?;

        {
            let pty_manager = self.pty_manager.read();
            if let Err(e) = pty_manager.kill(&variant.agent_id) {
                tracing::warn!("Failed to stop fusion variant PTY {}: {}", variant.agent_id, e);
            }
        }

        {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                if let Some(agent) = s.agents.iter_mut().find(|a| a.id == variant.agent_id) {
                    agent.status = AgentStatus::Completed;
                }
            }
        }
        self.update_session_storage(session_id);

        let already_judging = {
            let sessions = self.sessions.read();
            sessions.get(session_id).map(|s| {
                matches!(
                    s.state,
                    SessionState::SpawningJudge
                        | SessionState::Judging
                        | SessionState::AwaitingVerdictSelection
                        | SessionState::MergingWinner
                        | SessionState::Completed
                )
            }).unwrap_or(false)
        };
        if already_judging {
            return Ok(());
        }

        if metadata.variants.iter().all(|v| Self::is_task_completed(&v.task_file)) {
            self.spawn_fusion_judge(session_id)
                .map_err(SessionError::SpawnError)?;
        }

        Ok(())
    }

    fn spawn_fusion_judge(&self, session_id: &str) -> Result<(), String> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        if !matches!(session.session_type, SessionType::Fusion { .. }) {
            return Err(format!("Session {} is not a Fusion session", session_id));
        }

        let metadata = Self::read_fusion_metadata(&session.project_path, session_id)?;
        let judge_id = format!("{}-judge", session_id);

        let judge_exists = {
            let sessions = self.sessions.read();
            sessions
                .get(session_id)
                .map(|s| s.agents.iter().any(|a| a.id == judge_id))
                .unwrap_or(false)
        };
        if judge_exists {
            return Ok(());
        }

        {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.state = SessionState::SpawningJudge;
            }
        }
        self.emit_session_update(session_id);

        let judge_prompt = Self::build_fusion_judge_prompt(session_id, &metadata.variants, &metadata.decision_file);
        let prompt_file = Self::write_prompt_file(
            &session.project_path,
            session_id,
            "fusion-judge-prompt.md",
            &judge_prompt,
        )?;
        let prompt_path = prompt_file.to_string_lossy().to_string();

        let mut judge_config = metadata.judge_config.clone();
        if judge_config.cli.trim().is_empty() {
            judge_config.cli = session.default_cli.clone();
        }
        if judge_config.model.is_none() {
            judge_config.model = session.default_model.clone();
        }

        let (cmd, mut args) = Self::build_command(&judge_config);
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

        let cwd = session.project_path.to_string_lossy().to_string();
        {
            let pty_manager = self.pty_manager.read();
            pty_manager
                .create_session(
                    judge_id.clone(),
                    AgentRole::Judge {
                        session_id: session_id.to_string(),
                    },
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(&cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn fusion judge: {}", e))?;
        }

        {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.agents.push(AgentInfo {
                    id: judge_id,
                    role: AgentRole::Judge {
                        session_id: session_id.to_string(),
                    },
                    status: AgentStatus::Running,
                    config: judge_config,
                    parent_id: None,
                });
                s.state = SessionState::Judging;
            }
        }
        self.emit_session_update(session_id);
        self.update_session_storage(session_id);

        Ok(())
    }

    pub fn get_fusion_variant_statuses(&self, session_id: &str) -> Result<Vec<FusionVariantStatus>, String> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        if !matches!(session.session_type, SessionType::Fusion { .. }) {
            return Err(format!("Session {} is not a Fusion session", session_id));
        }

        let metadata = Self::read_fusion_metadata(&session.project_path, session_id)?;
        Ok(metadata
            .variants
            .iter()
            .map(|v| FusionVariantStatus {
                index: v.index,
                name: v.name.clone(),
                branch: v.branch.clone(),
                worktree_path: v.worktree_path.clone(),
                status: Self::read_task_status(&v.task_file),
            })
            .collect())
    }

    pub fn get_fusion_evaluation(&self, session_id: &str) -> Result<(String, Option<String>), String> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        if !matches!(session.session_type, SessionType::Fusion { .. }) {
            return Err(format!("Session {} is not a Fusion session", session_id));
        }

        let metadata = Self::read_fusion_metadata(&session.project_path, session_id)?;
        let report = match std::fs::read_to_string(&metadata.decision_file) {
            Ok(content) => Some(content),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(format!("Failed to read evaluation report: {}", err)),
        };

        if report.is_some() {
            let mut should_emit = false;
            {
                let mut sessions = self.sessions.write();
                if let Some(s) = sessions.get_mut(session_id) {
                    if s.state == SessionState::Judging {
                        s.state = SessionState::AwaitingVerdictSelection;
                        should_emit = true;
                    }
                }
            }
            if should_emit {
                self.emit_session_update(session_id);
                self.update_session_storage(session_id);
            }
        }

        Ok((metadata.decision_file, report))
    }

    pub fn select_fusion_winner(&self, session_id: &str, variant_name: &str) -> Result<(), String> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        if !matches!(session.session_type, SessionType::Fusion { .. }) {
            return Err(format!("Session {} is not a Fusion session", session_id));
        }

        let requested = variant_name.trim();
        if requested.is_empty() {
            return Err("Winner variant name cannot be empty".to_string());
        }

        let metadata = Self::read_fusion_metadata(&session.project_path, session_id)?;
        let requested_slug = Self::slugify_variant_name(requested);
        let winner = metadata
            .variants
            .iter()
            .find(|v| v.name == requested || v.slug == requested_slug)
            .ok_or_else(|| format!("Variant '{}' not found for session {}", requested, session_id))?;

        {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.state = SessionState::MergingWinner;
            }
        }
        self.emit_session_update(session_id);
        self.update_session_storage(session_id);

        Self::run_git_in_dir(&session.project_path, &["merge", "--squash", &winner.branch])?;

        // Commit the squash merge (--squash only stages changes, doesn't commit)
        Self::run_git_in_dir(
            &session.project_path,
            &["commit", "-m", &format!("Merge fusion winner: {}", winner.name)],
        )?;

        let mut cleanup_errors = Vec::new();
        for variant in &metadata.variants {
            if let Err(err) = Self::run_git_in_dir(
                &session.project_path,
                &["worktree", "remove", &variant.worktree_path, "--force"],
            ) {
                cleanup_errors.push(format!("{}: {}", variant.worktree_path, err));
            }

            let pty_manager = self.pty_manager.read();
            if let Err(err) = pty_manager.kill(&variant.agent_id) {
                tracing::warn!("Failed to stop variant agent {}: {}", variant.agent_id, err);
            }
        }
        if let Err(err) = Self::run_git_in_dir(&session.project_path, &["worktree", "prune"]) {
            cleanup_errors.push(format!("worktree prune: {}", err));
        }

        {
            let pty_manager = self.pty_manager.read();
            let judge_id = format!("{}-judge", session_id);
            let _ = pty_manager.kill(&judge_id);
        }

        {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                for agent in &mut s.agents {
                    agent.status = AgentStatus::Completed;
                }
                s.state = SessionState::Completed;
            }
        }
        self.emit_session_update(session_id);
        self.update_session_storage(session_id);

        if cleanup_errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Winner merged, but worktree cleanup had issues: {}",
                cleanup_errors.join(" | ")
            ))
        }
    }

    /// Terminate a worker
    fn terminate_worker(&self, session_id: &str, worker_id: u8) -> Result<(), SessionError> {
        let worker_agent_id = format!("{}-worker-{}", session_id, worker_id);

        let pty_manager = self.pty_manager.read();

        // Kill the PTY
        pty_manager.kill(&worker_agent_id)
            .map_err(|e| SessionError::TerminationError(format!("Failed to kill worker {}: {}", worker_id, e)))?;

        // Update agent status
        let mut sessions = self.sessions.write();
        if let Some(session) = sessions.get_mut(session_id) {
            if let Some(agent) = session.agents.iter_mut().find(|a| a.id == worker_agent_id) {
                agent.status = AgentStatus::Completed;
            }
        }

        Ok(())
    }

    /// Continue a session after planning phase - spawns Queen + Workers/Planners
    pub fn continue_after_planning(&self, session_id: &str) -> Result<Session, String> {
        // Get the session
        let session = {
            let sessions = self.sessions.read();
            sessions.get(session_id).cloned()
        }.ok_or_else(|| format!("Session not found: {}", session_id))?;

        // Verify session is in Planning or PlanReady state
        if session.state != SessionState::Planning && session.state != SessionState::PlanReady {
            return Err(format!("Session is not in planning phase: {:?}", session.state));
        }

        // Dispatch based on session type
        match &session.session_type {
            SessionType::Swarm { .. } => {
                return self.continue_swarm_after_planning(session_id, &session);
            }
            SessionType::Fusion { .. } => {
                return self.continue_fusion_after_planning(session_id, &session);
            }
            SessionType::Solo { .. } => {
                return Err("Solo sessions do not support planning continuation".to_string());
            }
            _ => {} // Continue with Hive logic below
        }

        let cwd = session.project_path.to_str().unwrap_or(".");

        // Load the pending config
        let pending_config_path = session.project_path.join(".hive-manager").join(session_id).join("pending-config.json");
        let config_json = std::fs::read_to_string(&pending_config_path)
            .map_err(|e| format!("Failed to read pending config: {}", e))?;
        let config: HiveLaunchConfig = serde_json::from_str(&config_json)
            .map_err(|e| format!("Failed to parse pending config: {}", e))?;

        // Clean up Master Planner PTY before spawning Queen (fixes terminal corruption)
        let planner_id = format!("{}-master-planner", session_id);
        if let Err(e) = self.stop_agent(session_id, &planner_id) {
            tracing::warn!("Failed to stop Master Planner {}: {}", planner_id, e);
        } else {
            tracing::info!("Stopped Master Planner {} before spawning Queen", planner_id);
            // Remove Master Planner from agents list to prevent resource leaks
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.agents.retain(|a| a.id != planner_id);
            }
        }

        let mut new_agents = Vec::new();

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent
            let queen_id = format!("{}-queen", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config);

            // Plan should exist now
            let has_plan = session.project_path.join(".hive-manager").join(session_id).join("plan.md").exists();

            // Write Queen prompt with plan reference
            let master_prompt = Self::build_queen_master_prompt(&config.queen_config.cli, session_id, &config.workers, config.prompt.as_deref(), has_plan);
            let prompt_file = Self::write_prompt_file(&session.project_path, session_id, "queen-prompt.md", &master_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            // Write tool documentation files
            Self::write_tool_files(&session.project_path, session_id, &config.queen_config.cli)?;

            tracing::info!("Launching Queen agent (after planning): {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    queen_id.clone(),
                    AgentRole::Queen,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Queen: {}", e))?;

            new_agents.push(AgentInfo {
                id: queen_id.clone(),
                role: AgentRole::Queen,
                status: AgentStatus::Running,
                config: config.queen_config.clone(),
                parent_id: None,
            });

            // Queen will spawn workers via HTTP API after reading the plan
            // No auto-spawning of workers - Queen controls the flow
        }

        // Update session with new agents - Queen will spawn workers
        let updated_session = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.agents.extend(new_agents);
                // Set state to Running - Queen will spawn workers via HTTP API
                s.state = SessionState::Running;
                s.clone()
            } else {
                return Err("Session disappeared".to_string());
            }
        };

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: updated_session.clone(),
            });
        }

        // Update storage
        self.update_session_storage(session_id);
        self.ensure_task_watcher(session_id, &updated_session.project_path);
        self.ensure_task_watcher(session_id, &updated_session.project_path);

        // Clean up pending config file
        let _ = std::fs::remove_file(&pending_config_path);

        Ok(updated_session)
    }

    /// Mark a planning session as ready (plan generated)
    pub fn mark_plan_ready(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.write();
        if let Some(session) = sessions.get_mut(session_id) {
            if session.state == SessionState::Planning {
                session.state = SessionState::PlanReady;

                if let Some(ref app_handle) = self.app_handle {
                    let _ = app_handle.emit("session-update", SessionUpdate {
                        session: session.clone(),
                    });
                }
                Ok(())
            } else {
                Err(format!("Session is not in planning state: {:?}", session.state))
            }
        } else {
            Err(format!("Session not found: {}", session_id))
        }
    }

    /// Resume a persisted session from storage
    pub fn resume_session(&self, session_id: &str) -> Result<Session, String> {
        // Validate session ID format to prevent path traversal
        if session_id.contains("..") || session_id.contains("/") || session_id.contains("\\") {
            return Err("Invalid session ID format".to_string());
        }

        // Check if session is already loaded in memory
        {
            let sessions = self.sessions.read();
            if sessions.contains_key(session_id) {
                return Err("Session is already loaded".to_string());
            }
        }

        // Load session from storage
        let storage = SessionStorage::new()
            .map_err(|e| format!("Failed to initialize storage: {}", e))?;
        let persisted = storage.load_session(session_id)
            .map_err(|e| format!("Failed to load session from storage: {}", e))?;

        // Convert persisted session to active session
        let session_type = match persisted.session_type {
            crate::storage::SessionTypeInfo::Hive { worker_count } => SessionType::Hive { worker_count },
            crate::storage::SessionTypeInfo::Swarm { planner_count } => SessionType::Swarm { planner_count },
            crate::storage::SessionTypeInfo::Fusion { variants } => SessionType::Fusion { variants },
            crate::storage::SessionTypeInfo::Solo { cli, model } => SessionType::Solo { cli, model },
        };

        // Convert persisted agents to active agents
        let agents: Vec<AgentInfo> = persisted.agents.iter().filter_map(|pa| {
            // Parse the role string (e.g., "Queen", "Planner(0)", "Worker(1)")
            let role = if pa.role == "MasterPlanner" {
                AgentRole::MasterPlanner
            } else if pa.role == "Queen" {
                AgentRole::Queen
            } else if pa.role.starts_with("Planner(") {
                let index_str = pa.role.trim_start_matches("Planner(").trim_end_matches(")");
                let index = index_str.parse::<u8>().ok()?;
                AgentRole::Planner { index }
            } else if pa.role.starts_with("Worker(") {
                let parts: Vec<&str> = pa.role.trim_start_matches("Worker(").trim_end_matches(")").split(',').collect();
                let index = parts.get(0)?.parse::<u8>().ok()?;
                let parent = parts.get(1).and_then(|s: &&str| {
                    let trimmed = s.trim();
                    if trimmed == "None" {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                });
                AgentRole::Worker { index, parent }
            } else if pa.role.starts_with("Fusion(") {
                let variant = pa.role.trim_start_matches("Fusion(").trim_end_matches(")").to_string();
                AgentRole::Fusion { variant }
            } else if pa.role.starts_with("Judge(") {
                let parsed_session_id = pa.role.trim_start_matches("Judge(").trim_end_matches(")").to_string();
                AgentRole::Judge { session_id: parsed_session_id }
            } else {
                return None;  // Skip unparseable roles
            };

            // Convert PersistedAgentConfig to AgentConfig
            let config = AgentConfig {
                cli: pa.config.cli.clone(),
                model: pa.config.model.clone(),
                flags: pa.config.flags.clone(),
                label: pa.config.label.clone(),
                role: pa.config.role_type.as_ref().map(|rt: &String| WorkerRole {
                    role_type: rt.clone(),
                    label: pa.config.label.clone().unwrap_or_default(),
                    default_cli: pa.config.cli.clone(),
                    prompt_template: pa.config.initial_prompt.clone(),
                }),
                initial_prompt: pa.config.initial_prompt.clone(),
            };

            Some(AgentInfo {
                id: pa.id.clone(),
                role,
                status: AgentStatus::Completed,  // All persisted sessions are completed
                config,
                parent_id: pa.parent_id.clone(),
            })
        }).collect();

        // Create session object
        let session = Session {
            id: persisted.id.clone(),
            session_type,
            project_path: PathBuf::from(persisted.project_path),
            state: SessionState::Completed,  // Persisted sessions are completed
            created_at: persisted.created_at,
            agents,
            default_cli: persisted.default_cli.clone(),
            default_model: persisted.default_model.clone(),
        };

        // Add to in-memory sessions
        {
            let mut sessions = self.sessions.write();
            sessions.insert(session.id.clone(), session.clone());
        }

        // Emit session-update event to frontend
        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        Ok(session)
    }

    /// Continue a Swarm session after planning phase
    fn continue_swarm_after_planning(&self, session_id: &str, session: &Session) -> Result<Session, String> {
        let cwd = session.project_path.to_str().unwrap_or(".");

        // Load the pending Swarm config
        let pending_config_path = session.project_path.join(".hive-manager").join(session_id).join("pending-swarm-config.json");
        let config_json = std::fs::read_to_string(&pending_config_path)
            .map_err(|e| format!("Failed to read pending swarm config: {}", e))?;
        let config: SwarmLaunchConfig = serde_json::from_str(&config_json)
            .map_err(|e| format!("Failed to parse pending swarm config: {}", e))?;

        // Generate planners from simplified config (or use legacy planners if provided)
        let planners: Vec<PlannerConfig> = if !config.planners.is_empty() {
            config.planners.clone()
        } else {
            (0..config.planner_count)
                .map(|i| PlannerConfig {
                    config: config.planner_config.clone(),
                    domain: format!("domain-{}", i + 1),
                    workers: config.workers_per_planner.clone(),
                })
                .collect()
        };

        // Clean up Master Planner PTY before spawning Queen
        let planner_id = format!("{}-master-planner", session_id);
        if let Err(e) = self.stop_agent(session_id, &planner_id) {
            tracing::warn!("Failed to stop Master Planner {}: {}", planner_id, e);
        } else {
            tracing::info!("Stopped Master Planner {} before spawning Queen", planner_id);
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.agents.retain(|a| a.id != planner_id);
            }
        }

        let mut new_agents = Vec::new();

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent ONLY - planners will be spawned sequentially by Queen via HTTP API
            let queen_id = format!("{}-queen", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config);

            // Write Queen prompt with sequential planner spawning protocol
            let master_prompt = Self::build_swarm_queen_prompt(&config.queen_config.cli, session_id, &planners, config.prompt.as_deref());
            let prompt_file = Self::write_prompt_file(&session.project_path, session_id, "queen-prompt.md", &master_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            // Write Swarm tool documentation files (includes spawn-planner.md)
            Self::write_swarm_tool_files(&session.project_path, session_id, planners.len() as u8, &config.queen_config.cli)?;

            tracing::info!("Launching Queen agent (swarm - sequential planner spawning, after planning): {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    queen_id.clone(),
                    AgentRole::Queen,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Queen: {}", e))?;

            new_agents.push(AgentInfo {
                id: queen_id.clone(),
                role: AgentRole::Queen,
                status: AgentStatus::Running,
                config: config.queen_config.clone(),
                parent_id: None,
            });

            // NOTE: Planners and Workers are NOT spawned here anymore
            // Queen will spawn them sequentially via HTTP API and commit between each planner
        }

        // Store planner config for Queen to reference when spawning via HTTP API
        let swarm_config_path = session.project_path.join(".hive-manager").join(session_id).join("swarm-planners.json");
        let planners_json = serde_json::to_string_pretty(&planners)
            .map_err(|e| format!("Failed to serialize planner config: {}", e))?;
        std::fs::write(&swarm_config_path, planners_json)
            .map_err(|e| format!("Failed to write planner config: {}", e))?;

        // Update session with new agents and Running state
        let updated_session = {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                session.agents.extend(new_agents);
                session.state = SessionState::Running;  // Queen will spawn planners sequentially
                session.clone()
            } else {
                return Err("Session disappeared".to_string());
            }
        };

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: updated_session.clone(),
            });
        }

        // Update storage
        self.update_session_storage(session_id);
        self.ensure_task_watcher(session_id, &updated_session.project_path);

        // Clean up pending config file (keep swarm-planners.json for Queen reference)
        let _ = std::fs::remove_file(&pending_config_path);

        Ok(updated_session)
    }

    pub fn launch_swarm(&self, config: SwarmLaunchConfig) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();

        // If with_planning is true, spawn Master Planner first
        if config.with_planning {
            return self.launch_swarm_planning_phase(session_id, config);
        }

        // Generate planners from simplified config (or use legacy planners if provided)
        let planners: Vec<PlannerConfig> = if !config.planners.is_empty() {
            config.planners.clone()
        } else {
            (0..config.planner_count)
                .map(|i| PlannerConfig {
                    config: config.planner_config.clone(),
                    domain: format!("domain-{}", i + 1),
                    workers: config.workers_per_planner.clone(),
                })
                .collect()
        };

        let mut agents = Vec::new();
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent ONLY - planners will be spawned sequentially by Queen via HTTP API
            let queen_id = format!("{}-queen", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config);

            // Write Queen prompt to file and pass to CLI
            let master_prompt = Self::build_swarm_queen_prompt(&config.queen_config.cli, &session_id, &planners, config.prompt.as_deref());
            let prompt_file = Self::write_prompt_file(&project_path, &session_id, "queen-prompt.md", &master_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            // Write Swarm tool documentation files (includes spawn-planner.md)
            Self::write_swarm_tool_files(&project_path, &session_id, planners.len() as u8, &config.queen_config.cli)?;

            tracing::info!("Launching Queen agent (swarm - sequential planner spawning): {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    queen_id.clone(),
                    AgentRole::Queen,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Queen: {}", e))?;

            agents.push(AgentInfo {
                id: queen_id.clone(),
                role: AgentRole::Queen,
                status: AgentStatus::Running,
                config: config.queen_config.clone(),
                parent_id: None,
            });

            // NOTE: Planners and Workers are NOT spawned here anymore
            // Queen will spawn them sequentially via HTTP API and commit between each planner
        }

        // Store planner config for Queen to reference when spawning
        let swarm_config_path = project_path.join(".hive-manager").join(&session_id).join("swarm-planners.json");
        std::fs::create_dir_all(swarm_config_path.parent().unwrap())
            .map_err(|e| format!("Failed to create session directory: {}", e))?;
        let planners_json = serde_json::to_string_pretty(&planners)
            .map_err(|e| format!("Failed to serialize planner config: {}", e))?;
        std::fs::write(&swarm_config_path, planners_json)
            .map_err(|e| format!("Failed to write planner config: {}", e))?;

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Swarm { planner_count: planners.len() as u8 },
            project_path,
            state: SessionState::Running,  // Queen will spawn planners sequentially
            created_at: Utc::now(),
            agents,
            default_cli: config.queen_config.cli.clone(),
            default_model: config.queen_config.model.clone(),
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        // Initialize session storage if available
        self.init_session_storage(&session);
        self.ensure_task_watcher(&session.id, &session.project_path);

        Ok(session)
    }

    /// Add a worker to an existing session
    pub fn add_worker(
        &self,
        session_id: &str,
        config: AgentConfig,
        role: WorkerRole,
        parent_id: Option<String>,
    ) -> Result<AgentInfo, String> {
        // Get session and validate
        let session = {
            let sessions = self.sessions.read();
            sessions.get(session_id).cloned()
        }.ok_or_else(|| format!("Session not found: {}", session_id))?;

        // Allow adding workers when:
        // - Running: Normal operation
        // - WaitingForWorker: Queen spawning workers sequentially (Hive mode)
        // - WaitingForPlanner: Planner spawning workers (Swarm mode)
        let can_add_worker = matches!(
            session.state,
            SessionState::Running | SessionState::WaitingForWorker(_) | SessionState::WaitingForPlanner(_)
        );
        if !can_add_worker {
            return Err(format!("Cannot add worker to session in state {:?}", session.state));
        }

        // Determine worker index
        let existing_workers = session.agents.iter()
            .filter(|a| matches!(a.role, AgentRole::Worker { .. }))
            .count();
        let worker_index = (existing_workers + 1) as u8;

        // Determine parent (default to Queen)
        let actual_parent_id = parent_id.unwrap_or_else(|| format!("{}-queen", session_id));

        // Generate worker ID
        let worker_id = format!("{}-worker-{}", session_id, worker_index);

        // Build command
        let (cmd, mut args) = Self::build_command(&config);

        // Get project path
        let cwd = session.project_path.to_str().unwrap_or(".");

        // Create a temporary config with role for prompt generation
        let mut config_with_role = config.clone();
        config_with_role.role = Some(role.clone());

        // Write task file for this worker (STANDBY or with initial task)
        Self::write_task_file(&session.project_path, session_id, worker_index, config_with_role.initial_prompt.as_deref())?;

        // Write worker prompt to file and add to args
        let worker_prompt = Self::build_worker_prompt(worker_index, &config_with_role, &actual_parent_id, session_id);
        let filename = format!("worker-{}-prompt.md", worker_index);
        let prompt_file = Self::write_prompt_file(&session.project_path, session_id, &filename, &worker_prompt)?;
        let prompt_path = prompt_file.to_string_lossy().to_string();
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

        tracing::info!(
            "Adding Worker {} ({}) to session {}: {} {:?}",
            worker_index,
            role.label,
            session_id,
            cmd,
            args
        );

        // Spawn PTY
        {
            let pty_manager = self.pty_manager.read();
            pty_manager
                .create_session(
                    worker_id.clone(),
                    AgentRole::Worker { index: worker_index, parent: Some(actual_parent_id.clone()) },
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Worker {}: {}", worker_index, e))?;
        }

        // Create agent info with role
        let mut agent_config = config;
        agent_config.role = Some(role);

        let agent_info = AgentInfo {
            id: worker_id.clone(),
            role: AgentRole::Worker { index: worker_index, parent: Some(actual_parent_id.clone()) },
            status: AgentStatus::Running,
            config: agent_config,
            parent_id: Some(actual_parent_id),
        };

        // Update session
        {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                session.agents.push(agent_info.clone());
            }
        }

        // Emit session update
        if let Some(ref app_handle) = self.app_handle {
            let sessions = self.sessions.read();
            if let Some(session) = sessions.get(session_id) {
                let _ = app_handle.emit("session-update", SessionUpdate {
                    session: session.clone(),
                });
            }
        }

        // Update session storage
        self.update_session_storage(session_id);
        self.ensure_task_watcher(session_id, &session.project_path);

        Ok(agent_info)
    }

    /// Add a planner to a Swarm session (called by Queen via HTTP API)
    pub fn add_planner(
        &self,
        session_id: &str,
        config: AgentConfig,
        domain: String,
        workers: Vec<AgentConfig>,
    ) -> Result<AgentInfo, String> {
        // Get session and validate
        let session = {
            let sessions = self.sessions.read();
            sessions.get(session_id).cloned()
        }.ok_or_else(|| format!("Session not found: {}", session_id))?;

        // Allow adding planners when Running or WaitingForPlanner
        let can_add_planner = matches!(
            session.state,
            SessionState::Running | SessionState::WaitingForPlanner(_)
        );
        if !can_add_planner {
            return Err(format!("Cannot add planner to session in state {:?}", session.state));
        }

        // Determine planner index
        let existing_planners = session.agents.iter()
            .filter(|a| matches!(a.role, AgentRole::Planner { .. }))
            .count();
        let planner_index = (existing_planners + 1) as u8;

        // Get queen ID as parent
        let queen_id = format!("{}-queen", session_id);

        // Generate planner ID
        let planner_id = format!("{}-planner-{}", session_id, planner_index);

        // Build command
        let (cmd, mut args) = Self::build_command(&config);

        let default_cli = session.default_cli.as_str();

        // Get project path
        let cwd = session.project_path.to_str().unwrap_or(".");

        // Build PlannerConfig for prompt generation
        let planner_config = PlannerConfig {
            config: config.clone(),
            domain: domain.clone(),
            workers: workers.clone(),
        };

        // Write planner prompt to file and add to args
        let planner_prompt = Self::build_planner_prompt_with_http(&config.cli, planner_index, &planner_config, &queen_id, session_id);
        let filename = format!("planner-{}-prompt.md", planner_index);
        let prompt_file = Self::write_prompt_file(&session.project_path, session_id, &filename, &planner_prompt)?;
        let prompt_path = prompt_file.to_string_lossy().to_string();
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

        // Write tool files for the planner (spawn-worker.md)
        Self::write_tool_files(&session.project_path, session_id, default_cli)?;

        tracing::info!(
            "Adding Planner {} ({}) to session {}: {} {:?}",
            planner_index,
            domain,
            session_id,
            cmd,
            args
        );

        // Spawn PTY
        {
            let pty_manager = self.pty_manager.read();
            pty_manager
                .create_session(
                    planner_id.clone(),
                    AgentRole::Planner { index: planner_index },
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Planner {}: {}", planner_index, e))?;
        }

        // Create agent info
        let mut agent_config = config;
        agent_config.label = Some(format!("{} Planner", domain));

        let agent_info = AgentInfo {
            id: planner_id.clone(),
            role: AgentRole::Planner { index: planner_index },
            status: AgentStatus::Running,
            config: agent_config,
            parent_id: Some(queen_id),
        };

        // Update session state to WaitingForPlanner
        {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                session.agents.push(agent_info.clone());
                session.state = SessionState::WaitingForPlanner(planner_index);
            }
        }

        // Emit session update
        if let Some(ref app_handle) = self.app_handle {
            let sessions = self.sessions.read();
            if let Some(session) = sessions.get(session_id) {
                let _ = app_handle.emit("session-update", SessionUpdate {
                    session: session.clone(),
                });
            }
        }

        // Update session storage
        self.update_session_storage(session_id);
        self.ensure_task_watcher(session_id, &session.project_path);

        // Store planner's worker config for sequential spawning
        let planner_workers_path = session.project_path
            .join(".hive-manager")
            .join(session_id)
            .join(format!("planner-{}-workers.json", planner_index));
        if !workers.is_empty() {
            let workers_json = serde_json::to_string_pretty(&workers)
                .map_err(|e| format!("Failed to serialize worker config: {}", e))?;
            std::fs::write(&planner_workers_path, workers_json)
                .map_err(|e| format!("Failed to write planner workers config: {}", e))?;
        }

        Ok(agent_info)
    }

    /// Initialize session storage for a new session
    /// Convert a Session to PersistedSession for storage
    fn session_to_persisted(&self, session: &Session) -> crate::storage::PersistedSession {
        use crate::storage::{PersistedSession, SessionTypeInfo, PersistedAgentInfo, PersistedAgentConfig};

        let session_type = match &session.session_type {
            SessionType::Hive { worker_count } => SessionTypeInfo::Hive { worker_count: *worker_count },
            SessionType::Swarm { planner_count } => SessionTypeInfo::Swarm { planner_count: *planner_count },
            SessionType::Fusion { variants } => SessionTypeInfo::Fusion { variants: variants.clone() },
            SessionType::Solo { cli, model } => SessionTypeInfo::Solo {
                cli: cli.clone(),
                model: model.clone(),
            },
        };

        let agents: Vec<PersistedAgentInfo> = session.agents.iter().map(|a| {
            let role_str = match &a.role {
                AgentRole::MasterPlanner => "MasterPlanner".to_string(),
                AgentRole::Queen => "Queen".to_string(),
                AgentRole::Planner { index } => format!("Planner({})", index),
                AgentRole::Worker { index, parent } => format!("Worker({},{})", index, parent.as_deref().unwrap_or("None")),
                AgentRole::Fusion { variant } => format!("Fusion({})", variant),
                AgentRole::Judge { session_id } => format!("Judge({})", session_id),
            };

            PersistedAgentInfo {
                id: a.id.clone(),
                role: role_str,
                config: PersistedAgentConfig {
                    cli: a.config.cli.clone(),
                    model: a.config.model.clone(),
                    flags: a.config.flags.clone(),
                    label: a.config.label.clone(),
                    role_type: a.config.role.as_ref().map(|r| r.role_type.clone()),
                    initial_prompt: a.config.initial_prompt.clone(),
                },
                parent_id: a.parent_id.clone(),
            }
        }).collect();

        let state_str = match &session.state {
            SessionState::Planning => "Planning",
            SessionState::PlanReady => "PlanReady",
            SessionState::Starting => "Starting",
            SessionState::SpawningWorker(_) => "SpawningWorker",
            SessionState::WaitingForWorker(_) => "WaitingForWorker",
            SessionState::SpawningPlanner(_) => "SpawningPlanner",
            SessionState::WaitingForPlanner(_) => "WaitingForPlanner",
            SessionState::SpawningFusionVariant(_) => "SpawningFusionVariant",
            SessionState::WaitingForFusionVariants => "WaitingForFusionVariants",
            SessionState::SpawningJudge => "SpawningJudge",
            SessionState::Judging => "Judging",
            SessionState::AwaitingVerdictSelection => "AwaitingVerdictSelection",
            SessionState::MergingWinner => "MergingWinner",
            SessionState::Running => "Running",
            SessionState::Paused => "Paused",
            SessionState::Completed => "Completed",
            SessionState::Failed(_) => "Failed",
        }.to_string();

        PersistedSession {
            id: session.id.clone(),
            session_type,
            project_path: session.project_path.to_string_lossy().to_string(),
            created_at: session.created_at,
            agents,
            state: state_str,
            default_cli: session.default_cli.clone(),
            default_model: session.default_model.clone(),
        }
    }

    fn init_session_storage(&self, session: &Session) {
        if let Some(ref storage) = self.storage {
            // Create session directory
            if let Err(e) = storage.create_session_dir(&session.id) {
                tracing::warn!("Failed to create session directory: {}", e);
                return;
            }

            // Save session metadata to session.json
            let persisted = self.session_to_persisted(session);
            if let Err(e) = storage.save_session(&persisted) {
                tracing::warn!("Failed to save session metadata: {}", e);
            }

            // Build hierarchy nodes
            let hierarchy: Vec<HierarchyNode> = session.agents.iter().map(|agent| {
                let role_str = match &agent.role {
                    AgentRole::MasterPlanner => "MasterPlanner".to_string(),
                    AgentRole::Queen => "Queen".to_string(),
                    AgentRole::Planner { index } => format!("Planner-{}", index),
                    AgentRole::Worker { index, .. } => format!("Worker-{}", index),
                    AgentRole::Fusion { variant } => format!("Fusion-{}", variant),
                    AgentRole::Judge { session_id } => format!("Judge-{}", session_id),
                };

                let children: Vec<String> = session.agents.iter()
                    .filter(|a| a.parent_id.as_ref() == Some(&agent.id))
                    .map(|a| a.id.clone())
                    .collect();

                HierarchyNode {
                    id: agent.id.clone(),
                    role: role_str,
                    parent_id: agent.parent_id.clone(),
                    children,
                }
            }).collect();

            // Build worker state info
            let workers: Vec<WorkerStateInfo> = session.agents.iter()
                .filter(|a| !matches!(a.role, AgentRole::Queen))
                .map(|a| WorkerStateInfo {
                    id: a.id.clone(),
                    role: a.config.role.clone().unwrap_or_default(),
                    cli: a.config.cli.clone(),
                    status: format!("{:?}", a.status),
                    current_task: None,
                    last_update: Utc::now(),
                    last_heartbeat: None,
                })
                .collect();

            // Update state files
            let state_manager = StateManager::new(storage.session_dir(&session.id));
            if let Err(e) = state_manager.update_hierarchy(&hierarchy) {
                tracing::warn!("Failed to update hierarchy: {}", e);
            }
            if let Err(e) = state_manager.update_workers_file(&workers) {
                tracing::warn!("Failed to update workers file: {}", e);
            }
        }
    }

    fn ensure_task_watcher(&self, session_id: &str, project_path: &PathBuf) {
        let app_handle = match self.app_handle.clone() {
            Some(handle) => handle,
            None => return,
        };

        let mut watchers = self.task_watchers.lock();
        if watchers.contains_key(session_id) {
            return;
        }

        let session_path = project_path.join(".hive-manager").join(session_id);
        let tasks_path = session_path.join("tasks");
        if !tasks_path.exists() {
            return;
        }

        match TaskFileWatcher::new(&session_path, session_id, app_handle) {
            Ok(watcher) => {
                watchers.insert(session_id.to_string(), watcher);
            }
            Err(e) => {
                tracing::warn!("Failed to start task watcher for {}: {}", session_id, e);
            }
        }
    }

    /// Update session storage after changes
    fn update_session_storage(&self, session_id: &str) {
        if let Some(ref storage) = self.storage {
            let sessions = self.sessions.read();
            if let Some(session) = sessions.get(session_id) {
                // Update session.json with latest state
                let persisted = self.session_to_persisted(session);
                if let Err(e) = storage.save_session(&persisted) {
                    tracing::warn!("Failed to update session metadata: {}", e);
                }

                // Build hierarchy nodes
                let hierarchy: Vec<HierarchyNode> = session.agents.iter().map(|agent| {
                    let role_str = match &agent.role {
                        AgentRole::MasterPlanner => "MasterPlanner".to_string(),
                        AgentRole::Queen => "Queen".to_string(),
                        AgentRole::Planner { index } => format!("Planner-{}", index),
                        AgentRole::Worker { index, .. } => format!("Worker-{}", index),
                        AgentRole::Fusion { variant } => format!("Fusion-{}", variant),
                        AgentRole::Judge { session_id } => format!("Judge-{}", session_id),
                    };

                    let children: Vec<String> = session.agents.iter()
                        .filter(|a| a.parent_id.as_ref() == Some(&agent.id))
                        .map(|a| a.id.clone())
                        .collect();

                    HierarchyNode {
                        id: agent.id.clone(),
                        role: role_str,
                        parent_id: agent.parent_id.clone(),
                        children,
                    }
                }).collect();

                // Build worker state info
                let workers: Vec<WorkerStateInfo> = session.agents.iter()
                    .filter(|a| !matches!(a.role, AgentRole::Queen))
                    .map(|a| WorkerStateInfo {
                        id: a.id.clone(),
                        role: a.config.role.clone().unwrap_or_default(),
                        cli: a.config.cli.clone(),
                        status: format!("{:?}", a.status),
                        current_task: None,
                        last_update: Utc::now(),
                        last_heartbeat: None,
                    })
                    .collect();

                // Update state files
                let state_manager = StateManager::new(storage.session_dir(session_id));
                if let Err(e) = state_manager.update_hierarchy(&hierarchy) {
                    tracing::warn!("Failed to update hierarchy: {}", e);
                }
                if let Err(e) = state_manager.update_workers_file(&workers) {
                    tracing::warn!("Failed to update workers file: {}", e);
                }
            }
        }
    }
}

impl Default for SessionController {
    fn default() -> Self {
        Self::new(Arc::new(RwLock::new(PtyManager::new())))
    }
}

#[cfg(test)]
mod tests {
    use super::SessionState;

    #[test]
    fn session_state_variants_exist() {
        let _planning = SessionState::Planning;
        let _plan_ready = SessionState::PlanReady;
        let _starting = SessionState::Starting;
        let _spawning = SessionState::SpawningWorker(1);
        let _waiting = SessionState::WaitingForWorker(1);
        let _spawning_fusion = SessionState::SpawningFusionVariant(1);
        let _waiting_fusion = SessionState::WaitingForFusionVariants;
        let _spawning_judge = SessionState::SpawningJudge;
        let _judging = SessionState::Judging;
        let _awaiting_verdict = SessionState::AwaitingVerdictSelection;
        let _merging_winner = SessionState::MergingWinner;
        let _running = SessionState::Running;
        let _paused = SessionState::Paused;
        let _completed = SessionState::Completed;
        let _failed = SessionState::Failed("error".to_string());
    }

    #[test]
    fn session_state_serialization() {
        let state = SessionState::SpawningWorker(3);
        let json = serde_json::to_string(&state).expect("serialize SessionState");
        assert!(json.contains("SpawningWorker"));
    }
}


