use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::cli::{CliRegistry, CliBehavior};
use crate::coordination::{HierarchyNode, StateManager, WorkerStateInfo};
use crate::events::{EventBus, EventEmitter};
use crate::pty::{AgentRole, AgentStatus, AgentConfig, PtyManager, WorkerRole};
use crate::storage::{SessionStorage, StorageError};
use crate::session::cell_status::{
    agent_in_cell, derive_cell_status_name, derive_cell_status_name_for_state, session_cell_ids, variant_to_cell_id,
    PRIMARY_CELL_ID, RESOLVER_CELL_ID,
};
use crate::templates::{PromptContext, TemplateEngine};
use crate::watcher::TaskFileWatcher;
use crate::artifacts::collector::ArtifactCollector;
use crate::domain::ArtifactBundle;
use crate::workspace::git::{
    cleanup_session_worktrees, create_session_worktree, remove_session_worktree_cell,
    current_head, resolve_fresh_base,
};

/// Example `coordination.log` lines for Queen quality-reconciliation (quiescence-based; no iteration cap).
const QUEEN_QUALITY_RECONCILIATION_LOG_LINES: &str = r#"[TIMESTAMP] QUEEN: Entering reconciliation loop for latest push
[TIMESTAMP] QUEEN: Collected N evaluator findings, M external comments since latest push
[TIMESTAMP] QUEEN: Spawned Reconciler — awaiting unified fix list
[TIMESTAMP] QUEEN: Reconciliation complete — N fixes assigned
[TIMESTAMP] QUEEN: Quality loop complete - session marked completed"#;

const QUEEN_QUALITY_RECONCILIATION_LOG_LINES_NO_EVALUATOR: &str = r#"[TIMESTAMP] QUEEN: Entering reconciliation loop for latest push
[TIMESTAMP] QUEEN: Collected N external comments and integrity findings since latest push
[TIMESTAMP] QUEEN: Spawned Reconciler — awaiting unified fix list
[TIMESTAMP] QUEEN: Reconciliation complete — N fixes assigned
[TIMESTAMP] QUEEN: Quality loop complete - session marked completed"#;

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

pub const DEFAULT_MAX_QA_ITERATIONS: u8 = 20;
const DEFAULT_QA_TIMEOUT_SECS: u64 = 300;
const MAX_PRIMARY_CELL_BRANCHES: usize = 4;
const MAX_PRIMARY_CELL_DIFF_SUMMARY_LEN: usize = 4_096;

/// Authentication strategy for QA workers accessing the session
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthStrategy {
    /// No authentication required (default for backward compat)
    None,
    /// Dev bypass with a per-session token (localhost only)
    DevBypass { token: String },
}

impl Default for AuthStrategy {
    fn default() -> Self {
        AuthStrategy::None
    }
}

impl AuthStrategy {
    fn dev_bypass() -> Self {
        Self::DevBypass {
            token: Uuid::new_v4().to_string(),
        }
    }

    fn from_persisted(value: &str) -> Self {
        match value {
            "" | "None" => Self::None,
            token if token.starts_with("DevBypass:") => Self::DevBypass {
                token: token.trim_start_matches("DevBypass:").to_string(),
            },
            _ => Self::None,
        }
    }

    fn persist_value(&self) -> String {
        match self {
            Self::None => String::new(),
            Self::DevBypass { token } => format!("DevBypass:{}", token),
        }
    }

    fn apply_prompt_variables(
        &self,
        session_id: &str,
        variables: &mut HashMap<String, String>,
    ) {
        match self {
            Self::DevBypass { token } => {
                variables.insert(
                    "auth_bypass_url".to_string(),
                    format!(
                        "http://localhost:18800/api/sessions/{}/auth/dev-login?token={}",
                        session_id, token
                    ),
                );
                variables.insert("auth_bypass_token".to_string(), token.clone());
            }
            Self::None => {
                variables.insert("auth_bypass_url".to_string(), "(not configured)".to_string());
                variables.insert("auth_bypass_token".to_string(), String::new());
            }
        }
    }
}

/// Structured error returned when session completion is blocked.
/// Used to enrich 409 response body with actionable unblock paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionBlockedError {
    pub error: String,
    pub current_state: String,
    pub unblock_paths: Vec<String>,
    pub remaining_quiescence_seconds: Option<i64>,
}

impl CompletionBlockedError {
    /// Create error for state mismatch (evaluator-backed session not in QaPassed).
    pub fn state_blocked(_session_id: &str, current_state: &SessionState, requires_evaluator: bool) -> Self {
        let (error, unblock_paths) = if requires_evaluator {
            (
                "Session completion blocked: evaluator-backed session must be in QaPassed state".to_string(),
                vec![
                    format!("POST /api/sessions/{{{{id}}}}/qa/verdict with {{\"verdict\":\"PASS\"}} (Evaluator submits PASS)"),
                    format!("POST /api/sessions/{{{{id}}}}/qa/force-pass (Operator override)"),
                ],
            )
        } else {
            (
                "Session completion blocked: session must be in Running or QaPassed state".to_string(),
                vec![],
            )
        };
        Self {
            error,
            current_state: format!("{:?}", current_state),
            unblock_paths,
            remaining_quiescence_seconds: None,
        }
    }

    /// Create error for quiescence period not yet elapsed.
    pub fn quiescence_blocked(remaining_seconds: i64) -> Self {
        Self {
            error: format!(
                "Session completion blocked: session must be quiescent for 10 minutes ({}s remaining)",
                remaining_seconds
            ),
            current_state: "quiescence_required".to_string(),
            unblock_paths: vec![format!("Wait {} more seconds before retrying", remaining_seconds)],
            remaining_quiescence_seconds: Some(remaining_seconds),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CompletionError {
    Blocked(CompletionBlockedError),
    NotFound(String),
    Storage(String),
}

impl CompletionError {
    fn not_found(session_id: &str) -> Self {
        Self::NotFound(format!("Session not found: {}", session_id))
    }

    fn storage(message: impl Into<String>) -> Self {
        Self::Storage(message.into())
    }
}

fn default_max_qa_iterations() -> u8 {
    DEFAULT_MAX_QA_ITERATIONS
}

fn default_qa_timeout_secs() -> u64 {
    DEFAULT_QA_TIMEOUT_SECS
}

fn default_session_qa_settings() -> (u8, u64, AuthStrategy) {
    (
        default_max_qa_iterations(),
        default_qa_timeout_secs(),
        AuthStrategy::dev_bypass(),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionState {
    Planning,
    PlanReady,
    Starting,
    SpawningWorker(u8),
    WaitingForWorker(u8),
    SpawningPlanner(u8),
    WaitingForPlanner(u8),
    SpawningFusionVariant(u8),
    WaitingForFusionVariants,
    SpawningJudge,
    Judging,
    AwaitingVerdictSelection,
    MergingWinner,
    SpawningEvaluator,
    QaInProgress { iteration: Option<u8> },
    QaPassed,
    QaFailed { iteration: u8 },
    QaMaxRetriesExceeded,
    Running,
    Paused,
    Completed,
    Closing,
    Closed,
    Failed(String),
}

impl SessionState {
    pub fn is_monitorable(&self) -> bool {
        matches!(
            self,
            SessionState::Running
                | SessionState::WaitingForWorker(_)
                | SessionState::WaitingForPlanner(_)
                | SessionState::SpawningEvaluator
                | SessionState::QaInProgress { .. }
                | SessionState::QaPassed
                | SessionState::QaFailed { .. }
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub role: AgentRole,
    pub status: AgentStatus,
    pub config: AgentConfig,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub base_commit_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiveLaunchConfig {
    pub project_path: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    pub queen_config: AgentConfig,
    pub workers: Vec<AgentConfig>,
    pub prompt: Option<String>,
    #[serde(default)]
    pub with_planning: bool,  // If true, spawn Master Planner first
    #[serde(default)]
    pub with_evaluator: bool,
    #[serde(default)]
    pub evaluator_config: Option<AgentConfig>,
    #[serde(default)]
    pub qa_workers: Option<Vec<QaWorkerConfig>>,
    #[serde(default)]
    pub smoke_test: bool,     // If true, create a minimal test plan without real investigation
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmLaunchConfig {
    pub project_path: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    pub queen_config: AgentConfig,
    pub planner_count: u8,                    // How many planners
    pub planner_config: AgentConfig,          // Config shared by all planners
    pub workers_per_planner: Vec<AgentConfig>, // Workers shared config (applied to each planner)
    pub prompt: Option<String>,
    #[serde(default)]
    pub with_planning: bool,  // If true, spawn Master Planner first
    #[serde(default)]
    pub with_evaluator: bool,
    #[serde(default)]
    pub evaluator_config: Option<AgentConfig>,
    #[serde(default)]
    pub qa_workers: Option<Vec<QaWorkerConfig>>,
    #[serde(default)]
    pub smoke_test: bool,     // If true, create a minimal test plan without real investigation

    // Legacy support - if planners vec is provided, use it instead
    #[serde(default)]
    pub planners: Vec<PlannerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub struct QaWorkerConfig {
    pub specialization: String,
    pub cli: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub flags: Option<Vec<String>>,
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
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
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
    pub name: Option<String>,
    pub color: Option<String>,
    pub session_type: SessionType,
    pub project_path: PathBuf,
    pub state: SessionState,
    pub created_at: DateTime<Utc>,
    /// Latest meaningful activity (state persistence, heartbeats, etc.) for dashboards.
    pub last_activity_at: DateTime<Utc>,
    pub agents: Vec<AgentInfo>,
    pub default_cli: String,
    pub default_model: Option<String>,
    #[serde(default)]
    pub qa_workers: Vec<QaWorkerConfig>,
    pub max_qa_iterations: u8,
    pub qa_timeout_secs: u64,
    #[serde(default)]
    pub auth_strategy: AuthStrategy,
    /// Primary git worktree path for this session (e.g. Queen or first Fusion variant), for UI.
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub worktree_branch: Option<String>,
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
    event_emitter: Option<EventEmitter>,
    storage: Option<Arc<SessionStorage>>,
    task_watchers: Mutex<HashMap<String, TaskFileWatcher>>,
    /// session_id -> agent_id -> heartbeat info
    agent_heartbeats: Arc<RwLock<HashMap<String, HashMap<String, AgentHeartbeatInfo>>>>,
    /// QA timeout cancel handles: session_id -> abort handle
    qa_timeout_handles: Mutex<HashMap<String, tokio::task::AbortHandle>>,
    evaluator_respawns_inflight: Mutex<HashSet<String>>,
}

// Explicitly implement Send + Sync
unsafe impl Send for SessionController {}
unsafe impl Sync for SessionController {}

fn is_terminal_session_state(state: &SessionState) -> bool {
    matches!(
        state,
        SessionState::QaMaxRetriesExceeded
            | SessionState::Completed
            | SessionState::Closed
            | SessionState::Failed(_)
    )
}

fn qa_in_progress_state(state: &SessionState) -> SessionState {
    match state {
        SessionState::QaFailed { iteration } => SessionState::QaInProgress {
            iteration: Some(*iteration),
        },
        SessionState::QaInProgress { iteration } => SessionState::QaInProgress {
            iteration: *iteration,
        },
        _ => SessionState::QaInProgress { iteration: None },
    }
}

fn next_qa_failure_iteration(state: &SessionState) -> u8 {
    match state {
        SessionState::QaFailed { iteration } => iteration.saturating_add(1),
        SessionState::QaInProgress { iteration } => iteration.unwrap_or(0).saturating_add(1),
        _ => 1,
    }
}

fn cell_type_for_id(cell_id: &str) -> &'static str {
    if cell_id == RESOLVER_CELL_ID {
        "resolver"
    } else {
        "hive"
    }
}

fn agent_cell_id(session: &Session, agent: &AgentInfo) -> String {
    match &session.session_type {
        SessionType::Fusion { .. } => match &agent.role {
            AgentRole::Fusion { variant } => variant_to_cell_id(variant),
            _ => RESOLVER_CELL_ID.to_string(),
        },
        _ => PRIMARY_CELL_ID.to_string(),
    }
}

fn cell_status_changes_for_transition(
    session: &Session,
    new_state: &SessionState,
) -> Vec<(String, String, String)> {
    let cell_ids = session_cell_ids(session);
    let before = cell_ids
        .iter()
        .map(|cell_id| (cell_id.clone(), derive_cell_status_name(session, cell_id)))
        .collect::<HashMap<_, _>>();

    let mut changes = Vec::new();
    for cell_id in cell_ids {
        let next_status = derive_cell_status_name_for_state(session, &cell_id, new_state);
        if let Some(previous_status) = before.get(&cell_id) {
            if previous_status != &next_status {
                changes.push((cell_id, previous_status.clone(), next_status));
            }
        }
    }

    changes
}
/// Generate CLI-specific polling instructions based on the CLI's behavioral profile
fn get_polling_instructions(cli: &str, task_file: &str, role_type: Option<&str>) -> String {
    match CliRegistry::get_behavior_for_role(cli, role_type) {
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
            event_emitter: None,
            storage: None,
            task_watchers: Mutex::new(HashMap::new()),
            agent_heartbeats: Arc::new(RwLock::new(HashMap::new())),
            qa_timeout_handles: Mutex::new(HashMap::new()),
            evaluator_respawns_inflight: Mutex::new(HashSet::new()),
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

    pub fn set_event_bus(&mut self, event_bus: Arc<EventBus>) {
        self.event_emitter = Some(EventEmitter::new(event_bus));
    }

    pub fn launch_hive(
        &self,
        project_path: PathBuf,
        worker_count: u8,
        command: &str,
        prompt: Option<String>,
        name: Option<String>,
        color: Option<String>,
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
                name: None,
                description: None,
                role: None,
                initial_prompt: None,
            };

            agents.push(AgentInfo {
                id: queen_id,
                role: AgentRole::Queen,
                status: AgentStatus::Running,
                config: queen_config,
                parent_id: None,
                commit_sha: None,
                base_commit_sha: None,
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
                    name: None,
                    description: None,
                    role: None,
                    initial_prompt: None,
                };

                agents.push(AgentInfo {
                    id: worker_id.clone(),
                    role: AgentRole::Worker { index: i, parent: Some(format!("{}-queen", session_id)) },
                    status: AgentStatus::Running,
                    config: worker_config,
                    parent_id: Some(format!("{}-queen", session_id)),
                    commit_sha: None,
                    base_commit_sha: None,
                });
            }
        }

        let (max_qa_iterations, qa_timeout_secs, auth_strategy) = default_session_qa_settings();
        let session = Session {
            id: session_id.clone(),
            name,
            color,
            session_type: SessionType::Hive { worker_count },
            project_path,
            state: SessionState::Running,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli: cmd.to_string(),
            default_model: if cmd == "claude" { Some("opus-4-6".to_string()) } else { None },
            qa_workers: Vec::new(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: None,
            worktree_branch: None,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        self.emit_agent_batch_launched(&session, &session.agents);

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        self.init_session_storage(&session);
        Ok(session)
    }

    pub fn get_session(&self, id: &str) -> Option<Session> {
        self.refresh_session_from_storage_if_clean(id);
        let sessions = self.sessions.read();
        sessions.get(id).cloned()
    }

    fn refresh_session_from_storage_if_clean(&self, session_id: &str) {
        let Some(storage) = self.storage.as_ref() else {
            return;
        };

        let needs_refresh = match storage.has_newer_session_file(session_id) {
            Ok(needs_refresh) => needs_refresh,
            Err(err) => {
                tracing::warn!(
                    "Failed to check session.json freshness for {}: {}",
                    session_id,
                    err
                );
                return;
            }
        };
        if !needs_refresh {
            return;
        }

        let current_session = {
            let sessions = self.sessions.read();
            sessions.get(session_id).cloned()
        };
        let Some(current_session) = current_session else {
            return;
        };

        let current_persisted = Self::session_to_persisted_snapshot(&current_session);
        let current_hash = SessionStorage::session_content_hash(&current_persisted);
        let refresh = match storage.load_session_if_newer_and_clean(session_id, current_hash) {
            Ok(Some(refresh)) => refresh,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(
                    "Failed to check session.json freshness for {}: {}",
                    session_id,
                    err
                );
                return;
            }
        };

        let refreshed = match self.session_from_persisted(&refresh.persisted) {
            Ok(session) => session,
            Err(err) => {
                tracing::warn!(
                    "Failed to hot-reload session {} from session.json: {}",
                    session_id,
                    err
                );
                return;
            }
        };

        let mut sessions = self.sessions.write();
        let Some(current_session) = sessions.get(session_id) else {
            return;
        };
        let current_hash = SessionStorage::session_content_hash(
            &Self::session_to_persisted_snapshot(current_session),
        );
        let still_clean =
            match storage.should_apply_session_refresh(session_id, &refresh, current_hash) {
                Ok(still_clean) => still_clean,
                Err(err) => {
                    tracing::warn!(
                        "Failed to re-check session.json freshness for {}: {}",
                        session_id,
                        err
                    );
                    return;
                }
            };
        if !still_clean {
            return;
        }

        // session_from_persisted reconstructs every agent with AgentStatus::Completed
        // because persisted snapshots don't carry runtime status. Overlay the current
        // in-memory statuses onto the refreshed agents so a clean disk refresh doesn't
        // mark live Queen/Worker/Evaluator PTYs as completed.
        let mut refreshed = refreshed;
        let current_statuses: std::collections::HashMap<String, AgentStatus> = current_session
            .agents
            .iter()
            .map(|a| (a.id.clone(), a.status.clone()))
            .collect();
        for agent in refreshed.agents.iter_mut() {
            if let Some(status) = current_statuses.get(&agent.id) {
                agent.status = status.clone();
            }
        }

        sessions.insert(session_id.to_string(), refreshed);
        drop(sessions);

        if let Err(err) = storage.mark_session_synced(session_id, &refresh.persisted) {
            tracing::warn!(
                "Failed to track refreshed session.json state for {}: {}",
                session_id,
                err
            );
        }
    }

    pub fn update_session_metadata(
        &self,
        session_id: &str,
        name: Option<Option<String>>,
        color: Option<Option<String>>,
    ) -> Result<Session, String> {
        let had_in_memory = {
            let mut sessions = self.sessions.write();
            sessions
                .get_mut(session_id)
                .map(|session| {
                    if let Some(name) = name.clone() {
                        session.name = name;
                    }
                    if let Some(color) = color.clone() {
                        session.color = color;
                    }
                })
                .is_some()
        };

        let updated = if had_in_memory {
            self.update_session_storage_checked(session_id)?;
            self.get_session(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?
        } else {
            let storage = self
                .storage
                .as_ref()
                .ok_or_else(|| format!("Session not found: {}", session_id))?;
            let mut persisted = storage
                .load_session(session_id)
                .map_err(|_| format!("Session not found: {}", session_id))?;

            if let Some(name) = name {
                persisted.name = name;
            }
            if let Some(color) = color {
                persisted.color = color;
            }
            persisted.last_activity_at = Some(Utc::now());

            storage
                .save_session(&persisted)
                .map_err(|e| format!("Failed to save session metadata: {}", e))?;

            let session = self.session_from_persisted(&persisted)?;
            {
                let mut sessions = self.sessions.write();
                sessions.insert(session.id.clone(), session.clone());
            }
            session
        };

        self.emit_session_update(session_id);
        Ok(updated)
    }

    /// Get the default CLI for a session
    pub fn get_session_default_cli(&self, session_id: &str) -> Option<String> {
        let sessions = self.sessions.read();
        sessions.get(session_id).map(|s| s.default_cli.clone())
    }

    pub fn list_sessions(&self) -> Vec<Session> {
        let sessions = self.sessions.read();
        let heartbeats = self.agent_heartbeats.read();
        sessions
            .values()
            .cloned()
            .map(|mut session| {
                if let Some(map) = heartbeats.get(&session.id) {
                    if let Some(max_hb) = map.values().map(|h| h.last_activity).max() {
                        if max_hb > session.last_activity_at {
                            session.last_activity_at = max_hb;
                        }
                    }
                }
                session
            })
            .collect()
    }

    fn session_requires_internal_evaluator(session: &Session) -> bool {
        session.agents.iter().any(|agent| {
            matches!(
                agent.role,
                AgentRole::Evaluator | AgentRole::QaWorker { .. }
            )
        })
    }

    fn state_allows_completion(session: &Session) -> bool {
        if Self::session_requires_internal_evaluator(session) {
            matches!(session.state, SessionState::QaPassed)
        } else {
            matches!(session.state, SessionState::Running | SessionState::QaPassed)
        }
    }

    pub fn can_complete_session(&self, session_id: &str) -> Result<(), CompletionError> {
        let session = if let Some(session) = self.get_session(session_id) {
            session
        } else {
            let storage = self
                .storage
                .as_ref()
                .ok_or_else(|| CompletionError::not_found(session_id))?;
            let persisted = storage
                .load_session(session_id)
                .map_err(|err| match err {
                    StorageError::SessionNotFound(_) => CompletionError::not_found(session_id),
                    _ => CompletionError::storage(format!("Storage error: {}", err)),
                })?;
            self.session_from_persisted(&persisted)
                .map_err(CompletionError::storage)?
        };

        if !Self::state_allows_completion(&session) {
            return Err(CompletionError::Blocked(CompletionBlockedError::state_blocked(
                session_id,
                &session.state,
                Self::session_requires_internal_evaluator(&session),
            )));
        }

        let quiet_for = Utc::now() - session.last_activity_at;
        if quiet_for < chrono::Duration::minutes(10) {
            let remaining = (chrono::Duration::minutes(10) - quiet_for)
                .num_seconds()
                .max(0);
            return Err(CompletionError::Blocked(
                CompletionBlockedError::quiescence_blocked(remaining),
            ));
        }

        // External reviewer comments are not tracked server-side yet, so the completion gate enforces
        // the internal quiescence conditions we can prove here: evaluator-aware state + 10-minute quiet period.
        Ok(())
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
        let session_snapshot = {
            let mut sessions = self.sessions.write();
            sessions.get_mut(session_id).map(|session| {
                if now > session.last_activity_at {
                    session.last_activity_at = now;
                }
                session.clone()
            })
        };

        if let (Some(storage), Some(session)) = (self.storage.as_ref(), session_snapshot.as_ref()) {
            Self::persist_session_snapshot(storage, session, session_id)?;
        }
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

    fn emit_cell_created(&self, session_id: &str, cell_id: &str) {
        let Some(emitter) = self.event_emitter.clone() else {
            return;
        };
        let session_id = session_id.to_string();
        let cell_id = cell_id.to_string();
        let cell_type = cell_type_for_id(&cell_id).to_string();
        tokio::spawn(async move {
            if let Err(error) = emitter.emit_cell_created(&session_id, &cell_id, &cell_type).await {
                tracing::debug!("Failed to emit cell created event: {}", error);
            }
        });
    }

    fn emit_agent_launched(&self, session: &Session, agent: &AgentInfo) {
        let Some(emitter) = self.event_emitter.clone() else {
            return;
        };
        let session_id = session.id.clone();
        let cell_id = agent_cell_id(session, agent);
        let agent_id = agent.id.clone();
        let cli = agent.config.cli.clone();
        tokio::spawn(async move {
            if let Err(error) = emitter
                .emit_agent_launched(&session_id, &cell_id, &agent_id, &cli)
                .await
            {
                tracing::debug!("Failed to emit agent launched event: {}", error);
            }
        });
    }

    fn merge_primary_cell_artifact_bundles(existing: ArtifactBundle, incoming: ArtifactBundle) -> ArtifactBundle {
        let mut commits = existing.commits.clone();
        for c in incoming.commits {
            if !commits.iter().any(|x| x == &c) {
                commits.push(c);
            }
        }
        let mut changed_files = existing.changed_files.clone();
        for f in incoming.changed_files {
            if !changed_files.iter().any(|x| x == &f) {
                changed_files.push(f);
            }
        }
        let branch = Self::merge_primary_cell_branch_labels([existing.branch.clone(), incoming.branch.clone()]);
        let summary = match (existing.summary, incoming.summary) {
            (Some(a), Some(b)) if a != b => Some(format!("{} · {}", a, b)),
            (Some(a), _) => Some(a),
            (_, Some(b)) => Some(b),
            _ => None,
        };
        let test_results = incoming.test_results.or(existing.test_results);
        let diff_summary =
            Self::merge_primary_cell_diff_summaries(existing.diff_summary, incoming.diff_summary);
        let mut unresolved_issues = existing.unresolved_issues;
        unresolved_issues.extend(incoming.unresolved_issues);
        let confidence = match (existing.confidence, incoming.confidence) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            _ => None,
        };
        let recommended_next_step = incoming
            .recommended_next_step
            .or(existing.recommended_next_step);
        ArtifactBundle {
            summary,
            changed_files,
            commits,
            branch,
            test_results,
            diff_summary,
            unresolved_issues,
            confidence,
            recommended_next_step,
        }
    }

    fn merge_primary_cell_branch_labels(branches: [String; 2]) -> String {
        let unique = branches
            .into_iter()
            .filter_map(|branch| {
                let trimmed = branch.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
            .fold(Vec::new(), |mut acc, branch| {
                if !acc.contains(&branch) {
                    acc.push(branch);
                }
                acc
            });

        match unique.len() {
            0 => String::new(),
            1 => unique.into_iter().next().unwrap_or_default(),
            len if len > MAX_PRIMARY_CELL_BRANCHES => {
                let mut limited = unique.into_iter().take(MAX_PRIMARY_CELL_BRANCHES).collect::<Vec<_>>();
                limited.push(format!("+{} more", len - MAX_PRIMARY_CELL_BRANCHES));
                limited.join(" | ")
            }
            _ => unique.join(" | "),
        }
    }

    fn merge_primary_cell_diff_summaries(
        existing: Option<String>,
        incoming: Option<String>,
    ) -> Option<String> {
        let mut unique = Vec::new();
        for summary in [existing, incoming].into_iter().flatten() {
            let trimmed = summary.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !unique.iter().any(|value: &String| value == trimmed) {
                unique.push(trimmed.to_string());
            }
        }

        if unique.is_empty() {
            return None;
        }

        let merged = unique.join("\n---\n");
        if merged.chars().count() <= MAX_PRIMARY_CELL_DIFF_SUMMARY_LEN {
            return Some(merged);
        }

        let truncated = merged
            .chars()
            .take(MAX_PRIMARY_CELL_DIFF_SUMMARY_LEN.saturating_sub(16))
            .collect::<String>();
        Some(format!("{truncated}\n...[truncated]"))
    }

    fn agent_git_worktree_path_for_artifacts(session: &Session, agent: &AgentInfo) -> Option<PathBuf> {
        match &agent.role {
            AgentRole::Fusion { variant } => {
                Self::read_fusion_metadata(&session.project_path, &session.id)
                    .ok()
                    .and_then(|meta| {
                        meta.variants
                            .iter()
                            .find(|v| &v.name == variant || v.agent_id == agent.id)
                            .map(|v| PathBuf::from(&v.worktree_path))
                    })
            }
            AgentRole::Queen => Some(
                session
                    .project_path
                    .join(".hive-manager")
                    .join("worktrees")
                    .join(&session.id)
                    .join("queen"),
            ),
            AgentRole::Worker { index, .. } => Some(
                session
                    .project_path
                    .join(".hive-manager")
                    .join("worktrees")
                    .join(&session.id)
                    .join(format!("worker-{index}")),
            ),
            _ => None,
        }
    }

    fn harvest_completion_artifacts(&self, session: &Session, agent: &AgentInfo) {
        let Some(storage) = self.storage.as_ref() else {
            return;
        };
        let Some(wt) = Self::agent_git_worktree_path_for_artifacts(session, agent) else {
            return;
        };
        if !wt.exists() {
            return;
        }
        let bundle = match ArtifactCollector::collect_from_worktree(&wt) {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!(
                    "Artifact harvest failed for agent {} in {}: {}",
                    agent.id,
                    wt.display(),
                    err
                );
                return;
            }
        };
        let cell_id = agent_cell_id(session, agent);
        let session_id = session.id.as_str();
        if cell_id == PRIMARY_CELL_ID {
            let incoming_bundle = bundle;
            if let Err(err) = storage.atomic_update_artifact(session_id, &cell_id, move |existing| {
                existing.map_or(incoming_bundle.clone(), |existing_bundle| {
                    Self::merge_primary_cell_artifact_bundles(existing_bundle, incoming_bundle)
                })
            }) {
                tracing::warn!(
                    "Failed to persist artifacts for session {} cell {}: {}",
                    session_id,
                    cell_id,
                    err
                );
                return;
            }
        } else {
            if let Err(err) = storage.save_artifact(session_id, &cell_id, &bundle) {
                tracing::warn!(
                    "Failed to persist artifacts for session {} cell {}: {}",
                    session_id,
                    cell_id,
                    err
                );
                return;
            }
        }
        self.emit_artifact_updated_for_cell(session_id, &cell_id, Some(agent.id.as_str()));
    }

    fn emit_agent_completed(&self, session: &Session, agent: &AgentInfo) {
        self.harvest_completion_artifacts(session, agent);
        let Some(emitter) = self.event_emitter.clone() else {
            return;
        };
        let session_id = session.id.clone();
        let cell_id = agent_cell_id(session, agent);
        let agent_id = agent.id.clone();
        tokio::spawn(async move {
            if let Err(error) = emitter
                .emit_agent_completed(&session_id, &cell_id, &agent_id)
                .await
            {
                tracing::debug!("Failed to emit agent completed event: {}", error);
            }
        });
    }

    fn emit_workspace_created(
        &self,
        session_id: &str,
        cell_id: &str,
        branch: &str,
        worktree_path: Option<&str>,
    ) {
        let Some(emitter) = self.event_emitter.clone() else {
            return;
        };
        let session_id = session_id.to_string();
        let cell_id = cell_id.to_string();
        let branch = branch.to_string();
        let worktree_path = worktree_path.map(str::to_string);
        tokio::spawn(async move {
            if let Err(error) = emitter
                .emit_workspace_created(&session_id, &cell_id, &branch, worktree_path.as_deref())
                .await
            {
                tracing::debug!("Failed to emit workspace created event: {}", error);
            }
        });
    }

    pub fn emit_artifact_updated_for_cell(
        &self,
        session_id: &str,
        cell_id: &str,
        agent_id: Option<&str>,
    ) {
        let Some(storage) = self.storage.as_ref() else {
            return;
        };
        let Some(emitter) = self.event_emitter.clone() else {
            return;
        };

        let resolved_agent_id = agent_id
            .map(str::to_string)
            .or_else(|| {
                self.get_session(session_id).and_then(|session| {
                    session
                        .agents
                        .iter()
                        .find(|agent| agent_in_cell(&session, cell_id, agent))
                        .map(|agent| agent.id.clone())
                })
            })
            .unwrap_or_else(|| cell_id.to_string());
        let artifact_path = storage
            .session_dir(session_id)
            .join("artifacts")
            .join(format!("{}.json", cell_id))
            .to_string_lossy()
            .to_string();
        let session_id = session_id.to_string();
        let cell_id = cell_id.to_string();

        tokio::spawn(async move {
            if let Err(error) = emitter
                .emit_artifact_updated(&session_id, &cell_id, &resolved_agent_id, &artifact_path)
                .await
            {
                tracing::debug!("Failed to emit artifact updated event: {}", error);
            }
        });
    }

    fn emit_agent_batch_launched(&self, session: &Session, agents: &[AgentInfo]) {
        let mut emitted_cells = HashMap::<String, bool>::new();
        for agent in agents {
            let cell_id = agent_cell_id(session, agent);
            if !emitted_cells.contains_key(&cell_id) {
                self.emit_cell_created(&session.id, &cell_id);
                emitted_cells.insert(cell_id, true);
            }
            self.emit_agent_launched(session, agent);
        }
    }

    fn fire_cell_status_changes(
        emitter: EventEmitter,
        session_id: String,
        changes: Vec<(String, String, String)>,
    ) {
        tokio::spawn(async move {
            for (cell_id, from, to) in changes {
                if let Err(error) = emitter
                    .emit_cell_status_changed(&session_id, &cell_id, &from, &to)
                    .await
                {
                    tracing::debug!("Failed to emit cell status change event: {}", error);
                }
            }
        });
    }

    fn emit_cell_status_changes(&self, session_id: &str, changes: Vec<(String, String, String)>) {
        let Some(emitter) = self.event_emitter.clone() else {
            return;
        };
        Self::fire_cell_status_changes(emitter, session_id.to_string(), changes);
    }

    fn set_session_state_with_events(
        &self,
        session: &mut Session,
        new_state: SessionState,
    ) -> Vec<(String, String, String)> {
        let changes = cell_status_changes_for_transition(session, &new_state);
        session.state = new_state;
        changes
    }

    fn persist_then_emit_session_update(
        &self,
        session_id: &str,
        changes: Vec<(String, String, String)>,
    ) -> Result<(), String> {
        self.update_session_storage_checked(session_id)?;
        self.emit_cell_status_changes(session_id, changes);
        self.emit_session_update(session_id);
        Ok(())
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

            let previous_state = {
                let mut sessions = self.sessions.write();
                sessions.get_mut(id).map(|s| {
                    let previous_state = (s.state.clone(), s.auth_strategy.clone());
                    let changes = self.set_session_state_with_events(s, SessionState::Completed);
                    s.auth_strategy = AuthStrategy::None;
                    (previous_state, changes)
                })
            };

            if let Some(((previous_session_state, previous_auth_strategy), changes)) = previous_state {
                if let Err(err) = self.persist_then_emit_session_update(id, changes) {
                    let mut sessions = self.sessions.write();
                    if let Some(session) = sessions.get_mut(id) {
                        session.state = previous_session_state;
                        session.auth_strategy = previous_auth_strategy;
                    }
                    return Err(err);
                }
            }

            Ok(())
        } else {
            Err(format!("Session not found: {}", id))
        }
    }

    pub fn mark_session_completed(&self, session_id: &str) -> Result<(), CompletionError> {
        self.can_complete_session(session_id)?;

        let previous_state = {
            let mut sessions = self.sessions.write();
            sessions.get_mut(session_id).map(|session| {
                let previous_state = (session.state.clone(), session.auth_strategy.clone());
                let changes = self.set_session_state_with_events(session, SessionState::Completed);
                session.auth_strategy = AuthStrategy::None;
                (previous_state, changes)
            })
        };

        if let Some(((previous_session_state, previous_auth_strategy), changes)) = previous_state {
                if let Err(err) = self.update_session_storage_checked(session_id) {
                    let mut sessions = self.sessions.write();
                    if let Some(session) = sessions.get_mut(session_id) {
                        session.state = previous_session_state;
                        session.auth_strategy = previous_auth_strategy;
                    }
                    return Err(CompletionError::storage(err));
                }

            self.emit_cell_status_changes(session_id, changes);
            self.emit_session_update(session_id);
            return Ok(());
        }

        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| CompletionError::not_found(session_id))?;
        let mut persisted = storage
            .load_session(session_id)
            .map_err(|err| match err {
                StorageError::SessionNotFound(_) => CompletionError::not_found(session_id),
                _ => CompletionError::storage(format!("Storage error: {}", err)),
            })?;
        persisted.state = serialize_session_state(&SessionState::Completed);
        persisted.auth_strategy = AuthStrategy::None.persist_value();
        storage
            .save_session(&persisted)
            .map_err(|e| CompletionError::storage(format!(
                "Failed to persist session completion: {}",
                e
            )))?;

        Ok(())
    }

    pub fn close_session(&self, id: &str) -> Result<(), String> {
        let (agent_ids, cleanup_session): (Vec<String>, Session) = {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(id) {
                let changes = self.set_session_state_with_events(session, SessionState::Closing);
                self.emit_cell_status_changes(id, changes);
                (
                    session.agents.iter().map(|a| a.id.clone()).collect(),
                    session.clone(),
                )
            } else {
                return Err(format!("Session not found: {}", id));
            }
        };

        let kill_errors: Vec<String> = {
            let pty_manager = self.pty_manager.read();
            let mut errors = Vec::new();
            for agent_id in &agent_ids {
                if let Err(e) = pty_manager.kill(agent_id) {
                    errors.push(format!("{}: {}", agent_id, e));
                }
            }
            errors
        };

        {
            let mut watchers = self.task_watchers.lock();
            let _ = watchers.remove(id);
        }

        {
            let mut heartbeats = self.agent_heartbeats.write();
            heartbeats.remove(id);
        }

        if let Err(err) = cleanup_session_worktrees(&cleanup_session) {
            tracing::warn!("Session {} cleanup had issues: {}", id, err);
        }

        let closed_state = {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(id) {
                let completed_agents = session
                    .agents
                    .iter()
                    .filter(|agent| agent.status != AgentStatus::Completed)
                    .cloned()
                    .collect::<Vec<_>>();
                for agent in &mut session.agents {
                    agent.status = AgentStatus::Completed;
                }
                let changes = self.set_session_state_with_events(session, SessionState::Closed);
                session.auth_strategy = AuthStrategy::None;
                session.worktree_path = None;
                session.worktree_branch = None;
                Some((session.clone(), completed_agents, changes))
            } else {
                None
            }
        };

        self.update_session_storage(id);
        if let Some((session, completed_agents, changes)) = closed_state {
            for agent in &completed_agents {
                self.emit_agent_completed(&session, agent);
            }
            self.emit_cell_status_changes(id, changes);
        }
        self.emit_session_update(id);
        if !kill_errors.is_empty() {
            tracing::warn!("Session {} closed with PTY kill errors: {}", id, kill_errors.join(" | "));
        }
        Ok(())
    }

    fn rollback_launch_allocations(
        &self,
        project_path: &PathBuf,
        session_id: &str,
        created_cells: &[(String, String)],
        spawned_agent_ids: &[String],
    ) {
        let mut seen_agent_ids = HashSet::new();
        {
            let pty_manager = self.pty_manager.read();
            for agent_id in spawned_agent_ids.iter().rev() {
                if !seen_agent_ids.insert(agent_id.clone()) {
                    continue;
                }
                if let Err(err) = pty_manager.kill(agent_id) {
                    tracing::warn!("Launch rollback failed to kill agent {}: {}", agent_id, err);
                }
            }
        }

        let mut seen_cells = HashSet::new();
        for (cell_id, branch_name) in created_cells.iter().rev() {
            if !seen_cells.insert(cell_id.clone()) {
                continue;
            }
            if let Err(err) = remove_session_worktree_cell(project_path, session_id, cell_id) {
                tracing::warn!(
                    "Launch rollback failed to remove worktree for session {} cell {}: {}",
                    session_id,
                    cell_id,
                    err
                );
            } else {
                Self::delete_branch(project_path, branch_name);
            }
        }
    }

    fn remove_worker_launch_file(session_id: &str, worker_cell_name: &str, file_path: &Path) {
        if let Err(err) = std::fs::remove_file(file_path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    "Worker launch rollback failed to remove file {} for session {} cell {}: {}",
                    file_path.display(),
                    session_id,
                    worker_cell_name,
                    err
                );
            }
        }
    }

    fn rollback_worker_launch_artifacts(
        project_path: &Path,
        session_id: &str,
        worker_cell_name: &str,
        task_file_path: &Path,
        prompt_file_path: Option<&Path>,
    ) {
        if let Some(prompt_file_path) = prompt_file_path {
            Self::remove_worker_launch_file(session_id, worker_cell_name, prompt_file_path);
        }
        Self::remove_worker_launch_file(session_id, worker_cell_name, task_file_path);
        if let Err(err) = remove_session_worktree_cell(project_path, session_id, worker_cell_name) {
            tracing::warn!(
                "Worker launch rollback failed to remove worktree for session {} cell {}: {}",
                session_id,
                worker_cell_name,
                err
            );
        } else {
            let branch_name = format!("hive/{session_id}/{worker_cell_name}");
            Self::delete_branch(project_path, &branch_name);
        }
    }

    fn delete_branch(project_path: &Path, branch_name: &str) {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(project_path)
            .arg("branch")
            .arg("-D")
            .arg(&branch_name);

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        match cmd.output() {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let message = if !stderr.is_empty() { stderr } else { stdout };
                tracing::warn!(
                    "Rollback failed to delete branch {}: {}",
                    branch_name,
                    if message.is_empty() { "git branch -D failed".to_string() } else { message }
                );
            }
            Err(err) => {
                tracing::warn!(
                    "Rollback failed to delete branch {}: {}",
                    branch_name,
                    err
                );
            }
        }
    }

    fn restore_session_state_after_worker_spawn_failure(
        &self,
        session_id: &str,
        previous_state: &SessionState,
    ) {
        let changes = {
            let mut sessions = self.sessions.write();
            sessions.get_mut(session_id).map(|session| {
                self.set_session_state_with_events(session, previous_state.clone())
            })
        };

        if let Some(changes) = changes {
            if let Err(err) = self.persist_then_emit_session_update(session_id, changes) {
                tracing::warn!(
                    "Failed to restore session {} state after worker spawn failure: {}",
                    session_id,
                    err
                );
            }
        }
    }

    pub fn stop_agent(&self, session_id: &str, agent_id: &str) -> Result<(), String> {
        let pty_manager = self.pty_manager.read();
        pty_manager.kill(agent_id).map_err(|e| e.to_string())?;

        let completed_agent = {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                if let Some(index) = session.agents.iter().position(|agent| agent.id == agent_id) {
                    session.agents[index].status = AgentStatus::Completed;
                    Some((session.clone(), session.agents[index].clone()))
                } else {
                    None
                }
            } else {
                None
            }
        };
        self.update_session_storage(session_id);
        if let Some((session, agent)) = completed_agent {
            self.emit_agent_completed(&session, &agent);
        }

        Ok(())
    }

    fn truncate_agent_label(value: String, max_chars: usize) -> String {
        let mut chars = value.chars();
        let truncated: String = chars.by_ref().take(max_chars).collect();
        if chars.next().is_some() {
            format!("{}...", truncated.trim_end())
        } else {
            value
        }
    }

    fn summarize_prompt_line(prompt: Option<&str>) -> Option<String> {
        prompt
            .and_then(|value| value.lines().find(|line| !line.trim().is_empty()))
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|line| !line.is_empty())
    }

    fn derive_worker_name(
        worker_index: u8,
        role: &WorkerRole,
        explicit_name: Option<&str>,
    ) -> String {
        explicit_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("Worker {} ({})", worker_index, role.label))
    }

    fn derive_worker_description(
        role: &WorkerRole,
        explicit_description: Option<&str>,
        prompt: Option<&str>,
    ) -> String {
        explicit_description
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .or_else(|| Self::summarize_prompt_line(prompt))
            .unwrap_or_else(|| format!("{} tasks", role.label))
    }

    fn derive_worker_label(name: &str, description: &str) -> String {
        Self::truncate_agent_label(format!("{} — {}", name, description), 80)
    }

    fn apply_worker_identity(
        worker_index: u8,
        role: &WorkerRole,
        mut config: AgentConfig,
    ) -> AgentConfig {
        let name = Self::derive_worker_name(worker_index, role, config.name.as_deref());
        let description = Self::derive_worker_description(
            role,
            config.description.as_deref(),
            config.initial_prompt.as_deref(),
        );
        config.name = Some(name.clone());
        config.description = Some(description.clone());
        config.label = Some(Self::derive_worker_label(&name, &description));
        config.role = Some(role.clone());
        config
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
            "claude" => {
                // Claude: positional prompt opens interactive mode with the prompt
                // (-p would be non-interactive print mode)
                args.push(task.to_string());
            }
            "gemini" => {
                args.push("-p".to_string());
                args.push(task.to_string());
            }
            "codex" => {
                // Codex uses positional prompt argument (no -q flag exists)
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

        // Add CLI-specific auto-approve flags (matching build_command for hive/swarm modes)
        match config.cli.as_str() {
            "claude" => {
                args.push("--dangerously-skip-permissions".to_string());
                if let Some(ref model) = config.model {
                    args.push("--model".to_string());
                    args.push(model.clone());
                }
            }
            "gemini" => {
                args.push("-y".to_string());
                if let Some(ref model) = config.model {
                    args.push("-m".to_string());
                    args.push(model.clone());
                }
            }
            "codex" => {
                args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
                if let Some(ref model) = config.model {
                    args.push("-m".to_string());
                    args.push(model.clone());
                }
            }
            "qwen" => {
                args.push("-y".to_string());
                if let Some(ref model) = config.model {
                    args.push("-m".to_string());
                    args.push(model.clone());
                }
            }
            "opencode" => {
                if let Some(ref model) = config.model {
                    args.push("-m".to_string());
                    args.push(model.clone());
                }
            }
            "cursor" => {
                args.push("-d".to_string());
                args.push("Ubuntu".to_string());
                args.push("/root/.local/bin/agent".to_string());
                args.push("--force".to_string());
            }
            "droid" => {
                // No auto-approve flag available
            }
            _ => {
                if let Some(ref model) = config.model {
                    args.push("--model".to_string());
                    args.push(model.clone());
                }
            }
        }

        // Add inline task if provided
        if let Some(task) = task {
            Self::add_inline_task_to_args(&config.cli, &mut args, task);
        }

        args.extend(config.flags.clone());

        let command = match config.cli.as_str() {
            "cursor" => "wsl".to_string(),
            _ => config.cli.clone(),
        };
        (command, args)
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
        worktree_path: &Path,
        variant_index: u8,
        variant_name: &str,
        task_description: &str,
    ) -> Result<PathBuf, String> {
        let tasks_dir = worktree_path.join(".hive-manager").join("tasks");
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

    fn fusion_variant_task_file_path(worktree_path: &Path, variant_index: usize) -> PathBuf {
        worktree_path
            .join(".hive-manager")
            .join("tasks")
            .join(format!("fusion-variant-{}-task.md", variant_index))
    }

    fn qa_task_file_path(project_path: &Path, session_id: &str, worker_index: usize) -> PathBuf {
        project_path
            .join(".hive-manager")
            .join(session_id)
            .join("tasks")
            .join(format!("qa-worker-{}-task.md", worker_index))
    }

    fn task_file_path_for_worker(worktree_path: &Path, worker_index: usize) -> PathBuf {
        worktree_path
            .join(".hive-manager")
            .join("tasks")
            .join(format!("worker-{}-task.md", worker_index))
    }

    pub(crate) fn absolute_task_file_path_for_worker(
        project_path: &Path,
        session_id: &str,
        worker_index: usize,
    ) -> PathBuf {
        let worktree_path = project_path
            .join(".hive-manager")
            .join("worktrees")
            .join(session_id)
            .join(format!("worker-{}", worker_index));
        Self::task_file_path_for_worker(&worktree_path, worker_index)
    }

    pub(crate) fn absolute_task_file_path_for_qa_worker(
        project_path: &Path,
        session_id: &str,
        worker_index: usize,
    ) -> PathBuf {
        Self::qa_task_file_path(project_path, session_id, worker_index)
    }

    fn build_fusion_worker_prompt(
        _session_id: &str,
        variant_index: u8,
        variant_name: &str,
        branch: &str,
        worktree_path: &str,
        task_description: &str,
        cli: &str,
    ) -> String {
        let task_file = format!(".hive-manager/tasks/fusion-variant-{}-task.md", variant_index);
        let polling_instructions = get_polling_instructions(cli, &task_file, None);
        let scope_block = Self::scope_block(".");

        format!(
r#"You are a Fusion worker implementing variant "{variant_name}".
Working directory: {worktree_path}
Branch: {branch}

## Your Task
{task_description}

{scope_block}

## Rules
- Commit all changes to your branch
- Do NOT interact with other variants
- When complete, update your task file status to COMPLETED

## Task Coordination
Read {task_file}. Begin work only when Status is ACTIVE.{polling_instructions}"#,
            variant_name = variant_name,
            worktree_path = worktree_path,
            branch = branch,
            task_description = task_description,
            scope_block = scope_block,
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

    fn prompt_path(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    fn worktree_boundary_rules(worktree_path: &str) -> String {
        format!(
            r#"- **READ**: You MAY inspect any repository file and git history for context by running Bash commands from this worktree.
- **WRITE**: You MUST create and modify files only inside `{worktree_path}`. You MUST NOT edit files outside this worktree."#,
            worktree_path = worktree_path,
        )
    }

    fn scope_block(worktree_path: &str) -> String {
        format!("## Scope\n\n{}", Self::worktree_boundary_rules(worktree_path))
    }

    fn queen_quality_reconciliation_log_lines(has_evaluator: bool) -> &'static str {
        if has_evaluator {
            QUEEN_QUALITY_RECONCILIATION_LOG_LINES
        } else {
            QUEEN_QUALITY_RECONCILIATION_LOG_LINES_NO_EVALUATOR
        }
    }

    fn queen_required_protocol(session_root: &Path, has_evaluator: bool) -> String {
        if !has_evaluator {
            return r#"## Required Protocol
```text
1. You MUST follow every numbered protocol in this prompt exactly as written.
2. You MUST use the inline bash polling commands shown in this prompt. You MUST NOT use `/loop`.
```"#
                .to_string();
        }

        let milestone_ready_path =
            Self::prompt_path(&session_root.join("peer").join("milestone-ready.json"));
        let qa_verdict_path =
            Self::prompt_path(&session_root.join("peer").join("qa-verdict.json"));

        format!(
            r#"## Required Protocol
```text
1. You MUST follow every numbered protocol in this prompt exactly as written.
2. You MUST use the inline bash polling commands shown in this prompt. You MUST NOT use `/loop`.
3. The Evaluator is created PROGRAMMATICALLY by the backend at session launch (`spawn_launch_evaluator_agents`). It already exists as `AgentRole::Evaluator`.
4. You MUST NOT spawn an Evaluator yourself. DO NOT `curl POST /workers` with `role=evaluator`. DO NOT `curl POST /evaluators`.
5. You MUST signal the existing Evaluator via `{milestone_ready_path}` and WAIT for `{qa_verdict_path}`.
```"#,
            milestone_ready_path = milestone_ready_path,
            qa_verdict_path = qa_verdict_path,
        )
    }

    fn evaluator_required_protocol(session_id: &str) -> String {
        format!(
            r#"## Required Protocol
```text
1. You MUST follow every numbered protocol in this prompt exactly as written.
2. You MUST use the inline bash polling commands shown in this prompt. You MUST NOT use `/loop`.
3. The backend already launched you as `AgentRole::Evaluator`. You MUST NOT spawn another Evaluator or ask the Queen to create one.
4. The Queen signals you via `.hive-manager/{session_id}/peer/milestone-ready.json`. You MUST wait for that handoff before you read the contract or grade criteria.
5. You MUST submit the verdict via `POST /api/sessions/{session_id}/qa/verdict`. You MUST NOT write shadow verdict files.
```"#,
            session_id = session_id,
        )
    }

    fn queen_post_workers_protocol(session_id: &str, session_root: &Path, has_evaluator: bool) -> String {
        let milestone_ready_path =
            Self::prompt_path(&session_root.join("peer").join("milestone-ready.json"));
        let qa_verdict_path =
            Self::prompt_path(&session_root.join("peer").join("qa-verdict.json"));

        if !has_evaluator {
            return format!(
                r#"## Post-Workers Protocol (MANDATORY)

1. You MUST commit and push the PR branch. This triggers CodeRabbit and Gemini external reviewers.
2. You MUST wait 10 minutes, collect PR comments plus any remaining integrity concerns, and use this `gh api` workflow:
   ```bash
   gh api repos/<owner>/<repo>/issues/<pr-number>/comments
   gh api repos/<owner>/<repo>/pulls/<pr-number>/comments
   ```
3. If unresolved findings remain, you MUST spawn a Reconciler worker and the required resolver workers via `POST /api/sessions/{session_id}/workers`, integrate their fixes, and then return to Step 1.
   ```bash
   curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
     -H "Content-Type: application/json" \
     -d '{{"role_type":"reconciler","cli":"<configured-cli>","name":"Reconciler","description":"Consolidate external review comments and integrity findings into one fix list"}}'

   curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
     -H "Content-Type: application/json" \
     -d '{{"role_type":"resolver","cli":"<configured-cli>","name":"Resolver 1","description":"Fix HIGH/MEDIUM findings from the reconciled list"}}'
   ```
4. You MUST call `POST /api/sessions/{session_id}/complete` only after the latest push has aged at least 10 minutes and there are no new unresolved PR comments or integrity concerns.
"#,
                session_id = session_id,
            );
        }

        format!(
            r#"## Post-Workers Protocol (MANDATORY)

Hard rule: The Evaluator is created PROGRAMMATICALLY by the backend at session launch (`spawn_launch_evaluator_agents`). It already exists as `AgentRole::Evaluator`. You MUST NOT spawn an Evaluator. DO NOT `curl POST /workers` with `role=evaluator`. DO NOT `curl POST /evaluators`. Signal via `{milestone_ready_path}` and WAIT for `{qa_verdict_path}`.

1. You MUST execute the QA Milestone Handoff block below exactly as written. Treat Step 2 of that handoff as blocking.
2. You MUST wait for the Evaluator verdict by polling `{qa_verdict_path}` inline. You MUST NOT use `/loop`.
   ```bash
   while [ ! -f "{qa_verdict_path}" ]; do
     sleep 30
   done
   cat "{qa_verdict_path}"
   ```
3. You MUST inspect the verdict. If it says `PASS`, continue to Step 5. If it says `FAIL`, continue to Step 4.
4. You MUST spawn a Reconciler worker and the required resolver workers via `POST /api/sessions/{session_id}/workers`. Reconcile Evaluator findings, external review comments, and your own integrity concerns before continuing.
   ```bash
   curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
     -H "Content-Type: application/json" \
     -d '{{"role_type":"reconciler","cli":"<configured-cli>","name":"Reconciler","description":"Consolidate evaluator verdicts, external review comments, and integrity findings into one fix list"}}'

   curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
     -H "Content-Type: application/json" \
     -d '{{"role_type":"resolver","cli":"<configured-cli>","name":"Resolver 1","description":"Fix HIGH/MEDIUM findings from the reconciled list"}}'
   ```
5. You MUST commit and push the PR branch. This triggers CodeRabbit and Gemini external reviewers.
6. You MUST wait 10 minutes, collect PR comments plus any remaining integrity concerns, and use this `gh api` workflow before looping back to Step 4 whenever unresolved findings remain:
   ```bash
   gh api repos/<owner>/<repo>/issues/<pr-number>/comments
   gh api repos/<owner>/<repo>/pulls/<pr-number>/comments
   ```
7. You MUST call `POST /api/sessions/{session_id}/complete` only after QA is PASS, the latest push has aged at least 10 minutes, and there are no new unresolved PR comments.
"#,
            milestone_ready_path = milestone_ready_path,
            qa_verdict_path = qa_verdict_path,
            session_id = session_id,
        )
    }

    fn session_root_path(project_path: &Path, session_id: &str) -> PathBuf {
        project_path.join(".hive-manager").join(session_id)
    }

    fn build_evaluator_qa_plan(
        default_config: &AgentConfig,
        qa_workers: &[QaWorkerConfig],
    ) -> (String, String, String, String) {
        let configured_workers = if qa_workers.is_empty() {
            vec![
                QaWorkerConfig {
                    specialization: "api".to_string(),
                    cli: default_config.cli.clone(),
                    model: default_config.model.clone(),
                    label: Some(Self::qa_worker_label("api").to_string()),
                    flags: None,
                },
                QaWorkerConfig {
                    specialization: "ui".to_string(),
                    cli: default_config.cli.clone(),
                    model: default_config.model.clone(),
                    label: Some(Self::qa_worker_label("ui").to_string()),
                    flags: None,
                },
                QaWorkerConfig {
                    specialization: "a11y".to_string(),
                    cli: default_config.cli.clone(),
                    model: default_config.model.clone(),
                    label: Some(Self::qa_worker_label("a11y").to_string()),
                    flags: None,
                },
            ]
        } else {
            qa_workers.to_vec()
        };

        let mut command_block = String::new();
        for (index, worker) in configured_workers.iter().enumerate() {
            let label = worker
                .label
                .as_deref()
                .unwrap_or(Self::qa_worker_label(&worker.specialization));
            let payload = serde_json::to_string(worker)
                .unwrap_or_else(|_| format!(
                    r#"{{"specialization":"{}","cli":"{}"}}"#,
                    worker.specialization, worker.cli
                ))
                .replace('\'', "'\\''");

            command_block.push_str(&format!(
                "   # {}. {} worker\n   curl -X POST \"{{{{api_base_url}}}}/api/sessions/{{{{session_id}}}}/qa-workers\" \\\n     -H \"Content-Type: application/json\" \\\n     -d '{}'\n\n",
                index + 1,
                label,
                payload,
            ));
        }

        let intro = if qa_workers.is_empty() {
            "You start with NO QA workers. You MUST spawn all three specializations before you grade any criterion.".to_string()
        } else {
            format!(
                "You start with NO QA workers. You MUST spawn the configured QA workers below ({} total) before you grade any criterion.",
                configured_workers.len()
            )
        };
        let spawn_plan = format!(
            "```bash\n{}   ```",
            command_block,
        );
        let coverage_rule = if qa_workers.is_empty() {
            "You MUST NOT skip any specialization. Every milestone requires full coverage.".to_string()
        } else {
            "You MUST NOT skip any configured QA specialization. Every milestone requires the requested coverage.".to_string()
        };

        (
            intro,
            spawn_plan,
            configured_workers.len().to_string(),
            coverage_rule,
        )
    }

    #[allow(dead_code)]
    fn build_evaluator_prompt(
        session_id: &str,
        config: &AgentConfig,
        qa_workers: &[QaWorkerConfig],
        smoke_test: bool,
    ) -> String {
        let custom_instructions = config.initial_prompt.as_deref().unwrap_or(
            "You MUST grade the milestone against the contract, spawn QA workers when direct evidence is missing, and return a strict PASS/FAIL verdict with criterion-numbered evidence.",
        );
        let default_model = config.model.as_deref().unwrap_or("");
        let default_model_suffix = if default_model.is_empty() {
            String::new()
        } else {
            format!(", Model: {}", default_model)
        };
        let default_model_field = if default_model.is_empty() {
            String::new()
        } else {
            format!(r#""model": "{}", "#, default_model)
        };
        let (qa_worker_intro, qa_worker_spawn_plan, qa_worker_count, qa_worker_coverage_rule) =
            Self::build_evaluator_qa_plan(config, qa_workers);
        let required_protocol = Self::evaluator_required_protocol(session_id);

        let mut variables = HashMap::new();
        variables.insert("custom_instructions".to_string(), custom_instructions.to_string());
        variables.insert("default_cli".to_string(), config.cli.clone());
        variables.insert("default_model".to_string(), default_model.to_string());
        variables.insert("default_model_field".to_string(), default_model_field);
        variables.insert("default_model_suffix".to_string(), default_model_suffix);
        variables.insert("required_protocol".to_string(), required_protocol);
        variables.insert("qa_worker_intro".to_string(), qa_worker_intro);
        variables.insert("qa_worker_spawn_plan".to_string(), qa_worker_spawn_plan);
        variables.insert("qa_worker_count".to_string(), qa_worker_count);
        variables.insert(
            "qa_worker_coverage_rule".to_string(),
            qa_worker_coverage_rule,
        );

        if smoke_test {
            variables.insert("idle_poll_interval".to_string(), "30 seconds".to_string());
            variables.insert("idle_poll_secs".to_string(), "30".to_string());
            variables.insert("active_poll_interval".to_string(), "15 seconds".to_string());
            variables.insert("active_poll_secs".to_string(), "15".to_string());
        } else {
            variables.insert("idle_poll_interval".to_string(), "20 minutes".to_string());
            variables.insert("idle_poll_secs".to_string(), "1200".to_string());
            variables.insert("active_poll_interval".to_string(), "5 minutes".to_string());
            variables.insert("active_poll_secs".to_string(), "300".to_string());
        }

        Self::render_named_prompt("roles/evaluator", session_id, None, variables)
    }

    #[allow(dead_code)]
    fn build_qa_worker_prompt(
        session_id: &str,
        index: u8,
        specialization: &str,
        config: &AgentConfig,
        auth: &AuthStrategy,
    ) -> String {
        let (template_name, default_guidance) = match specialization {
            "ui" => (
                "roles/qa-worker-ui",
                "Validate the full UI flow, capture screenshot evidence, and report failures only with criterion-numbered proof.",
            ),
            "api" => (
                "roles/qa-worker-api",
                "Exercise the API surface directly, include concrete request and response evidence, and fail ambiguous behavior.",
            ),
            "a11y" => (
                "roles/qa-worker-a11y",
                "Audit accessibility rigorously with tooling and manual keyboard checks, then report criterion-numbered findings with exact defects.",
            ),
            _ => (
                "roles/qa-worker-api",
                "Exercise the API surface directly, include concrete request and response evidence, and fail ambiguous behavior.",
            ),
        };

        let custom_instructions = config
            .initial_prompt
            .as_deref()
            .unwrap_or(default_guidance);

        let mut variables = HashMap::new();
        variables.insert("qa_worker_index".to_string(), index.to_string());
        variables.insert("custom_instructions".to_string(), custom_instructions.to_string());
        variables.insert(
            "supports_chrome".to_string(),
            (specialization == "ui" && config.cli == "claude").to_string(),
        );

        auth.apply_prompt_variables(session_id, &mut variables);

        Self::render_named_prompt(template_name, session_id, None, variables)
    }

    fn qa_worker_label(specialization: &str) -> &'static str {
        match specialization {
            "ui" => "UI QA",
            "api" => "API QA",
            "a11y" => "A11Y QA",
            _ => "QA Worker",
        }
    }

    fn render_named_prompt(
        template_name: &str,
        session_id: &str,
        task: Option<String>,
        variables: HashMap<String, String>,
    ) -> String {
        let engine = TemplateEngine::default();
        let context = PromptContext {
            session_id: session_id.to_string(),
            task,
            variables,
            ..PromptContext::default()
        };

        engine
            .render_template(template_name, &context)
            .unwrap_or_else(|_| format!("Template '{}' failed to render for session {}", template_name, session_id))
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

## Project Knowledge Intake

Before investigating, read:
- `.ai-docs/project-dna.md`
- `.ai-docs/learnings.jsonl`

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
        project_path: &Path,
        session_id: &str,
        variants: &[FusionVariantMetadata],
        task_description: &str,
        has_evaluator: bool,
    ) -> String {
        let session_root = Self::session_root_path(project_path, session_id);
        let variant_count = variants.len();
        let mut variant_info = String::new();
        let mut task_files = String::new();
        for v in variants {
            variant_info.push_str(&format!("| {} | {} | {} | {} |\n", v.index, v.name, v.branch, v.worktree_path));
            task_files.push_str(&format!("- Variant {} ({}): `{}`\n", v.index, v.name, v.task_file));
        }
        let required_protocol = Self::queen_required_protocol(&session_root, has_evaluator);
        let qa_milestone_handoff = if has_evaluator {
            Self::build_qa_milestone_handoff(session_id, &session_root, "winner integration work")
        } else {
            String::new()
        };
        let post_workers_protocol =
            Self::queen_post_workers_protocol(session_id, &session_root, has_evaluator);
        let status_reporting_lines = if has_evaluator {
            r#"[TIMESTAMP] QUEEN: Variant N (name) - COMPLETED/IN_PROGRESS/FAILED
[TIMESTAMP] QUEEN: All variants complete - spawning Judge
[TIMESTAMP] QUEEN: Judge evaluation complete
[TIMESTAMP] QUEEN: Entering quality loop for latest push
[TIMESTAMP] QUEEN: QA PASS received / waiting on QA PASS
[TIMESTAMP] QUEEN: Latest push has / has not aged 10 minutes
[TIMESTAMP] QUEEN: Found / no new unresolved PR comments since latest push
[TIMESTAMP] QUEEN: Quality loop complete - session marked completed"#
        } else {
            r#"[TIMESTAMP] QUEEN: Variant N (name) - COMPLETED/IN_PROGRESS/FAILED
[TIMESTAMP] QUEEN: All variants complete - spawning Judge
[TIMESTAMP] QUEEN: Judge evaluation complete
[TIMESTAMP] QUEEN: Entering quality loop for latest push
[TIMESTAMP] QUEEN: Latest push has / has not aged 10 minutes
[TIMESTAMP] QUEEN: Found / no new unresolved PR comments since latest push
[TIMESTAMP] QUEEN: Quality loop complete - session marked completed"#
        };
        let task_file_glob = variants
            .iter()
            .map(|variant| format!("\"{}\"", Self::prompt_path(Path::new(&variant.task_file))))
            .collect::<Vec<_>>()
            .join(" ");

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
{required_protocol}

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

{qa_milestone_handoff}

{post_workers_protocol}

## Status Reporting

Write status updates to `.hive-manager/{session_id}/coordination.log`:
```
{status_reporting_lines}
```

## Learning Tools

Read tool docs in `.hive-manager/{session_id}/tools/` for:
- `submit-learning.md` — Record observations
- `list-learnings.md` — View existing learnings
"#,
            variant_count = variant_count,
            hardening = hardening,
            required_protocol = required_protocol,
            session_id = session_id,
            task_description = task_description,
            variant_info = variant_info,
            task_files = task_files,
            task_file_glob = task_file_glob,
            cli = cli,
            qa_milestone_handoff = qa_milestone_handoff,
            post_workers_protocol = post_workers_protocol,
            status_reporting_lines = status_reporting_lines,
        )
    }

    fn build_qa_milestone_handoff(
        _session_id: &str,
        session_root: &Path,
        completion_scope: &str,
    ) -> String {
        let peer_dir = Self::prompt_path(&session_root.join("peer"));
        let milestone_ready_path =
            Self::prompt_path(&session_root.join("peer").join("milestone-ready.json"));
        let qa_verdict_path =
            Self::prompt_path(&session_root.join("peer").join("qa-verdict.json"));
        let contracts_dir = Self::prompt_path(&session_root.join("contracts"));
        let contract_path = Self::prompt_path(&session_root.join("contracts").join("milestone-1.md"));

        format!(
r#"## QA Milestone Handoff (CRITICAL — Evaluator waits for this)

When ALL {completion_scope} have completed, you MUST signal the existing Evaluator:

1. You MUST create or update the contract FIRST. For smoke tests, use this contract:
   ```bash
   mkdir -p "{contracts_dir}"
   cat > "{contract_path}" << 'CONTRACT_EOF'
   # Smoke Test Contract

   ## Criteria
   1. All workers spawned and ran successfully
   2. Heartbeat API exercised by all workers
   3. Conversation API exercised (queen inbox + shared channel)
   4. All task files transitioned to COMPLETED status
   CONTRACT_EOF
   ```

2. You MUST write the milestone payload to a temp file in `{peer_dir}` and rename it to `{milestone_ready_path}` LAST. This step is blocking. The already-running Evaluator polls the final filename.
   ```bash
   mkdir -p "{peer_dir}"
   TMP_MILESTONE="$(mktemp "{peer_dir}/milestone-ready.XXXXXX")"
   cat > "$TMP_MILESTONE" << 'MILESTONE_EOF'
   {{"kind":"milestone-ready","from":"queen","to":"evaluator","content":"MILESTONE_READY\nmilestone: [name or smoke-test]\ncontract: {contract_path}\nscope: [brief description of what was implemented]\nrisks: [known risks or none]"}}
   MILESTONE_EOF
   mv "$TMP_MILESTONE" "{milestone_ready_path}"
   ```

3. You MUST NOT spawn an Evaluator here. The backend already launched it. After this handoff exists, continue with the Post-Workers Protocol and wait for `{qa_verdict_path}`."#,
            completion_scope = completion_scope,
            peer_dir = peer_dir,
            milestone_ready_path = milestone_ready_path,
            qa_verdict_path = qa_verdict_path,
            contracts_dir = contracts_dir,
            contract_path = contract_path,
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

## Project Knowledge Intake

Before investigating, read:
- `.ai-docs/project-dna.md`
- `.ai-docs/learnings.jsonl`

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

### Scout 3 - Cursor (Quick Search)

Task(subagent_type="general-purpose", prompt="You are a codebase investigation agent. IMMEDIATELY run: cursor-cli --print 'Scout codebase for: [TASK]. Identify entry points, test files, implementation surface.' Return file paths with notes.")

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

## Project Knowledge Intake

Before investigating, read:
- `.ai-docs/project-dna.md`
- `.ai-docs/learnings.jsonl`

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

# Scout 3 - Related Code (Cursor)
cursor-cli --print "Find code related to: [TASK]"
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
    fn build_smoke_test_prompt(
        session_id: &str,
        workers: &[AgentConfig],
        with_evaluator: bool,
        qa_workers: Option<&[QaWorkerConfig]>,
    ) -> String {
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
            let task_desc = match index {
                1 => format!("Send a message to queen via conversation API, send heartbeat, then read shared conversation -> Worker {}", index),
                2 => format!("Read queen conversation for messages, post to shared conversation, send heartbeat with summary -> Worker {}", index),
                _ => format!("Send heartbeat, read shared conversation, post completion message to queen -> Worker {}", index),
            };
            task_list.push_str(&format!(
                "- [ ] [{}] Smoke test task {}: {} \n",
                priority, index, task_desc
            ));

            if index > 1 {
                dependencies.push_str(&format!("- Task {} depends on Task {} completing.\n", index, index - 1));
            }
        }

        if dependencies.is_empty() {
            dependencies = "None - single worker smoke test.".to_string();
        }

        // Build evaluator/QA section if configured
        let evaluator_section = if with_evaluator {
            let qa_list = qa_workers.unwrap_or(&[]);
            let mut qa_table = String::new();
            let mut qa_tasks = String::new();
            for (i, qw) in qa_list.iter().enumerate() {
                let idx = i + 1;
                let label = qw.label.as_deref().unwrap_or(Self::qa_worker_label(&qw.specialization));
                qa_table.push_str(&format!("| QA Worker {} | {} | {} | {} |\n", idx, label, qw.specialization, qw.cli));
                qa_tasks.push_str(&format!(
                    "### QA Worker {} ({} - {}):\n\
                     1. Read the evaluator prompt: `curl -s \"http://localhost:18800/api/sessions/{}/evaluators\"`\n\
                     2. Exercise the {} endpoint smoke test\n\
                     3. Post QA findings to shared conversation\n\
                     4. Mark task file as COMPLETED\n\n",
                    idx, label, qw.specialization, session_id, qw.specialization
                ));
            }
            if qa_table.is_empty() {
                qa_table = "| (no QA workers configured) | - | - | - |\n".to_string();
                qa_tasks = "No QA workers configured. Evaluator will self-assess.\n".to_string();
            }
            format!(
r#"

## Evaluator & QA Configuration

An **Evaluator** agent will be spawned after workers complete. It reviews the milestone handoff
and coordinates QA workers to validate the work.

| QA Worker | Label | Specialization | CLI |
|-----------|-------|----------------|-----|
{qa_table}
## Evaluator Smoke Test Tasks

After all worker tasks complete, the Evaluator will:
1. List evaluators: `curl -s "http://localhost:18800/api/sessions/{session_id}/evaluators"`
2. Review worker task files for COMPLETED status
3. Coordinate QA workers (if any) to validate

{qa_tasks}### Evaluator Verdict:
1. Collect QA worker results
2. Submit verdict via HTTP endpoint: `curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/qa/verdict" -H "Content-Type: application/json" -d '{{"verdict":"PASS","rationale":"smoke test validated"}}'`
"#,
                qa_table = qa_table.trim_end(),
                qa_tasks = qa_tasks,
                session_id = session_id,
            )
        } else {
            String::new()
        };

        let evaluator_test_items = if with_evaluator {
            let qa_count = qa_workers.map(|q| q.len()).unwrap_or(0);
            format!(
                "\n4. Evaluator spawns and reviews worker output\n\
                 5. {} QA worker(s) exercise their specialization\n\
                 6. Evaluator submits verdict via POST /api/sessions/{session_id}/qa/verdict",
                qa_count
            )
        } else {
            String::new()
        };

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
## Task Details

Each worker should use the Inter-Agent Communication endpoints from their prompt.
Workers MUST use curl to exercise the conversation and heartbeat APIs.

### Task 1 (Worker 1):
1. Send heartbeat: `curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/heartbeat" -H "Content-Type: application/json" -d '{{"agent_id":"worker-1","status":"working","summary":"Starting smoke test"}}'`
2. Post message to queen: `curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/conversations/queen/append" -H "Content-Type: application/json" -d '{{"from":"worker-1","content":"Worker 1 reporting in. Smoke test task started."}}'`
3. Post to shared: `curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/conversations/shared/append" -H "Content-Type: application/json" -d '{{"from":"worker-1","content":"Worker 1 completed conversation smoke test."}}'`
4. Send completed heartbeat: `curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/heartbeat" -H "Content-Type: application/json" -d '{{"agent_id":"worker-1","status":"completed","summary":"Smoke test done"}}'`

### Task 2 (Worker 2, if present):
1. Send heartbeat with working status
2. Read queen conversation: `curl -s "http://localhost:18800/api/sessions/{session_id}/conversations/queen"`
3. Read shared conversation: `curl -s "http://localhost:18800/api/sessions/{session_id}/conversations/shared"`
4. Post message to queen confirming what was read
5. Send completed heartbeat

### Task N (additional workers):
1. Send heartbeat, read shared, post completion message to queen, send completed heartbeat
{evaluator_section}
## Files to Modify
| File | Priority | Changes Needed |
|------|----------|----------------|
| (smoke test - no real files) | N/A | N/A |

## Dependencies
{dependencies}
## Risks
None - this is a smoke test.

## Notes
This smoke test validates the inter-agent conversation and heartbeat flow.
Testing all {worker_count} configured workers with real API calls.
```

After writing the plan, say: **"PLAN READY FOR REVIEW"**

This tests that:
1. Master Planner can write to the plan file
2. User can see and approve the plan
3. Flow continues to spawn Queen and all {worker_count} Workers{evaluator_test_items}"#,
            session_id = session_id,
            worker_count = workers.len(),
            worker_table = worker_table.trim_end(),
            task_list = task_list.trim_end(),
            dependencies = dependencies.trim_end(),
            evaluator_section = evaluator_section,
            evaluator_test_items = evaluator_test_items,
        )
    }

    /// Build a smoke test prompt for Swarm mode that accounts for planners AND workers
    fn build_swarm_smoke_test_prompt(
        session_id: &str,
        planner_count: u8,
        workers_per_planner: &[AgentConfig],
        with_evaluator: bool,
        qa_workers: Option<&[QaWorkerConfig]>,
    ) -> String {
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

        // Build evaluator/QA section if configured
        let evaluator_section = if with_evaluator {
            let qa_list = qa_workers.unwrap_or(&[]);
            let mut qa_info = String::new();
            for (i, qw) in qa_list.iter().enumerate() {
                let label = qw.label.as_deref().unwrap_or(Self::qa_worker_label(&qw.specialization));
                qa_info.push_str(&format!("| QA Worker {} | {} | {} | {} |\n", i + 1, label, qw.specialization, qw.cli));
            }
            if qa_info.is_empty() {
                qa_info = "| (no QA workers configured) | - | - | - |\n".to_string();
            }
            format!(
r#"

## Evaluator & QA Configuration

An **Evaluator** agent validates work after all planners complete.

| QA Worker | Label | Specialization | CLI |
|-----------|-------|----------------|-----|
{qa_info}
After all planner domains complete, the Evaluator will:
1. Review all worker outputs across all domains
2. Coordinate QA workers to validate each domain
3. Submit verdict via HTTP endpoint: `POST /api/sessions/{{{{session_id}}}}/qa/verdict`
"#,
                qa_info = qa_info.trim_end(),
            )
        } else {
            String::new()
        };

        let evaluator_test_items = if with_evaluator {
            let qa_count = qa_workers.map(|q| q.len()).unwrap_or(0);
            format!(
                "\n6. Evaluator reviews all planner outputs\n\
                 7. {} QA worker(s) validate domain results\n\
                 8. Evaluator submits verdict via POST /api/sessions/{{{{session_id}}}}/qa/verdict",
                qa_count
            )
        } else {
            String::new()
        };

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
{evaluator_section}
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
5. Queen commits between each Planner completion{evaluator_test_items}"#,
            session_id = session_id,
            planner_count = planner_count,
            workers_per = workers_per,
            total_workers = total_workers,
            planner_table = planner_table.trim_end(),
            domain_tasks = domain_tasks.trim_end(),
            worker_breakdown = worker_breakdown.trim_end(),
            evaluator_section = evaluator_section,
            evaluator_test_items = evaluator_test_items,
        )
    }

    /// Build the Queen's master prompt with worker information
    fn build_queen_master_prompt(
        cli: &str,
        project_path: &Path,
        queen_workspace_path: &Path,
        session_id: &str,
        workers: &[AgentConfig],
        user_prompt: Option<&str>,
        has_plan: bool,
        has_evaluator: bool,
    ) -> String {
        let session_root = Self::session_root_path(project_path, session_id);
        let prompts_dir = Self::prompt_path(&session_root.join("prompts"));
        let _tasks_dir = Self::prompt_path(&session_root.join("tasks"));
        let tools_dir = Self::prompt_path(&session_root.join("tools"));
        let conversations_dir = session_root.join("conversations");
        let queen_conversation = Self::prompt_path(&conversations_dir.join("queen.md"));
        let shared_conversation = Self::prompt_path(&conversations_dir.join("shared.md"));
        let worker_conversation_glob = Self::prompt_path(&conversations_dir.join("worker-N.md"));
        let plan_path = Self::prompt_path(&session_root.join("plan.md"));
        let lessons_dir = Self::prompt_path(&session_root.join("lessons"));
        let coordination_log_path = Self::prompt_path(&session_root.join("coordination.log"));
        let worker_worktree_root =
            Self::prompt_path(&project_path.join(".hive-manager").join("worktrees").join(session_id));
        let queen_scope_rules =
            Self::worktree_boundary_rules(&Self::prompt_path(queen_workspace_path));
        let required_protocol = Self::queen_required_protocol(&session_root, has_evaluator);
        let post_workers_protocol =
            Self::queen_post_workers_protocol(session_id, &session_root, has_evaluator);
        let final_integration_step = if has_evaluator {
            "8. **Signal Evaluator** - Once all tasks are done, write milestone-ready (see above)"
        } else {
            ""
        };


        let mut worker_list = String::new();
        for (i, worker_config) in workers.iter().enumerate() {
            let index = i + 1;
            let worker_id = format!("{}-worker-{}", session_id, index);
            let role_label = worker_config
                .role
                .as_ref()
                .map(|r| format!("Worker {} ({})", index, r.label))
                .unwrap_or_else(|| format!("Worker {}", index));
            worker_list.push_str(&format!(
                "| {} | {} | {} |\n",
                worker_id, role_label, worker_config.cli
            ));
        }

        let worker_worktrees_dir = Self::prompt_path(
            &project_path.join(".hive-manager").join("worktrees").join(session_id),
        );
        let worker_task_file_example = Self::prompt_path(
            &project_path
                .join(".hive-manager")
                .join("worktrees")
                .join(session_id)
                .join("worker-N")
                .join(".hive-manager")
                .join("tasks")
                .join("worker-N-task.md"),
        );
        let plan_section = if has_plan {
            format!(
                r#"## Implementation Plan

**IMPORTANT**: A plan has been generated for this session. Read it first:
```
{}
```

Follow the plan's task breakdown when assigning work to workers."#,
                plan_path
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
        let qa_milestone_handoff = if has_evaluator {
            Self::build_qa_milestone_handoff(session_id, &session_root, "workers")
        } else {
            String::new()
        };

        format!(
r#"# Queen Agent - Hive Manager Session

You are the **Queen** orchestrating a multi-agent Hive session. You have full Claude Code capabilities plus coordination tools.
{hardening}
{branch_protocol}
{required_protocol}
## Session Info
- **Session ID**: {session_id}
- **Prompts Directory**: `{prompts_dir}`
- **Worker Task Files**: each worker keeps `.hive-manager/tasks/worker-N-task.md` inside its own worktree under `{worker_worktrees_dir}`
- **Tools Directory**: `{tools_dir}`
- **Conversation Files**: `{queen_conversation}`, `{shared_conversation}`, `{worker_conversation_glob}`

## Project Knowledge Intake

Before assigning work, read:
- `.ai-docs/project-dna.md`
- `.ai-docs/learnings.jsonl`

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

Tool documentation is in `{tools_dir}`. Read these files for detailed usage:

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
  -d '{{"role_type": "backend", "cli": "{cli}", "name": "Worker 1 (Backend)", "description": "Implement backend changes"}}'
```

### Task Assignment
To assign tasks to existing workers, update their task files:

```
Edit: {worker_task_file_example}
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

### File-Based Fallback

If curl returns exit code 7 (connection refused) or any non-zero exit, write directly to the conversation files instead:

```bash
# Append to a worker's inbox (fallback)
echo -e "---\n[$(date -u +%Y-%m-%dT%H:%M:%SZ)] from @queen\nYour message here\n" >> "{worker_conversation_glob}"

# Append to shared channel (fallback)
echo -e "---\n[$(date -u +%Y-%m-%dT%H:%M:%SZ)] from @queen\nYour message here\n" >> "{shared_conversation}"

# Read queen inbox (fallback)
cat "{queen_conversation}"

# Read shared broadcasts (fallback)
cat "{shared_conversation}"
```

You MUST call the curl API first. Only use file fallback if curl fails.

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
{lessons_dir}/
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

{qa_milestone_handoff}

## Coordination Protocol

1. **Read the plan** - Check `{plan_path}` if it exists
2. **Spawn workers** - Use the spawn-worker tool to create workers as needed
3. **Assign tasks** - Update worker task files with specific assignments
4. **Monitor progress** - Watch for workers to mark tasks COMPLETED
5. **Spawn next worker** - When a task completes, spawn the next worker if needed
6. **Review & integrate** - Review worker output and coordinate integration

## Worktree Awareness (READ THIS FIRST)

You are running in your own worktree, separate from the workers.

{queen_scope_rules}
- `git status` / `git diff` in your CWD will NOT show worker changes.
- `ls`, Read, and Glob in your CWD show your own worktree snapshot, not a worker's live edits.
- Never assume "no diff = no work done." Workers commit into `hive/{session_id}/worker-N` branches inside their own worktree paths.

### Worker progress cheat sheet

Always target the worker's worktree path or branch explicitly:

```bash
WT="{worker_worktree_root}/worker-N"
BR=hive/{session_id}/worker-N

# Has the worker committed anything yet?
git -C "$WT" log --oneline "$BR" ^<feature-branch>

# What's changed (committed)?
git -C "$WT" diff --stat <feature-branch>...$BR
git -C "$WT" diff <feature-branch>...$BR -- <path>

# What's in-flight (staged + unstaged + untracked)?
git -C "$WT" status --short
git -C "$WT" diff            # unstaged
git -C "$WT" diff --cached   # staged

# Read a file as the worker currently has it on disk:
cat "$WT/<relative/path>"
# Or as committed on their branch:
git -C "$WT" show "$BR:<relative/path>"
```

If a worker's task file says COMPLETED but `git log` on their branch is empty, check `git status` in their worktree before treating it as a failure.

### Monitoring cadence

When polling for worker progress, iterate over every worker worktree instead of relying on your own CWD's `git status`:

```bash
for WT in "{worker_worktree_root}"/worker-*; do
  BR="hive/{session_id}/$(basename "$WT")"
  echo "=== $BR ==="
  git -C "$WT" log --oneline "$BR" ^<feature-branch> 2>/dev/null | head -5
  git -C "$WT" status --short
done
```

## Worktree Integration Protocol

Workers run in isolated git worktrees. Each worker has its own worktree + branch created by the backend at `{worker_worktree_root}/worker-N` on branch `hive/{session_id}/worker-N`. Integrate them back into the feature branch as follows:

### Step 0 — Learning Consolidation (MANDATORY, before any cherry-pick)

Worker worktrees are ephemeral. Learnings written directly to `.ai-docs/learnings.jsonl` there will be lost at cleanup, so consolidate into the main repo's `.ai-docs/learnings.jsonl` first:

**a. Primary — flush the session-scoped store (deterministic):**
```bash
curl -s "http://localhost:18800/api/sessions/{session_id}/learnings" \
  | jq -c '.learnings[]? // .[]?' \
  >> .ai-docs/learnings.jsonl
```
Deduplicate against existing lines (e.g., by `task` + `insight`) before appending.

**b. Fallback sweep — scan worker worktrees for any direct file writes:**
```bash
for WT in "{worker_worktree_root}"/*; do
  f="$WT/.ai-docs/learnings.jsonl"
  [ -f "$f" ] || continue
  # Append only lines not already present in root
  comm -13 <(sort -u .ai-docs/learnings.jsonl) <(sort -u "$f") >> .ai-docs/learnings.jsonl
done
# Also scoop session-scoped files
for f in .hive-manager/{session_id}/learnings*.json .hive-manager/{session_id}/learning-submission.json; do
  [ -f "$f" ] && echo "Review and merge: $f"
done
```

**c. Stage the updated learnings file into your integration commit** so consolidation is visible in the PR.

1. **LOCATE** each worker's worktree at `{worker_worktree_root}/worker-N` on branch `hive/{session_id}/worker-N`. Inspect changes via:
   - `git -C <worktree> log <branch> ^<feature-branch>`
   - `git -C <worktree> diff <feature-branch>...<branch>`

2. **CHOOSE** integration method per worker:
   - **Preferred — cherry-pick the full branch range**: `git rev-list --reverse <feature-branch>..<branch> | xargs -n1 git cherry-pick` — preserves the worker's full commit history.
   - **Squash merge**: `git merge --squash <branch> && git commit -m '...'` — use when the worker made noisy WIP commits.
   - **Patch apply**: only use this when the worker has no commits and has staged/tracked all files first; otherwise newly created untracked files will be missed.

3. **ORDER integration** to minimize conflicts. Integrate disjoint-file tasks first. For tasks that touch overlapping files, integrate one, then rebase the next worker branch onto the updated tip before picking.

4. **COMMIT CADENCE**: one separate commit per worker on the feature branch; push after each commit to give external reviewers (CodeRabbit/Gemini) incremental surface.

5. **CLEANUP** after successful integration: `git worktree remove <path>` and `git branch -D hive/{session_id}/worker-N`. (Backend also cleans on session completion — safe to leave if unsure.)

6. **CONFLICTS**: resolve in the main checkout, re-run the repository's relevant verification commands (from the plan, project DNA, and touched package/tooling) to confirm integrity, then commit the resolution.

7. **Commit & push** - You handle final commits (workers don't push)
{final_integration_step}

{post_workers_protocol}

Log each iteration to `{coordination_log_path}`:
```
{queen_quality_log}
```

After your orchestration objective is complete, transition to `idle` heartbeat status and continue checking your conversation file on heartbeat cadence.

## Your Task

{task}"#,
            hardening = hardening,
            branch_protocol = branch_protocol,
            required_protocol = required_protocol,
            session_id = session_id,
            cli = cli,
            prompts_dir = prompts_dir,
            tools_dir = tools_dir,
            queen_conversation = queen_conversation,
            shared_conversation = shared_conversation,
            worker_conversation_glob = worker_conversation_glob,
            plan_path = plan_path,
            lessons_dir = lessons_dir,
            coordination_log_path = coordination_log_path,
            worker_worktree_root = worker_worktree_root,
            queen_scope_rules = queen_scope_rules,
            plan_section = plan_section,
            worker_list = worker_list,
            qa_milestone_handoff = qa_milestone_handoff,
            post_workers_protocol = post_workers_protocol,
            queen_quality_log = Self::queen_quality_reconciliation_log_lines(has_evaluator),
            final_integration_step = final_integration_step,
            worker_worktrees_dir = worker_worktrees_dir,
            worker_task_file_example = worker_task_file_example,
            task = user_prompt.unwrap_or("Read the plan and begin coordinating workers.")
        )
    }

    /// Build a worker's role prompt
    fn build_worker_prompt(index: u8, config: &AgentConfig, queen_id: &str, session_id: &str) -> String {
        let role_name = config.role.as_ref()
            .map(|r| r.label.clone())
            .unwrap_or_else(|| format!("Worker {}", index));
        let scope_block = Self::scope_block(".");

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
                "reconciler" => "Deep-think reconciliation: collect Evaluator QA verdicts, CodeRabbit comments, and Gemini findings. Triage conflicts, deduplicate, and produce a unified fix list with priorities.",
                _ => "General development tasks as assigned.",
            })
            .unwrap_or("General development tasks as assigned.");

        let task_file = format!(".hive-manager/tasks/worker-{}-task.md", index);
        let polling_instructions = get_polling_instructions(
            &config.cli,
            &task_file,
            config.role.as_ref().map(|role| role.role_type.as_str()),
        );

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

{scope_block}

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

### File-Based Fallback

If curl returns exit code 7 (connection refused) or any non-zero exit, write directly to the conversation files instead:

```bash
# Append to queen's inbox (fallback)
echo -e "---\n[$(date -u +%Y-%m-%dT%H:%M:%SZ)] from @worker-{index}\nYour message here\n" >> ".hive-manager/{session_id}/conversations/queen.md"

# Append to shared channel (fallback)
echo -e "---\n[$(date -u +%Y-%m-%dT%H:%M:%SZ)] from @worker-{index}\nYour message here\n" >> ".hive-manager/{session_id}/conversations/shared.md"

# Read your inbox (fallback)
cat ".hive-manager/{session_id}/conversations/worker-{index}.md"

# Read shared broadcasts (fallback)
cat ".hive-manager/{session_id}/conversations/shared.md"
```

You MUST call the curl API first. Only use file fallback if curl fails.

## Learnings Protocol (MANDATORY)

Before marking your task COMPLETED, submit what you learned **via the HTTP API only**:

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

⚠️ **DO NOT write to `.ai-docs/learnings.jsonl` directly.** That file lives in your isolated worktree; direct writes are discarded when the worktree is cleaned up. The HTTP API is the only durable path — it writes to the session-scoped store the Queen consolidates at integration time.

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
            scope_block = scope_block,
            task_file = task_file,
            polling_instructions = polling_instructions
        )
    }

    /// Build a planner's prompt with HTTP API for spawning workers sequentially
    fn build_planner_prompt_with_http(
        project_path: &PathBuf,
        cli: &str,
        index: u8,
        config: &PlannerConfig,
        queen_id: &str,
        session_id: &str,
    ) -> String {
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
        let worker_task_file_example = project_path
            .join(".hive-manager")
            .join("worktrees")
            .join(session_id)
            .join("worker-N")
            .join(".hive-manager")
            .join("tasks")
            .join("worker-N-task.md")
            .to_string_lossy()
            .to_string();

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
  -d '{{"role_type": "ROLE", "cli": "{cli}", "name": "Worker N (Role)", "description": "TASK", "initial_task": "TASK", "parent_id": "{session_id}-planner-{index}"}}'
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

Each worker's own task file path inside its worktree is `.hive-manager/tasks/worker-N-task.md`.
When checking from your terminal, use the absolute path for that worker's worktree, for example:
```bash
# Read worker task file to check status
cat "{worker_task_file_example}" | grep "Status:"
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
            queen_id = queen_id,
            worker_task_file_example = worker_task_file_example
        )
    }

    /// Build the Queen's master prompt for Swarm mode with sequential planner spawning
    fn build_swarm_queen_prompt(
        cli: &str,
        project_path: &Path,
        session_id: &str,
        planners: &[PlannerConfig],
        user_prompt: Option<&str>,
        has_evaluator: bool,
    ) -> String {
        let planner_count = planners.len();
        let session_root = Self::session_root_path(project_path, session_id);
        let required_protocol = Self::queen_required_protocol(&session_root, has_evaluator);
        let post_workers_protocol =
            Self::queen_post_workers_protocol(session_id, &session_root, has_evaluator);

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
        let qa_milestone_handoff = if has_evaluator {
            Self::build_qa_milestone_handoff(session_id, &session_root, "workers/planners")
        } else {
            String::new()
        };

        format!(
r#"# Queen Agent - Swarm Session

You are the **Queen** orchestrating a multi-agent Swarm session. You spawn and coordinate Planners who each manage their own domain.
{hardening}
{required_protocol}

## Session Info

- **Session ID**: {session_id}
- **Mode**: Swarm (hierarchical with sequential spawning)
- **Prompts Directory**: `.hive-manager/{session_id}/prompts/`
- **Tools Directory**: `.hive-manager/{session_id}/tools/`

## Project Knowledge Intake

Before assigning work, read:
- `.ai-docs/project-dna.md`
- `.ai-docs/learnings.jsonl`

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

{qa_milestone_handoff}

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

{post_workers_protocol}

Log each iteration to `.hive-manager/{session_id}/coordination.log`:
```
{queen_quality_log}
```

## Your Task

{task}"#,
            hardening = hardening,
            required_protocol = required_protocol,
            session_id = session_id,
            cli = cli,
            planner_info = planner_info,
            planner_count = planner_count,
            qa_milestone_handoff = qa_milestone_handoff,
            post_workers_protocol = post_workers_protocol,
            queen_quality_log = Self::queen_quality_reconciliation_log_lines(has_evaluator),
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
        let worker_task_file_example = project_path
            .join(".hive-manager")
            .join("worktrees")
            .join(session_id)
            .join("worker-N")
            .join(".hive-manager")
            .join("tasks")
            .join("worker-N-task.md")
            .to_string_lossy()
            .to_string();
        let qa_task_file_example = format!(".hive-manager/{}/tasks/qa-worker-N-task.md", session_id);
        let worker_one_task_file_example = project_path
            .join(".hive-manager")
            .join("worktrees")
            .join(session_id)
            .join("worker-1")
            .join(".hive-manager")
            .join("tasks")
            .join("worker-1-task.md")
            .to_string_lossy()
            .to_string();

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
  "name": "Worker 2 (Frontend)",
  "description": "One-line task summary",
  "initial_task": "Optional task description"
}}
```

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| role_type | string | Yes | Worker role: backend, frontend, coherence, simplify, reviewer, resolver, tester, code-quality |
| cli | string | No | CLI to use: {default_cli} (default), gemini, codex, opencode, cursor, droid, qwen |
| name | string | No | Stable worker name; defaults to `Worker N (Role)` |
| description | string | No | One-line task summary used for deterministic labels |
| label | string | No | Legacy label field; kept as a fallback input |
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
  -d '{{"role_type": "frontend", "cli": "{default_cli}", "name": "Worker 2 (Frontend)", "description": "Implement the login form UI", "initial_task": "Implement the login form UI"}}'

# Spawn a reviewer worker
curl -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
  -H "Content-Type: application/json" \
  -d '{{"role_type": "reviewer", "cli": "{default_cli}", "name": "Worker 3 (Reviewer)", "description": "Review the current implementation"}}'
```

## Response

```json
{{
  "worker_id": "{session_id}-worker-N",
  "role": "Backend",
  "cli": "{default_cli}",
  "status": "Running",
  "task_file": "{worker_task_file_example}"
}}
```

## Notes

- Workers spawn in a new Windows Terminal tab (visible window)
- Each worker's own task file lives at `.hive-manager/tasks/worker-N-task.md` inside that worker's worktree
- Workers poll their task files for ACTIVE status
- Use this to spawn workers sequentially as tasks complete
"#, session_id = session_id, default_cli = default_cli, worker_task_file_example = worker_task_file_example);

        Self::write_tool_file(project_path, session_id, "spawn-worker.md", &spawn_worker_tool)?;

        let spawn_qa_worker_tool = format!(r#"# Spawn QA Worker Tool

Spawn a QA worker for the Evaluator.

## HTTP API

**Endpoint:** `POST http://localhost:18800/api/sessions/{session_id}/qa-workers`

**Headers:**
```
Content-Type: application/json
```

**Request Body:**
```json
{{
  "specialization": "ui",
  "cli": "{default_cli}",
  "initial_task": "Optional QA assignment"
}}
```

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| specialization | string | Yes | QA specialization: `ui`, `api`, or `a11y` |
| cli | string | No | CLI to use: {default_cli} (default), gemini, codex, opencode, cursor, droid, qwen |
| model | string | No | Optional model override |
| label | string | No | Custom label for the QA worker |
| initial_task | string | No | Initial QA assignment |
| parent_id | string | No | Parent evaluator ID (defaults to `{session_id}-evaluator`) |

## Example Usage

```bash
curl -X POST "http://localhost:18800/api/sessions/{session_id}/qa-workers" \
  -H "Content-Type: application/json" \
  -d '{{"specialization": "ui", "cli": "{default_cli}"}}'

curl -X POST "http://localhost:18800/api/sessions/{session_id}/qa-workers" \
  -H "Content-Type: application/json" \
  -d '{{"specialization": "api", "cli": "{default_cli}", "initial_task": "Validate milestone criteria 1-3 via HTTP requests"}}'
```

## Response

```json
{{
  "worker_id": "{session_id}-qa-worker-N",
  "role": "UI QA",
  "cli": "{default_cli}",
  "status": "Running",
  "task_file": "{qa_task_file_example}"
}}
```
"#, session_id = session_id, default_cli = default_cli, qa_task_file_example = qa_task_file_example);

        Self::write_tool_file(project_path, session_id, "spawn-qa-worker.md", &spawn_qa_worker_tool)?;

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
      "task_file": "{worker_one_task_file_example}"
    }}
  ],
  "count": 1
}}
```
"#, session_id = session_id, default_cli = default_cli, worker_one_task_file_example = worker_one_task_file_example);

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

    /// Write a task file for a worker (ACTIVE when pre-seeded with a task, otherwise STANDBY)
    fn write_task_file(worktree_path: &Path, worker_index: u8, initial_task: Option<&str>) -> Result<PathBuf, String> {
        let status = initial_task.map(|_| "ACTIVE");
        Self::write_task_file_with_status(worktree_path, worker_index, initial_task, status)
    }

    /// Write a task file with an optional status override (used for sequential spawning)
    fn write_task_file_with_status(
        worktree_path: &Path,
        worker_index: u8,
        initial_task: Option<&str>,
        status: Option<&str>,
    ) -> Result<PathBuf, String> {
        let tasks_dir = worktree_path.join(".hive-manager").join("tasks");
        std::fs::create_dir_all(&tasks_dir)
            .map_err(|e| format!("Failed to create tasks directory: {}", e))?;

        let file_path = Self::task_file_path_for_worker(worktree_path, worker_index as usize);
        let scope_block = Self::scope_block(".");
        let status = status.unwrap_or("STANDBY");

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

{scope_block}

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
            scope_block = scope_block,
            task_content = task_content,
            timestamp = timestamp
        );

        std::fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write task file: {}", e))?;

        Ok(file_path)
    }

    fn write_qa_task_file(
        project_path: &PathBuf,
        session_id: &str,
        worker_index: u8,
        specialization: &str,
        initial_task: Option<&str>,
    ) -> Result<PathBuf, String> {
        let tasks_dir = project_path.join(".hive-manager").join(session_id).join("tasks");
        std::fs::create_dir_all(&tasks_dir)
            .map_err(|e| format!("Failed to create tasks directory: {}", e))?;

        let filename = format!("qa-worker-{}-task.md", worker_index);
        let file_path = tasks_dir.join(&filename);

        let (status, task_content) = if let Some(task) = initial_task {
            ("ACTIVE", task.to_string())
        } else {
            ("STANDBY", "Awaiting QA assignment from the Evaluator. Monitor this file for updates.".to_string())
        };

        let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let content = format!(
"# Task Assignment - QA Worker {worker_index} ({specialization})

## Status: {status}

## Role Constraints

- **EXECUTOR**: You have full authority to test and verify behavior within your QA specialization.
- **SCOPE**: Stay within the assigned QA specialization and report criterion-numbered evidence.
- **GIT**: Do NOT push or commit. Provide evidence and findings for the Evaluator to act on.

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
            specialization = specialization,
            status = status,
            task_content = task_content,
            timestamp = timestamp
        );

        std::fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write QA task file: {}", e))?;

        Ok(file_path)
    }
    fn launch_solo_internal(
        &self,
        project_path: PathBuf,
        task_description: Option<String>,
        name: Option<String>,
        color: Option<String>,
        cli: String,
        model: Option<String>,
        flags: Vec<String>,
        with_evaluator: bool,
        evaluator_config: Option<AgentConfig>,
        qa_workers: Option<Vec<QaWorkerConfig>>,
        smoke_test: bool,
    ) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();
        let base_ref = resolve_fresh_base(&project_path);
        let solo_branch = format!("solo/{}/worker-1", session_id);
        let mut created_cells = Vec::new();
        let mut spawned_agent_ids = Vec::new();
        let (_, solo_cwd) = create_session_worktree(
            &session_id,
            "worker-1",
            &solo_branch,
            &base_ref,
            &project_path,
        )?;
        created_cells.push(("worker-1".to_string(), solo_branch.clone()));
        self.emit_workspace_created(
            &session_id,
            PRIMARY_CELL_ID,
            &solo_branch,
            Some(&solo_cwd),
        );
        let solo_name = "Solo Worker".to_string();
        let solo_description = Self::summarize_prompt_line(task_description.as_deref())
            .unwrap_or_else(|| "Solo session".to_string());
        let solo_config = AgentConfig {
            cli: cli.clone(),
            model: model.clone(),
            flags,
            label: Some(Self::derive_worker_label(&solo_name, &solo_description)),
            name: Some(solo_name),
            description: Some(solo_description),
            role: None,
            initial_prompt: task_description.clone(),
        };
        let (cmd, args) = Self::build_solo_command(&solo_config, task_description.as_deref());
        let solo_id = format!("{}-worker-1", session_id);

        {
            let pty_manager = self.pty_manager.read();
            if let Err(e) = pty_manager.create_session(
                solo_id.clone(),
                AgentRole::Worker { index: 1, parent: None },
                &cmd,
                &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                Some(&solo_cwd),
                120,
                30,
            ) {
                self.rollback_launch_allocations(
                    &project_path,
                    &session_id,
                    &created_cells,
                    &spawned_agent_ids,
                );
                return Err(format!("Failed to spawn solo agent: {}", e));
            }
        }
        spawned_agent_ids.push(solo_id.clone());

        let (max_qa_iterations, qa_timeout_secs, auth_strategy) = default_session_qa_settings();
        let session = Session {
            id: session_id.clone(),
            name,
            color,
            project_path: project_path.clone(),
            session_type: SessionType::Solo {
                cli: cli.clone(),
                model: model.clone(),
            },
            state: SessionState::Running,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents: vec![AgentInfo {
                id: solo_id,
                role: AgentRole::Worker { index: 1, parent: None },
                status: AgentStatus::Running,
                config: solo_config.clone(),
                parent_id: None,
                commit_sha: None,
                base_commit_sha: None,
            }],
            default_cli: cli,
            default_model: model,
            qa_workers: qa_workers.clone().unwrap_or_default(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: Some(solo_cwd.clone()),
            worktree_branch: Some(solo_branch.clone()),
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        self.emit_agent_batch_launched(&session, &session.agents);

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        self.init_session_storage(&session);
        self.spawn_launch_evaluator_agents(
            &session.id,
            with_evaluator,
            evaluator_config,
            qa_workers.as_deref(),
            smoke_test,
        )
        .map_err(|err| {
            {
                let mut heartbeats = self.agent_heartbeats.write();
                heartbeats.remove(&session.id);
            }
            {
                let mut sessions = self.sessions.write();
                sessions.remove(&session.id);
            }
            if let Some(storage) = self.storage.as_ref() {
                if let Err(delete_err) = storage.delete_session(&session_id) {
                    eprintln!(
                        "Failed to delete persisted session {} after evaluator launch error: {}",
                        session_id, delete_err
                    );
                }
            }
            self.rollback_launch_allocations(
                &project_path,
                &session_id,
                &created_cells,
                &spawned_agent_ids,
            );
            err
        })?;

        self.get_session(&session_id)
            .ok_or_else(|| format!("Session disappeared after evaluator launch: {}", session_id))
    }

    pub fn launch_solo(&self, config: HiveLaunchConfig) -> Result<Session, String> {
        let project_path = PathBuf::from(&config.project_path);
        let task_description = config
            .prompt
            .clone()
            .or_else(|| config.queen_config.initial_prompt.clone());

        self.launch_solo_internal(
            project_path.clone(),
            task_description,
            config.name.clone(),
            config.color.clone(),
            config.queen_config.cli.clone(),
            config.queen_config.model.clone(),
            config.queen_config.flags.clone(),
            config.with_evaluator,
            config.evaluator_config.clone(),
            config.qa_workers.clone(),
            config.smoke_test,
        )
    }

    pub fn launch_hive_v2(&self, config: HiveLaunchConfig) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();
        let mut agents = Vec::new();
        let project_path = PathBuf::from(&config.project_path);
        let mut created_cells = Vec::new();
        let mut spawned_agent_ids = Vec::new();

        // If with_planning is true, spawn Master Planner first
        if config.with_planning {
            return self.launch_planning_phase(session_id, config);
        }

        // Solo mode: skip orchestration and launch one agent directly.
        if config.workers.is_empty() {
            return self.launch_solo(config);
        }

        // Fetch latest from origin so all worktrees branch from the most
        // recent remote state, avoiding stale-base divergence.
        let base_ref = resolve_fresh_base(&project_path);

        // Create Queen agent
        let queen_id = format!("{}-queen", session_id);
        let (cmd, mut args) = Self::build_command(&config.queen_config);
        let queen_branch = format!("hive/{}/queen", session_id);
        let (_, queen_cwd) = create_session_worktree(
            &session_id,
            "queen",
            &queen_branch,
            &base_ref,
            &project_path,
        )?;
        created_cells.push(("queen".to_string(), queen_branch.clone()));
        self.emit_workspace_created(
            &session_id,
            PRIMARY_CELL_ID,
            &queen_branch,
            Some(&queen_cwd),
        );

        // Check if plan.md exists (from previous planning phase)
        let plan_path = project_path.join(".hive-manager").join(&session_id).join("plan.md");
        let has_plan = plan_path.exists();

        // Write Queen prompt to file and pass to CLI
        let master_prompt = Self::build_queen_master_prompt(
            &config.queen_config.cli,
            &project_path,
            queen_cwd.as_ref(),
            &session_id,
            &config.workers,
            config.prompt.as_deref(),
            has_plan,
            config.with_evaluator,
        );
        let prompt_file = match Self::write_prompt_file(&project_path, &session_id, "queen-prompt.md", &master_prompt) {
            Ok(prompt_file) => prompt_file,
            Err(err) => {
                self.rollback_launch_allocations(&project_path, &session_id, &created_cells, &spawned_agent_ids);
                return Err(err);
            }
        };
        let prompt_path = prompt_file.to_string_lossy().to_string();
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

        // Write tool documentation files
        if let Err(err) = Self::write_tool_files(&project_path, &session_id, &config.queen_config.cli) {
            self.rollback_launch_allocations(&project_path, &session_id, &created_cells, &spawned_agent_ids);
            return Err(err);
        }

        tracing::info!("Launching Queen agent (v2): {} {:?} in {:?}", cmd, args, queen_cwd);

        {
            let pty_manager = self.pty_manager.read();
            if let Err(e) = pty_manager.create_session(
                queen_id.clone(),
                AgentRole::Queen,
                &cmd,
                &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                Some(&queen_cwd),
                120,
                30,
            ) {
                self.rollback_launch_allocations(&project_path, &session_id, &created_cells, &spawned_agent_ids);
                return Err(format!("Failed to spawn Queen: {}", e));
            }
        }
        spawned_agent_ids.push(queen_id.clone());

        agents.push(AgentInfo {
            id: queen_id.clone(),
            role: AgentRole::Queen,
            status: AgentStatus::Running,
            config: config.queen_config.clone(),
            parent_id: None,
            commit_sha: None,
            base_commit_sha: None,
        });

        // Create Worker agents
        for (i, worker_config) in config.workers.iter().enumerate() {
            let index = (i + 1) as u8;
            let worker_id = format!("{}-worker-{}", session_id, index);
            let worker_role = worker_config.role.clone().unwrap_or_else(|| {
                WorkerRole::new("general", "Worker", &worker_config.cli)
            });
            let worker_config =
                Self::apply_worker_identity(index, &worker_role, worker_config.clone());
            let (cmd, mut args) = Self::build_command(&worker_config);
            let worker_branch = format!("hive/{}/worker-{}", session_id, index);
            let worker_cell_id = format!("worker-{}", index);
            let (_, worker_cwd) = match create_session_worktree(
                &session_id,
                &worker_cell_id,
                &worker_branch,
                &base_ref,
                &project_path,
            ) {
                Ok(result) => result,
                Err(err) => {
                    self.rollback_launch_allocations(&project_path, &session_id, &created_cells, &spawned_agent_ids);
                    return Err(err);
                }
            };
            created_cells.push((worker_cell_id.clone(), worker_branch.clone()));
            self.emit_workspace_created(
                &session_id,
                PRIMARY_CELL_ID,
                &worker_branch,
                Some(&worker_cwd),
            );

            // Write task file for this worker (STANDBY or with initial task)
            if let Err(err) =
                Self::write_task_file(Path::new(&worker_cwd), index, worker_config.initial_prompt.as_deref())
            {
                self.rollback_launch_allocations(&project_path, &session_id, &created_cells, &spawned_agent_ids);
                return Err(err);
            }

            // Write worker prompt to file and pass to CLI
            let worker_prompt = Self::build_worker_prompt(index, &worker_config, &queen_id, &session_id);
            let filename = format!("worker-{}-prompt.md", index);
            let prompt_file = match Self::write_prompt_file(&project_path, &session_id, &filename, &worker_prompt) {
                Ok(prompt_file) => prompt_file,
                Err(err) => {
                    self.rollback_launch_allocations(&project_path, &session_id, &created_cells, &spawned_agent_ids);
                    return Err(err);
                }
            };
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!("Launching Worker {} agent (v2): {} {:?} in {:?}", index, cmd, args, worker_cwd);

            {
                let pty_manager = self.pty_manager.read();
                if let Err(e) = pty_manager.create_session(
                    worker_id.clone(),
                    AgentRole::Worker { index, parent: Some(queen_id.clone()) },
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(&worker_cwd),
                    120,
                    30,
                ) {
                    self.rollback_launch_allocations(&project_path, &session_id, &created_cells, &spawned_agent_ids);
                    return Err(format!("Failed to spawn Worker {}: {}", index, e));
                }
            }
            spawned_agent_ids.push(worker_id.clone());

            agents.push(AgentInfo {
                id: worker_id,
                role: AgentRole::Worker { index, parent: Some(queen_id.clone()) },
                status: AgentStatus::Running,
                config: worker_config.clone(),
                parent_id: Some(queen_id.clone()),
                commit_sha: None,
                base_commit_sha: None,
            });
        }

        let (max_qa_iterations, qa_timeout_secs, auth_strategy) = default_session_qa_settings();
        let session = Session {
            id: session_id.clone(),
            name: config.name.clone(),
            color: config.color.clone(),
            session_type: SessionType::Hive { worker_count: config.workers.len() as u8 },
            project_path: project_path.clone(),
            state: SessionState::Running,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli: config.queen_config.cli.clone(),
            default_model: config.queen_config.model.clone(),
            qa_workers: config.qa_workers.clone().unwrap_or_default(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: Some(queen_cwd.clone()),
            worktree_branch: Some(queen_branch.clone()),
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        self.emit_agent_batch_launched(&session, &session.agents);

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        // Initialize session storage
        self.init_session_storage(&session);
        self.ensure_task_watcher(&session.id, &session.project_path);
        self.spawn_launch_evaluator_agents(
            &session.id,
            config.with_evaluator,
            config.evaluator_config.clone(),
            config.qa_workers.as_deref(),
            config.smoke_test,
        )
        .map_err(|err| {
            {
                let mut watchers = self.task_watchers.lock();
                let _ = watchers.remove(&session.id);
            }
            {
                let mut heartbeats = self.agent_heartbeats.write();
                heartbeats.remove(&session.id);
            }
            {
                let mut sessions = self.sessions.write();
                sessions.remove(&session.id);
            }
            self.rollback_launch_allocations(&project_path, &session_id, &created_cells, &spawned_agent_ids);
            err
        })?;

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
            let task_file = Self::fusion_variant_task_file_path(
                Path::new(&worktree_path),
                index as usize,
            )
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

        let (max_qa_iterations, qa_timeout_secs, auth_strategy) = default_session_qa_settings();
        let session = Session {
            id: session_id.clone(),
            name: config.name.clone(),
            color: config.color.clone(),
            session_type: SessionType::Fusion {
                variants: variants.iter().map(|v| v.name.clone()).collect(),
            },
            project_path: project_path.clone(),
            state: SessionState::Starting,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents: Vec::new(),
            default_cli: default_cli.clone(),
            default_model: config.default_model.clone(),
            qa_workers: Vec::new(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: variants.first().map(|v| v.worktree_path.clone()),
            worktree_branch: variants.first().map(|v| v.branch.clone()),
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session);
        }
        self.emit_session_update(&session_id);

        let fresh_base = resolve_fresh_base(&project_path);
        let base_branch = format!("fusion/{}/base", session_id);
        Self::run_git_in_dir(&project_path, &["branch", &base_branch, &fresh_base])?;

        for (variant_idx, variant) in variants.iter().enumerate() {
            let spawning_changes = {
                let mut sessions = self.sessions.write();
                if let Some(s) = sessions.get_mut(&session_id) {
                    Some(self.set_session_state_with_events(
                        s,
                        SessionState::SpawningFusionVariant(variant.index),
                    ))
                } else {
                    None
                }
            };
            if let Some(changes) = spawning_changes {
                self.emit_cell_status_changes(&session_id, changes);
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
            self.emit_workspace_created(
                &session_id,
                &variant_to_cell_id(&variant.name),
                &variant.branch,
                Some(&variant.worktree_path),
            );

            Self::write_fusion_variant_task_file(
                Path::new(&variant.worktree_path),
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
                name: None,
                description: None,
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
                commit_sha: None,
                base_commit_sha: None,
            };

            let waiting_changes = {
                let mut sessions = self.sessions.write();
                if let Some(s) = sessions.get_mut(&session_id) {
                    s.agents.push(agent_info.clone());
                    self.emit_agent_launched(s, &agent_info);
                    Some(self.set_session_state_with_events(
                        s,
                        SessionState::WaitingForFusionVariants,
                    ))
                } else {
                    None
                }
            };
            if let Some(changes) = waiting_changes {
                self.emit_cell_status_changes(&session_id, changes);
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
            Self::build_smoke_test_prompt(&session_id, &config.workers, config.with_evaluator, config.qa_workers.as_deref())
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
                commit_sha: None,
                base_commit_sha: None,
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

        let (max_qa_iterations, qa_timeout_secs, auth_strategy) = default_session_qa_settings();
        let session = Session {
            id: session_id.clone(),
            name: config.name.clone(),
            color: config.color.clone(),
            session_type: SessionType::Hive { worker_count: config.workers.len() as u8 },
            project_path,
            state: SessionState::Planning,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli: config.queen_config.cli.clone(),
            default_model: config.queen_config.model.clone(),
            qa_workers: config.qa_workers.clone().unwrap_or_default(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: None,
            worktree_branch: None,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        self.emit_agent_batch_launched(&session, &session.agents);

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
                commit_sha: None,
                base_commit_sha: None,
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
        let (max_qa_iterations, qa_timeout_secs, auth_strategy) = default_session_qa_settings();
        let session = Session {
            id: session_id.clone(),
            name: config.name.clone(),
            color: config.color.clone(),
            session_type: SessionType::Fusion { variants: variant_names },
            project_path: project_path.clone(),
            state: SessionState::Planning,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli: if config.default_cli.trim().is_empty() { "claude".to_string() } else { config.default_cli.trim().to_string() },
            default_model: config.default_model.clone(),
            qa_workers: Vec::new(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: None,
            worktree_branch: None,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        self.emit_agent_batch_launched(&session, &session.agents);

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
            let task_file = Self::fusion_variant_task_file_path(
                Path::new(&worktree_path),
                index as usize,
            )
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
        let fresh_base = resolve_fresh_base(&session.project_path);
        let base_branch = format!("fusion/{}/base", session_id);
        Self::run_git_in_dir(&session.project_path, &["branch", &base_branch, &fresh_base])?;

        let mut new_agents = Vec::new();

        // Spawn Queen agent
        let queen_cfg = config.queen_config.as_ref().unwrap_or(&config.judge_config).clone();
        {
            let pty_manager = self.pty_manager.read();

            let queen_id = format!("{}-queen", session_id);
            let (cmd, mut args) = Self::build_command(&queen_cfg);

            let queen_prompt = Self::build_fusion_queen_prompt(
                &queen_cfg.cli,
                &session.project_path,
                session_id,
                &variants,
                &config.task_description,
                false, // Fusion sessions never launch an Evaluator
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
                commit_sha: None,
                base_commit_sha: None,
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
            self.emit_workspace_created(
                session_id,
                &variant_to_cell_id(&variant.name),
                &variant.branch,
                Some(&variant.worktree_path),
            );

            Self::write_fusion_variant_task_file(
                Path::new(&variant.worktree_path),
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
                name: None,
                description: None,
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
                commit_sha: None,
                base_commit_sha: None,
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
        let (updated_session, changes) = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.agents.extend(new_agents.clone());
                if let Some(v) = variants.first() {
                    s.worktree_path = Some(v.worktree_path.clone());
                    s.worktree_branch = Some(v.branch.clone());
                }
                self.emit_agent_batch_launched(s, &new_agents);
                let changes =
                    self.set_session_state_with_events(s, SessionState::WaitingForFusionVariants);
                (s.clone(), changes)
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
        self.emit_cell_status_changes(session_id, changes);
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
            Self::build_swarm_smoke_test_prompt(&session_id, planner_count, &config.workers_per_planner, config.with_evaluator, config.qa_workers.as_deref())
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
                commit_sha: None,
                base_commit_sha: None,
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

        let (max_qa_iterations, qa_timeout_secs, auth_strategy) = default_session_qa_settings();
        let session = Session {
            id: session_id.clone(),
            name: config.name.clone(),
            color: config.color.clone(),
            session_type: SessionType::Swarm { planner_count: if config.planners.is_empty() { config.planner_count } else { config.planners.len() as u8 } },
            project_path,
            state: SessionState::Planning,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli: config.queen_config.cli.clone(),
            default_model: config.queen_config.model.clone(),
            qa_workers: config.qa_workers.clone().unwrap_or_default(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: None,
            worktree_branch: None,
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
            let changes = {
                let mut sessions = self.sessions.write();
                sessions
                    .get_mut(session_id)
                    .map(|s| self.set_session_state_with_events(s, SessionState::Running))
            };
            if let Some(changes) = changes {
                self.persist_then_emit_session_update(session_id, changes)
                    .map_err(SessionError::ConfigError)?;
            }
            return Ok(());
        }

        let worker_config = &config.workers[worker_index];
        let index = (worker_index + 1) as u8;
        let worker_branch = format!("hive/{}/worker-{}", session_id, index);
        let previous_state = self
            .sessions
            .read()
            .get(session_id)
            .map(|s| s.state.clone())
            .ok_or_else(|| SessionError::NotFound(format!("Session not found: {}", session_id)))?;

        // Update state to spawning this worker
        let spawning_changes = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                Some(self.set_session_state_with_events(
                    s,
                    SessionState::SpawningWorker(index),
                ))
            } else {
                None
            }
        };
        if let Some(changes) = spawning_changes {
            self.persist_then_emit_session_update(session_id, changes)
                .map_err(SessionError::ConfigError)?;
        }

        let base_ref = Self::resolve_worker_base_ref(&session, "spawn_next_worker", index);
        let worker_cell_name = format!("worker-{index}");
        let worker_id = format!("{}-worker-{}", session_id, index);

        // 1. Create worker worktree FIRST (before writing task/prompt files)
        let (_, worker_cwd) = create_session_worktree(
            session_id,
            &worker_cell_name,
            &worker_branch,
            &base_ref,
            &session.project_path,
        )
        .map_err(|err| {
            self.restore_session_state_after_worker_spawn_failure(session_id, &previous_state);
            SessionError::ConfigError(err)
        })?;
        let task_file_path = Self::task_file_path_for_worker(Path::new(&worker_cwd), index as usize);
        let worker_base_commit_sha = current_head(Path::new(&worker_cwd)).map_err(|err| {
            Self::rollback_worker_launch_artifacts(
                &session.project_path,
                session_id,
                &worker_cell_name,
                &task_file_path,
                None,
            );
            self.restore_session_state_after_worker_spawn_failure(session_id, &previous_state);
            SessionError::ConfigError(format!(
                "Failed to snapshot worker base commit for worker {}: {}",
                index, err
            ))
        })?;
        self.emit_workspace_created(
            session_id,
            PRIMARY_CELL_ID,
            &worker_branch,
            Some(&worker_cwd),
        );
        let filename = format!("worker-{}-prompt.md", index);
        let prompt_file_path = session
            .project_path
            .join(".hive-manager")
            .join(session_id)
            .join("prompts")
            .join(&filename);

        // 2. Write task file (Status: ACTIVE since it's their turn)
        Self::write_task_file_with_status(
            Path::new(&worker_cwd),
            index,
            worker_config.initial_prompt.as_deref(),
            Some("ACTIVE"),
        )
        .map_err(|err| {
            Self::rollback_worker_launch_artifacts(
                &session.project_path,
                session_id,
                &worker_cell_name,
                &task_file_path,
                None,
            );
            self.restore_session_state_after_worker_spawn_failure(session_id, &previous_state);
            SessionError::ConfigError(err)
        })?;

        // 3. Write worker prompt to file
        let worker_prompt = Self::build_worker_prompt(index, worker_config, queen_id, session_id);
        let prompt_file =
            Self::write_prompt_file(&session.project_path, session_id, &filename, &worker_prompt)
                .map_err(|err| {
                    Self::rollback_worker_launch_artifacts(
                        &session.project_path,
                        session_id,
                        &worker_cell_name,
                        &task_file_path,
                        Some(&prompt_file_path),
                    );
                    self.restore_session_state_after_worker_spawn_failure(session_id, &previous_state);
                    SessionError::ConfigError(err)
                })?;
        let prompt_path = prompt_file.to_string_lossy().to_string();

        // 4. Build command with prompt
        let (cmd, mut args) = Self::build_command(worker_config);
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

        // 5. Spawn the worker (use worker_cwd as PTY cwd)
        let pty_manager = self.pty_manager.read();
        pty_manager
            .create_session(
                worker_id.clone(),
                AgentRole::Worker { index, parent: Some(queen_id.to_string()) },
                &cmd,
                &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                Some(&worker_cwd),
                120,
                30,
            )
            .map_err(|e| {
                Self::rollback_worker_launch_artifacts(
                    &session.project_path,
                    session_id,
                    &worker_cell_name,
                    &task_file_path,
                    Some(&prompt_file_path),
                );
                self.restore_session_state_after_worker_spawn_failure(session_id, &previous_state);
                SessionError::SpawnError(format!("Failed to spawn Worker {}: {}", index, e))
            })?;

        // 5. Add worker to session
        let waiting_changes = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                let agent = AgentInfo {
                    id: worker_id,
                    role: AgentRole::Worker { index, parent: Some(queen_id.to_string()) },
                    status: AgentStatus::Running,
                    config: worker_config.clone(),
                    parent_id: Some(queen_id.to_string()),
                    commit_sha: None,
                    base_commit_sha: Some(worker_base_commit_sha.clone()),
                };
                s.agents.push(agent.clone());
                self.emit_agent_launched(s, &agent);
                Some(self.set_session_state_with_events(
                    s,
                    SessionState::WaitingForWorker(index),
                ))
            } else {
                None
            }
        };
        if let Some(changes) = waiting_changes {
            self.persist_then_emit_session_update(session_id, changes)
                .map_err(SessionError::ConfigError)?;
        }

        Ok(())
    }

    fn resolve_worker_base_ref(session: &Session, log_context: &str, worker_index: u8) -> String {
        let maybe_worktree_head = session.worktree_path.as_ref().and_then(|worktree_path| {
            match current_head(Path::new(worktree_path)) {
                Ok(sha) => {
                    tracing::info!(
                        "{}: using session worktree HEAD {} as base for worker {} in session {}",
                        log_context,
                        sha,
                        worker_index,
                        session.id
                    );
                    Some(sha)
                }
                Err(err) => {
                    tracing::warn!(
                        "{}: failed to read session worktree HEAD at {} for session {}: {}; falling back to project HEAD",
                        log_context,
                        worktree_path,
                        session.id,
                        err
                    );
                    None
                }
            }
        });

        maybe_worktree_head.unwrap_or_else(|| match current_head(&session.project_path) {
            Ok(sha) => {
                tracing::info!(
                    "{}: using project HEAD {} as base for worker {} in session {}",
                    log_context,
                    sha,
                    worker_index,
                    session.id
                );
                sha
            }
            Err(err) => {
                let fresh_base = resolve_fresh_base(&session.project_path);
                tracing::info!(
                    "{}: using resolve_fresh_base {} for worker {} in session {} after project HEAD lookup failed: {}",
                    log_context,
                    fresh_base,
                    worker_index,
                    session.id,
                    err
                );
                fresh_base
            }
        })
    }

    fn require_commit_sha_gate_enabled() -> bool {
        std::env::var("REQUIRE_COMMIT_SHA")
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    }

    fn worker_base_commit_sha(session: &Session, worker_id: u8) -> Option<String> {
        session.agents.iter().find_map(|agent| match &agent.role {
            AgentRole::Worker { index, .. } if *index == worker_id => agent.base_commit_sha.clone(),
            _ => None,
        })
    }

    fn worker_completion_commit_sha(session: &Session, worker_id: u8) -> Option<String> {
        let worker_worktree = session
            .project_path
            .join(".hive-manager")
            .join("worktrees")
            .join(&session.id)
            .join(format!("worker-{worker_id}"));
        let head = match current_head(&worker_worktree) {
            Ok(sha) => sha,
            Err(err) => {
                tracing::warn!(
                    session_id = %session.id,
                    worker_id,
                    worktree = %worker_worktree.display(),
                    error = %err,
                    "Unable to resolve worker HEAD for completion gate"
                );
                return None;
            }
        };

        if let Some(base_commit_sha) = Self::worker_base_commit_sha(session, worker_id) {
            return if head == base_commit_sha { None } else { Some(head) };
        }

        let base_ref = Self::resolve_worker_base_ref(session, "worker_completion_commit_sha", worker_id);
        if head == base_ref { None } else { Some(head) }
    }

    pub(crate) fn sync_agent_commit_sha(&self, session_id: &str, agent_id: &str, commit_sha: Option<String>) {
        let updated = {
            let mut sessions = self.sessions.write();
            let Some(session) = sessions.get_mut(session_id) else {
                return;
            };
            let Some(agent) = session.agents.iter_mut().find(|agent| agent.id == agent_id) else {
                return;
            };
            if agent.commit_sha == commit_sha {
                false
            } else {
                agent.commit_sha = commit_sha;
                true
            }
        };

        if updated {
            self.update_session_storage(session_id);
            self.emit_session_update(session_id);
        }
    }

    fn apply_qa_verdict_to_session(
        &self,
        session: &mut Session,
        normalized_verdict: &str,
        evaluator_id: Option<&str>,
        commit_sha: Option<&str>,
    ) -> (SessionState, Vec<(String, String, String)>) {
        if let Some(evaluator_id) = evaluator_id {
            if let Some(agent) = session.agents.iter_mut().find(|agent| agent.id == evaluator_id) {
                agent.commit_sha = commit_sha.map(str::to_string);
            }
        }

        match normalized_verdict {
            "PASS" | "QA_VERDICT: PASS" => {
                let changes = self.set_session_state_with_events(session, SessionState::QaPassed);
                (SessionState::QaPassed, changes)
            }
            "FAIL" | "QA_VERDICT: FAIL" => {
                let next_iteration = next_qa_failure_iteration(&session.state);

                if next_iteration > session.max_qa_iterations {
                    tracing::warn!(
                        "Session {} exhausted the QA safety ceiling at {} failed verdicts",
                        session.id,
                        session.max_qa_iterations
                    );
                    let changes =
                        self.set_session_state_with_events(session, SessionState::QaMaxRetriesExceeded);
                    session.auth_strategy = AuthStrategy::None;
                    (SessionState::QaMaxRetriesExceeded, changes)
                } else {
                    let next_state = SessionState::QaFailed {
                        iteration: next_iteration,
                    };
                    let changes = self.set_session_state_with_events(session, next_state.clone());
                    (next_state, changes)
                }
            }
            _ => unreachable!("unsupported verdict already validated"),
        }
    }

    pub fn record_http_qa_verdict(
        &self,
        session_id: &str,
        evaluator_id: &str,
        verdict: &str,
        commit_sha: Option<&str>,
    ) -> Result<SessionState, String> {
        let normalized = verdict.trim().to_ascii_uppercase();
        if !matches!(
            normalized.as_str(),
            "PASS" | "QA_VERDICT: PASS" | "FAIL" | "QA_VERDICT: FAIL"
        ) {
            return Err(format!("Unsupported QA verdict '{}'", verdict));
        }

        let (previous_session, updated_session, changes, new_state) = {
            let mut sessions = self.sessions.write();
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?;
            // Defense-in-depth: reject verdicts outside QaInProgress so HTTP callers
            // can't jump Running straight to QaPassed/QaFailed, bypassing the
            // milestone-ready -> QA transition. Force-pass/force-fail uses a
            // separate path (apply_verdict -> on_qa_verdict) with broader semantics.
            if !matches!(session.state, SessionState::QaInProgress { .. }) {
                return Err(format!(
                    "Cannot record QA verdict: session is in {:?} state, expected QaInProgress",
                    session.state
                ));
            }
            let previous_session = session.clone();
            let now = Utc::now();
            if now > session.last_activity_at {
                session.last_activity_at = now;
            }
            let (new_state, changes) = self.apply_qa_verdict_to_session(
                session,
                normalized.as_str(),
                Some(evaluator_id),
                commit_sha,
            );
            (previous_session, session.clone(), changes, new_state)
        };

        if let Some(storage) = self.storage.as_ref() {
            if let Err(err) = Self::persist_session_snapshot(storage, &updated_session, session_id) {
                let mut sessions = self.sessions.write();
                if let Some(session) = sessions.get_mut(session_id) {
                    *session = previous_session;
                }
                return Err(err);
            }
        }

        self.cancel_qa_timeout(session_id);

        self.emit_session_update(session_id);
        self.emit_cell_status_changes(session_id, changes);

        Ok(new_state)
    }

    fn try_begin_evaluator_respawn(&self, session_id: &str) -> bool {
        let mut inflight = self.evaluator_respawns_inflight.lock();
        inflight.insert(session_id.to_string())
    }

    fn finish_evaluator_respawn(&self, session_id: &str) {
        let mut inflight = self.evaluator_respawns_inflight.lock();
        inflight.remove(session_id);
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

        let worker_agent_id = format!("{}-worker-{}", session_id, worker_id);
        let commit_sha_session = session.clone();
        let worker_commit_sha = tokio::task::spawn_blocking(move || {
            Self::worker_completion_commit_sha(&commit_sha_session, worker_id)
        })
        .await
        .map_err(|err| {
            SessionError::ConfigError(format!(
                "Failed to resolve worker commit SHA for {} worker {}: {}",
                session_id, worker_id, err
            ))
        })?;
        self.sync_agent_commit_sha(session_id, &worker_agent_id, worker_commit_sha.clone());
        if Self::require_commit_sha_gate_enabled() && worker_commit_sha.is_none() {
            tracing::warn!(
                session_id = %session_id,
                worker_id,
                agent_id = %worker_agent_id,
                gate = "require_commit_sha",
                reason = "missing_commit_sha",
                "Rejecting worker completion transition"
            );
            return Err(SessionError::ConfigError(format!(
                "Worker {} completion rejected: commit SHA required before advancing the session",
                worker_id
            )));
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

    #[allow(dead_code)]
    pub fn on_milestone_ready(&self, session_id: &str) -> Result<(), String> {
        let (maybe_evaluator, config) = {
            let sessions = self.sessions.read();
            let session = sessions
                .get(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?;

            let maybe_evaluator = session
                .agents
                .iter()
                .find(|agent| matches!(agent.role, AgentRole::Evaluator))
                .cloned();

            let config = maybe_evaluator
                .as_ref()
                .map(|agent| agent.config.clone())
                .unwrap_or_else(|| AgentConfig {
                    cli: session.default_cli.clone(),
                    model: session.default_model.clone(),
                    flags: vec![],
                    label: Some("Evaluator".to_string()),
                    name: None,
                    description: None,
                    role: None,
                    initial_prompt: None,
                });

            (maybe_evaluator, config)
        };

        let evaluator_alive = maybe_evaluator
            .as_ref()
            .map(|agent| self.pty_manager.read().is_alive(&agent.id))
            .unwrap_or(false);

        if maybe_evaluator.is_none() || !evaluator_alive {
            if !self.try_begin_evaluator_respawn(session_id) {
                tracing::debug!(
                    session_id = %session_id,
                    "Ignoring duplicate milestone-ready signal while evaluator respawn is already in flight"
                );
                return Ok(());
            }

            tracing::info!(
                session_id = %session_id,
                reason = if maybe_evaluator.is_some() { "dead_evaluator" } else { "missing_evaluator" },
                "Launching evaluator from milestone-ready signal"
            );
            let result = self.launch_evaluator(session_id, config, false);
            self.finish_evaluator_respawn(session_id);
            result?;
            return Ok(());
        }

        let timeout_secs = {
            let mut sessions = self.sessions.write();
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?;
            let next_state = qa_in_progress_state(&session.state);
            let changes = self.set_session_state_with_events(session, next_state);
            (session.qa_timeout_secs, changes)
        };
        self.emit_session_update(session_id);
        self.update_session_storage(session_id);
        self.emit_cell_status_changes(session_id, timeout_secs.1);
        self.start_qa_timeout(session_id, timeout_secs.0);

        Ok(())
    }

    #[allow(dead_code)]
    pub fn on_qa_verdict(&self, session_id: &str, verdict: &str) -> Result<SessionState, String> {
        let normalized = verdict.trim().to_ascii_uppercase();
        if !matches!(
            normalized.as_str(),
            "PASS" | "QA_VERDICT: PASS" | "FAIL" | "QA_VERDICT: FAIL"
        ) {
            return Err(format!("Unsupported QA verdict '{}'", verdict));
        }

        self.cancel_qa_timeout(session_id);
        let (new_state, changes) = {
            let mut sessions = self.sessions.write();
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?;
            self.apply_qa_verdict_to_session(session, normalized.as_str(), None, None)
        };

        self.emit_session_update(session_id);
        self.update_session_storage(session_id);
        self.emit_cell_status_changes(session_id, changes);

        Ok(new_state)
    }

    #[allow(dead_code)]
    pub fn on_qa_timeout(&self, session_id: &str) -> Result<(), String> {
        tracing::warn!(
            "QA timed out for session {}; defaulting to pass-with-warning after {} seconds",
            session_id,
            self.get_session(session_id)
                .map(|session| session.qa_timeout_secs)
                .unwrap_or(DEFAULT_QA_TIMEOUT_SECS)
        );
        self.on_qa_verdict(session_id, "QA_VERDICT: PASS")?;

        // Emit qa-timeout event
        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("qa-timeout", serde_json::json!({
                "session_id": session_id,
                "action": "pass-with-warning"
            }));
        }

        Ok(())
    }

    /// Start a QA timeout timer. On expiry, auto-passes QA with a warning.
    /// Cancel by calling `cancel_qa_timeout`.
    pub fn start_qa_timeout(&self, session_id: &str, timeout_secs: u64) {
        // Cancel any existing timer
        self.cancel_qa_timeout(session_id);

        let sid = session_id.to_string();
        let sessions = Arc::clone(&self.sessions);
        let app_handle = self.app_handle.clone();
        let event_emitter = self.event_emitter.clone();
        let storage = self.storage.clone();

        let handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(timeout_secs)).await;

            // Check if still QaInProgress before timing out
            let is_qa = {
                let sessions = sessions.read();
                sessions
                    .get(&sid)
                    .map(|s| matches!(s.state, SessionState::QaInProgress { .. }))
                    .unwrap_or(false)
            };

            if is_qa {
                tracing::warn!("QA timeout fired for session {} after {}s — auto-passing", sid, timeout_secs);

                // A timeout auto-pass should leave the session in QaPassed so the server can enforce
                // the same quiescence gate as an explicit evaluator PASS.
                let transition = {
                    let mut sessions = sessions.write();
                    if let Some(session) = sessions.get_mut(&sid) {
                        let previous_state = session.state.clone();
                        let changes = cell_status_changes_for_transition(session, &SessionState::QaPassed);
                        session.state = SessionState::QaPassed;
                        Some((previous_state, changes, session.clone()))
                    } else {
                        None
                    }
                };

                if let Some((previous_state, changes, updated_session)) = transition {
                    if let Some(storage) = storage.as_ref() {
                        if let Err(error) =
                            SessionController::persist_session_snapshot(storage, &updated_session, &sid)
                        {
                            tracing::warn!("Failed to persist QA timeout state for {}: {}", sid, error);
                            let mut sessions = sessions.write();
                            if let Some(session) = sessions.get_mut(&sid) {
                                session.state = previous_state;
                            }
                            return;
                        }
                    }

                    if let Some(emitter) = event_emitter.clone() {
                        SessionController::fire_cell_status_changes(emitter, sid.clone(), changes);
                    }

                    if let Some(ref app_handle) = app_handle {
                        let _ = app_handle.emit("session-update", SessionUpdate {
                            session: updated_session,
                        });
                        let _ = app_handle.emit("qa-timeout", serde_json::json!({
                            "session_id": sid,
                            "action": "pass-with-warning"
                        }));
                    }
                }
            }
        });

        let abort_handle = handle.abort_handle();
        let mut handles = self.qa_timeout_handles.lock();
        handles.insert(session_id.to_string(), abort_handle);
    }

    /// Cancel a pending QA timeout timer
    pub fn cancel_qa_timeout(&self, session_id: &str) {
        let mut handles = self.qa_timeout_handles.lock();
        if let Some(handle) = handles.remove(session_id) {
            handle.abort();
        }
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

        let completed_agent = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                if let Some(index) = s.agents.iter().position(|agent| agent.id == variant.agent_id) {
                    s.agents[index].status = AgentStatus::Completed;
                    Some((s.clone(), s.agents[index].clone()))
                } else {
                    None
                }
            } else {
                None
            }
        };
        self.update_session_storage(session_id);
        if let Some((session, agent)) = completed_agent {
            self.emit_agent_completed(&session, &agent);
        }

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

        let spawning_changes = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                Some(self.set_session_state_with_events(s, SessionState::SpawningJudge))
            } else {
                None
            }
        };
        if let Some(changes) = spawning_changes {
            self.emit_cell_status_changes(session_id, changes);
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

        let judging_changes = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                let agent = AgentInfo {
                    id: judge_id,
                    role: AgentRole::Judge {
                        session_id: session_id.to_string(),
                    },
                    status: AgentStatus::Running,
                    config: judge_config,
                    parent_id: None,
                    commit_sha: None,
                    base_commit_sha: None,
                };
                s.agents.push(agent.clone());
                self.emit_agent_launched(s, &agent);
                Some(self.set_session_state_with_events(s, SessionState::Judging))
            } else {
                None
            }
        };
        self.emit_session_update(session_id);
        self.update_session_storage(session_id);
        if let Some(changes) = judging_changes {
            self.emit_cell_status_changes(session_id, changes);
        }

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
            let awaiting_changes = {
                let mut sessions = self.sessions.write();
                if let Some(s) = sessions.get_mut(session_id) {
                    if s.state == SessionState::Judging {
                        Some(self.set_session_state_with_events(
                            s,
                            SessionState::AwaitingVerdictSelection,
                        ))
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            if let Some(changes) = awaiting_changes {
                self.emit_session_update(session_id);
                self.update_session_storage(session_id);
                self.emit_cell_status_changes(session_id, changes);
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

        let merging_changes = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                Some(self.set_session_state_with_events(s, SessionState::MergingWinner))
            } else {
                None
            }
        };
        self.emit_session_update(session_id);
        self.update_session_storage(session_id);
        if let Some(changes) = merging_changes {
            self.emit_cell_status_changes(session_id, changes);
        }

        Self::run_git_in_dir(&session.project_path, &["merge", "--squash", &winner.branch])?;

        // Commit the squash merge (--squash only stages changes, doesn't commit)
        Self::run_git_in_dir(
            &session.project_path,
            &["commit", "-m", &format!("Merge fusion winner: {}", winner.name)],
        )?;

        for variant in &metadata.variants {
            let pty_manager = self.pty_manager.read();
            if let Err(err) = pty_manager.kill(&variant.agent_id) {
                tracing::warn!("Failed to stop variant agent {}: {}", variant.agent_id, err);
            }
        }

        {
            let pty_manager = self.pty_manager.read();
            let judge_id = format!("{}-judge", session_id);
            let _ = pty_manager.kill(&judge_id);
        }

        let cleanup_result = cleanup_session_worktrees(&session);

        let completed_state = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                let completed_agents = s
                    .agents
                    .iter()
                    .filter(|agent| agent.status != AgentStatus::Completed)
                    .cloned()
                    .collect::<Vec<_>>();
                for agent in &mut s.agents {
                    agent.status = AgentStatus::Completed;
                }
                let changes = self.set_session_state_with_events(s, SessionState::Completed);
                s.auth_strategy = AuthStrategy::None;
                s.worktree_path = None;
                s.worktree_branch = None;
                Some((s.clone(), completed_agents, changes))
            } else {
                None
            }
        };
        self.emit_session_update(session_id);
        self.update_session_storage(session_id);
        if let Some((session, completed_agents, changes)) = completed_state {
            for agent in &completed_agents {
                self.emit_agent_completed(&session, agent);
            }
            self.emit_cell_status_changes(session_id, changes);
        }

        if cleanup_result.is_ok() {
            Ok(())
        } else {
            Err(format!(
                "Winner merged, but worktree cleanup had issues: {}",
                cleanup_result.unwrap_err()
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
        let completed_agent = {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                if let Some(index) = session
                    .agents
                    .iter()
                    .position(|agent| agent.id == worker_agent_id)
                {
                    session.agents[index].status = AgentStatus::Completed;
                    Some((session.clone(), session.agents[index].clone()))
                } else {
                    None
                }
            } else {
                None
            }
        };
        self.update_session_storage(session_id);
        if let Some((session, agent)) = completed_agent {
            self.emit_agent_completed(&session, &agent);
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
            let master_prompt = Self::build_queen_master_prompt(
                &config.queen_config.cli,
                &session.project_path,
                &session.project_path,
                session_id,
                &config.workers,
                config.prompt.as_deref(),
                has_plan,
                config.with_evaluator,
            );
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
                commit_sha: None,
                base_commit_sha: None,
            });

            // Queen will spawn workers via HTTP API after reading the plan
            // No auto-spawning of workers - Queen controls the flow
        }

        // Update session with new agents - Queen will spawn workers
        let (updated_session, changes) = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.agents.extend(new_agents.clone());
                self.emit_agent_batch_launched(s, &new_agents);
                // Set state to Running - Queen will spawn workers via HTTP API
                let changes = self.set_session_state_with_events(s, SessionState::Running);
                (s.clone(), changes)
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
        self.emit_cell_status_changes(session_id, changes);
        self.ensure_task_watcher(session_id, &updated_session.project_path);
        self.ensure_task_watcher(session_id, &updated_session.project_path);
        self.spawn_launch_evaluator_agents(
            session_id,
            config.with_evaluator,
            config.evaluator_config.clone(),
            config.qa_workers.as_deref(),
            config.smoke_test,
        )?;

        // Clean up pending config file
        let _ = std::fs::remove_file(&pending_config_path);

        Ok(updated_session)
    }

    /// Mark a planning session as ready (plan generated)
    pub fn mark_plan_ready(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.write();
        if let Some(session) = sessions.get_mut(session_id) {
            if session.state == SessionState::Planning {
                let changes = self.set_session_state_with_events(session, SessionState::PlanReady);

                if let Some(ref app_handle) = self.app_handle {
                    let _ = app_handle.emit("session-update", SessionUpdate {
                        session: session.clone(),
                    });
                }
                self.emit_cell_status_changes(session_id, changes);
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

        // Fail fast when storage isn't installed. Previously this fell back to a
        // locally-constructed SessionStorage, but self.storage (set via &mut self)
        // would still be None, so later update_session_storage_* calls silently
        // dropped writes for the resumed session.
        let storage = self
            .storage
            .clone()
            .ok_or_else(|| "Session storage is not initialized".to_string())?;
        let persisted = storage.load_session(session_id)
            .map_err(|e| format!("Failed to load session from storage: {}", e))?;
        storage
            .mark_session_synced(session_id, &persisted)
            .map_err(|e| format!("Failed to track session storage state: {}", e))?;

        // Convert persisted session to active session
        let session = self.session_from_persisted(&persisted)?;

        // Add to in-memory sessions
        {
            let mut sessions = self.sessions.write();
            sessions.insert(session.id.clone(), session.clone());
        }

        self.ensure_task_watcher(&session.id, &session.project_path);

        // Emit session-update event to frontend
        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        Ok(session)
    }

    fn session_from_persisted(
        &self,
        persisted: &crate::storage::PersistedSession,
    ) -> Result<Session, String> {
        let session_type = match &persisted.session_type {
            crate::storage::SessionTypeInfo::Hive { worker_count } => SessionType::Hive {
                worker_count: *worker_count,
            },
            crate::storage::SessionTypeInfo::Swarm { planner_count } => SessionType::Swarm {
                planner_count: *planner_count,
            },
            crate::storage::SessionTypeInfo::Fusion { variants } => SessionType::Fusion {
                variants: variants.clone(),
            },
            crate::storage::SessionTypeInfo::Solo { cli, model } => SessionType::Solo {
                cli: cli.clone(),
                model: model.clone(),
            },
        };

        let agents: Vec<AgentInfo> = persisted
            .agents
            .iter()
            .filter_map(|pa| {
                let role = parse_agent_role(&pa.role)?;
                let config = AgentConfig {
                    cli: pa.config.cli.clone(),
                    model: pa.config.model.clone(),
                    flags: pa.config.flags.clone(),
                    label: pa.config.label.clone(),
                    name: pa.config.name.clone(),
                    description: pa.config.description.clone(),
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
                    status: AgentStatus::Completed,
                    config,
                    parent_id: pa.parent_id.clone(),
                    commit_sha: pa.commit_sha.clone(),
                    base_commit_sha: pa.base_commit_sha.clone(),
                })
            })
            .collect();

        let state = parse_persisted_session_state(&persisted.state);
        let auth_strategy = if is_terminal_session_state(&state) {
            AuthStrategy::None
        } else {
            AuthStrategy::from_persisted(&persisted.auth_strategy)
        };

        Ok(Session {
            id: persisted.id.clone(),
            name: persisted.name.clone(),
            color: persisted.color.clone(),
            session_type,
            project_path: PathBuf::from(&persisted.project_path),
            state,
            created_at: persisted.created_at,
            last_activity_at: persisted
                .last_activity_at
                .unwrap_or(persisted.created_at),
            agents,
            default_cli: persisted.default_cli.clone(),
            default_model: persisted.default_model.clone(),
            qa_workers: persisted.qa_workers.clone(),
            max_qa_iterations: persisted.max_qa_iterations,
            qa_timeout_secs: persisted.qa_timeout_secs,
            auth_strategy,
            worktree_path: persisted.worktree_path.clone(),
            worktree_branch: persisted.worktree_branch.clone(),
        })
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
            let master_prompt = Self::build_swarm_queen_prompt(
                &config.queen_config.cli,
                &session.project_path,
                session_id,
                &planners,
                config.prompt.as_deref(),
                config.with_evaluator,
            );
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
                commit_sha: None,
                base_commit_sha: None,
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
        let (updated_session, changes) = {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                session.agents.extend(new_agents.clone());
                self.emit_agent_batch_launched(session, &new_agents);
                let changes = self.set_session_state_with_events(session, SessionState::Running);
                (session.clone(), changes)  // Queen will spawn planners sequentially
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
        self.emit_cell_status_changes(session_id, changes);
        self.ensure_task_watcher(session_id, &updated_session.project_path);
        self.spawn_launch_evaluator_agents(
            session_id,
            config.with_evaluator,
            config.evaluator_config.clone(),
            config.qa_workers.as_deref(),
            config.smoke_test,
        )?;

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
            let master_prompt = Self::build_swarm_queen_prompt(
                &config.queen_config.cli,
                &project_path,
                &session_id,
                &planners,
                config.prompt.as_deref(),
                config.with_evaluator,
            );
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
                commit_sha: None,
                base_commit_sha: None,
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

        let (max_qa_iterations, qa_timeout_secs, auth_strategy) = default_session_qa_settings();
        let session = Session {
            id: session_id.clone(),
            name: config.name.clone(),
            color: config.color.clone(),
            session_type: SessionType::Swarm { planner_count: planners.len() as u8 },
            project_path,
            state: SessionState::Running,  // Queen will spawn planners sequentially
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli: config.queen_config.cli.clone(),
            default_model: config.queen_config.model.clone(),
            qa_workers: config.qa_workers.clone().unwrap_or_default(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: None,
            worktree_branch: None,
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
        self.spawn_launch_evaluator_agents(
            &session.id,
            config.with_evaluator,
            config.evaluator_config.clone(),
            config.qa_workers.as_deref(),
            config.smoke_test,
        )?;

        Ok(session)
    }

    fn spawn_launch_evaluator_agents(
        &self,
        session_id: &str,
        with_evaluator: bool,
        evaluator_config: Option<AgentConfig>,
        qa_workers: Option<&[QaWorkerConfig]>,
        smoke_test: bool,
    ) -> Result<(), String> {
        if !with_evaluator {
            return Ok(());
        }

        let evaluator_config = evaluator_config.unwrap_or(AgentConfig {
            cli: String::new(),
            model: None,
            flags: vec![],
            label: Some("Evaluator".to_string()),
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        });

        if let Some(configured_qa_workers) = qa_workers {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                session.qa_workers = configured_qa_workers.to_vec();
            }
        }

        // Launch evaluator only — QA workers are spawned by the Evaluator
        // itself after activation, based on milestone contract criteria
        let _evaluator = self.launch_evaluator(session_id, evaluator_config, smoke_test)?;

        Ok(())
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

        let can_add_worker = matches!(
            session.state,
            SessionState::Running
                | SessionState::WaitingForWorker(_)
                | SessionState::WaitingForPlanner(_)
                | SessionState::SpawningEvaluator
                | SessionState::QaInProgress { .. }
                | SessionState::QaPassed
                | SessionState::QaFailed { .. }
                | SessionState::QaMaxRetriesExceeded
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

        let config_with_role = Self::apply_worker_identity(worker_index, &role, config);
        let (cmd, mut args) = Self::build_command(&config_with_role);
        let worker_branch = format!("hive/{}/worker-{}", session_id, worker_index);
        // Late-spawned workers should branch from the most recent session-integrated commit when possible.
        let base_ref = Self::resolve_worker_base_ref(&session, "add_worker", worker_index);
        let (_, worker_cwd) = create_session_worktree(
            session_id,
            &format!("worker-{}", worker_index),
            &worker_branch,
            &base_ref,
            &session.project_path,
        )?;
        self.emit_workspace_created(
            session_id,
            PRIMARY_CELL_ID,
            &worker_branch,
            Some(&worker_cwd),
        );

        let worker_cell_name = format!("worker-{worker_index}");
        let task_file_path = Self::task_file_path_for_worker(Path::new(&worker_cwd), worker_index as usize);

        // Write task file for this worker (STANDBY or with initial task)
        let _task_file = match Self::write_task_file(
            Path::new(&worker_cwd),
            worker_index,
            config_with_role.initial_prompt.as_deref(),
        ) {
            Ok(task_file) => task_file,
            Err(err) => {
                Self::rollback_worker_launch_artifacts(
                    &session.project_path,
                    session_id,
                    &worker_cell_name,
                    &task_file_path,
                    None,
                );
                return Err(err);
            }
        };

        // Write worker prompt to file and add to args
        let worker_prompt = Self::build_worker_prompt(worker_index, &config_with_role, &actual_parent_id, session_id);
        let filename = format!("worker-{}-prompt.md", worker_index);
        let prompt_file_path = session
            .project_path
            .join(".hive-manager")
            .join(session_id)
            .join("prompts")
            .join(&filename);
        let prompt_file = match Self::write_prompt_file(&session.project_path, session_id, &filename, &worker_prompt) {
            Ok(prompt_file) => prompt_file,
            Err(err) => {
                Self::rollback_worker_launch_artifacts(
                    &session.project_path,
                    session_id,
                    &worker_cell_name,
                    &task_file_path,
                    Some(&prompt_file_path),
                );
                return Err(err);
            }
        };
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

        let worker_role = AgentRole::Worker { index: worker_index, parent: Some(actual_parent_id.clone()) };

        // Spawn PTY
        {
            let pty_manager = self.pty_manager.read();
            if let Err(e) = pty_manager.create_session(
                worker_id.clone(),
                worker_role.clone(),
                &cmd,
                &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                Some(&worker_cwd),
                120,
                30,
            ) {
                Self::rollback_worker_launch_artifacts(
                    &session.project_path,
                    session_id,
                    &worker_cell_name,
                    &task_file_path,
                    Some(&prompt_file),
                );
                return Err(format!("Failed to spawn Worker {}: {}", worker_index, e));
            }
        }

        // Create agent info with role
        let agent_config = config_with_role;

        let agent_info = AgentInfo {
            id: worker_id.clone(),
            role: worker_role,
            status: AgentStatus::Running,
            config: agent_config,
            parent_id: Some(actual_parent_id),
            commit_sha: None,
            base_commit_sha: None,
        };

        // Update session
        {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                session.agents.push(agent_info.clone());
                // Don't promote ephemeral worker worktrees to session-level metadata.
                // Only persist long-lived primary worktrees here.
                self.emit_agent_launched(session, &agent_info);
            }
        }

        self.emit_session_update(session_id);

        // Update session storage
        self.update_session_storage(session_id);
        self.ensure_task_watcher(session_id, &session.project_path);

        Ok(agent_info)
    }

    #[allow(dead_code)]
    pub fn launch_evaluator(&self, session_id: &str, mut config: AgentConfig, smoke_test: bool) -> Result<AgentInfo, String> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        let evaluator_id = format!("{}-evaluator", session_id);
        if let Some(existing) = session.agents.iter().find(|agent| agent.id == evaluator_id) {
            let evaluator_alive = self.pty_manager.read().is_alive(&evaluator_id);
            if evaluator_alive {
                return Ok(existing.clone());
            }
            tracing::info!(
                session_id = %session_id,
                evaluator_id = %evaluator_id,
                "Respawning stale evaluator after PTY exit"
            );
        }

        if config.cli.trim().is_empty() {
            config.cli = session.default_cli.clone();
        }
        if config.model.is_none() {
            config.model = session.default_model.clone();
        }
        if config.label.is_none() {
            config.label = Some("Evaluator".to_string());
        }

        let spawning_changes = {
            let mut sessions = self.sessions.write();
            if let Some(current) = sessions.get_mut(session_id) {
                current.agents.retain(|agent| agent.id != evaluator_id);
                Some(self.set_session_state_with_events(
                    current,
                    SessionState::SpawningEvaluator,
                ))
            } else {
                None
            }
        };
        self.emit_session_update(session_id);
        self.update_session_storage(session_id);
        if let Some(changes) = spawning_changes {
            self.emit_cell_status_changes(session_id, changes);
        }

        Self::write_tool_files(&session.project_path, session_id, &config.cli)?;

        let evaluator_prompt =
            Self::build_evaluator_prompt(session_id, &config, &session.qa_workers, smoke_test);
        let prompt_file = Self::write_prompt_file(
            &session.project_path,
            session_id,
            "evaluator-prompt.md",
            &evaluator_prompt,
        )?;

        let (cmd, mut args) = Self::build_command(&config);
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_file.to_string_lossy());

        let cwd = session.project_path.to_str().unwrap_or(".");
        {
            let pty_manager = self.pty_manager.read();
            pty_manager
                .create_session(
                    evaluator_id.clone(),
                    AgentRole::Evaluator,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Evaluator: {}", e))?;
        }

        let agent_info = AgentInfo {
            id: evaluator_id,
            role: AgentRole::Evaluator,
            status: AgentStatus::Running,
            config,
            parent_id: None,
            commit_sha: None,
            base_commit_sha: None,
        };

        let (timeout_secs, qa_changes) = {
            let mut sessions = self.sessions.write();
            let current = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?;
            current.agents.push(agent_info.clone());
            self.emit_agent_launched(current, &agent_info);
            let next_state = qa_in_progress_state(&current.state);
            let changes = self.set_session_state_with_events(current, next_state);
            (current.qa_timeout_secs, changes)
        };

        self.emit_session_update(session_id);
        self.update_session_storage(session_id);
        self.emit_cell_status_changes(session_id, qa_changes);
        self.ensure_task_watcher(session_id, &session.project_path);
        self.start_qa_timeout(session_id, timeout_secs);

        Ok(agent_info)
    }

    #[allow(dead_code)]
    pub fn add_qa_worker(
        &self,
        session_id: &str,
        mut config: AgentConfig,
        specialization: String,
        parent_id: Option<String>,
    ) -> Result<AgentInfo, String> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        let evaluator_id = parent_id.unwrap_or_else(|| format!("{}-evaluator", session_id));
        let parent_agent = session.agents.iter().find(|agent| agent.id == evaluator_id);
        match parent_agent {
            Some(agent) if matches!(agent.role, AgentRole::Evaluator) => {}
            Some(_) => {
                return Err(format!(
                    "Cannot add QA worker: parent '{}' is not an Evaluator",
                    evaluator_id
                ));
            }
            None => {
                return Err(format!(
                    "Evaluator {} not found for session {}",
                    evaluator_id, session_id
                ));
            }
        }

        if config.cli.trim().is_empty() {
            config.cli = session.default_cli.clone();
        }
        if config.model.is_none() {
            config.model = session.default_model.clone();
        }
        if config.label.is_none() {
            config.label = Some(Self::qa_worker_label(&specialization).to_string());
        }

        let next_index = session
            .agents
            .iter()
            .filter(|agent| matches!(agent.role, AgentRole::QaWorker { .. }))
            .count() as u8
            + 1;

        let qa_worker_id = format!("{}-qa-worker-{}", session_id, next_index);
        Self::write_qa_task_file(
            &session.project_path,
            session_id,
            next_index,
            &specialization,
            config.initial_prompt.as_deref(),
        )?;
        let qa_worker_prompt = Self::build_qa_worker_prompt(
            session_id,
            next_index,
            &specialization,
            &config,
            &session.auth_strategy,
        );
        let prompt_file = Self::write_prompt_file(
            &session.project_path,
            session_id,
            &format!("qa-worker-{}-prompt.md", next_index),
            &qa_worker_prompt,
        )?;

        let (cmd, mut args) = Self::build_command(&config);
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_file.to_string_lossy());

        let cwd = session.project_path.to_str().unwrap_or(".");
        let role = AgentRole::QaWorker {
            index: next_index,
            parent: Some(evaluator_id.clone()),
        };
        {
            let pty_manager = self.pty_manager.read();
            pty_manager
                .create_session(
                    qa_worker_id.clone(),
                    role.clone(),
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn QA worker {}: {}", next_index, e))?;
        }

        let agent_info = AgentInfo {
            id: qa_worker_id,
            role,
            status: AgentStatus::Running,
            config,
            parent_id: Some(evaluator_id),
            commit_sha: None,
            base_commit_sha: None,
        };

        let qa_changes = {
            let mut sessions = self.sessions.write();
            if let Some(current) = sessions.get_mut(session_id) {
                current.agents.push(agent_info.clone());
                self.emit_agent_launched(current, &agent_info);
                let next_state = qa_in_progress_state(&current.state);
                Some(self.set_session_state_with_events(current, next_state))
            } else {
                None
            }
        };

        self.emit_session_update(session_id);
        self.update_session_storage(session_id);
        if let Some(changes) = qa_changes {
            self.emit_cell_status_changes(session_id, changes);
        }
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
        let planner_prompt = Self::build_planner_prompt_with_http(
            &session.project_path,
            &config.cli,
            planner_index,
            &planner_config,
            &queen_id,
            session_id,
        );
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
            commit_sha: None,
            base_commit_sha: None,
        };

        // Update session state to WaitingForPlanner
        let waiting_changes = {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                session.agents.push(agent_info.clone());
                self.emit_agent_launched(session, &agent_info);
                Some(self.set_session_state_with_events(
                    session,
                    SessionState::WaitingForPlanner(planner_index),
                ))
            } else {
                None
            }
        };

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
        if let Some(changes) = waiting_changes {
            self.emit_cell_status_changes(session_id, changes);
        }
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
        Self::session_to_persisted_snapshot(session)
    }

    fn session_to_persisted_snapshot(session: &Session) -> crate::storage::PersistedSession {
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
            let role_str = serialize_persisted_agent_role(&a.role);

            PersistedAgentInfo {
                id: a.id.clone(),
                role: role_str,
                config: PersistedAgentConfig {
                    cli: a.config.cli.clone(),
                    model: a.config.model.clone(),
                    flags: a.config.flags.clone(),
                    label: a.config.label.clone(),
                    name: a.config.name.clone(),
                    description: a.config.description.clone(),
                    role_type: a.config.role.as_ref().map(|r| r.role_type.clone()),
                    initial_prompt: a.config.initial_prompt.clone(),
                },
                parent_id: a.parent_id.clone(),
                commit_sha: a.commit_sha.clone(),
                base_commit_sha: a.base_commit_sha.clone(),
            }
        }).collect();

        let state_str = serialize_session_state(&session.state);
        let auth_strategy = if is_terminal_session_state(&session.state) {
            AuthStrategy::None
        } else {
            session.auth_strategy.clone()
        };

        PersistedSession {
            id: session.id.clone(),
            name: session.name.clone(),
            color: session.color.clone(),
            session_type,
            project_path: session.project_path.to_string_lossy().to_string(),
            created_at: session.created_at,
            last_activity_at: Some(session.last_activity_at),
            agents,
            state: state_str,
            default_cli: session.default_cli.clone(),
            default_model: session.default_model.clone(),
            qa_workers: session.qa_workers.clone(),
            max_qa_iterations: session.max_qa_iterations,
            qa_timeout_secs: session.qa_timeout_secs,
            auth_strategy: auth_strategy.persist_value(),
            worktree_path: session.worktree_path.clone(),
            worktree_branch: session.worktree_branch.clone(),
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
                let role_str = format_agent_display(&agent.role);

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
                .filter(|a| include_in_worker_roster(&a.role))
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
        let worktrees_path = project_path
            .join(".hive-manager")
            .join("worktrees")
            .join(session_id);
        let fusion_worktrees_path = project_path.join(".hive-fusion").join(session_id);

        match TaskFileWatcher::new(
            &session_path,
            &worktrees_path,
            &fusion_worktrees_path,
            session_id,
            app_handle,
        ) {
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
        if let Err(e) = self.update_session_storage_checked(session_id) {
            tracing::warn!("Failed to update session metadata: {}", e);
        }
    }

    fn update_session_storage_checked(&self, session_id: &str) -> Result<(), String> {
        if let Some(ref storage) = self.storage {
            let session = {
                let mut sessions = self.sessions.write();
                let Some(session) = sessions.get_mut(session_id) else {
                    return Ok(());
                };
                let now = Utc::now();
                if now > session.last_activity_at {
                    session.last_activity_at = now;
                }
                session.clone()
            };

            Self::persist_session_snapshot(storage, &session, session_id)?;
        }

        Ok(())
    }

    fn persist_session_snapshot(
        storage: &SessionStorage,
        session: &Session,
        session_id: &str,
    ) -> Result<(), String> {
        let persisted = Self::session_to_persisted_snapshot(session);
        storage
            .save_session(&persisted)
            .map_err(|e| format!("Failed to update session metadata: {}", e))?;

        let hierarchy: Vec<HierarchyNode> = session
            .agents
            .iter()
            .map(|agent| {
                let role_str = format_agent_display(&agent.role);

                let children: Vec<String> = session
                    .agents
                    .iter()
                    .filter(|a| a.parent_id.as_ref() == Some(&agent.id))
                    .map(|a| a.id.clone())
                    .collect();

                HierarchyNode {
                    id: agent.id.clone(),
                    role: role_str,
                    parent_id: agent.parent_id.clone(),
                    children,
                }
            })
            .collect();

        let workers: Vec<WorkerStateInfo> = session
            .agents
            .iter()
            .filter(|a| include_in_worker_roster(&a.role))
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

        let state_manager = StateManager::new(storage.session_dir(session_id));
        if let Err(e) = state_manager.update_hierarchy(&hierarchy) {
            tracing::warn!("Failed to update hierarchy: {}", e);
        }
        if let Err(e) = state_manager.update_workers_file(&workers) {
            tracing::warn!("Failed to update workers file: {}", e);
        }

        Ok(())
    }
}

impl Default for SessionController {
    fn default() -> Self {
        Self::new(Arc::new(RwLock::new(PtyManager::new())))
    }
}

fn parse_agent_role(role: &str) -> Option<AgentRole> {
    if role == "MasterPlanner" {
        Some(AgentRole::MasterPlanner)
    } else if role == "Queen" {
        Some(AgentRole::Queen)
    } else if role == "Evaluator" {
        Some(AgentRole::Evaluator)
    } else if role.starts_with("Planner(") {
        let index = role
            .trim_start_matches("Planner(")
            .trim_end_matches(")")
            .parse::<u8>()
            .ok()?;
        Some(AgentRole::Planner { index })
    } else if role.starts_with("Worker(") {
        parse_indexed_role(role, "Worker(").map(|(index, parent)| AgentRole::Worker { index, parent })
    } else if role.starts_with("QaWorker(") {
        parse_indexed_role(role, "QaWorker(").map(|(index, parent)| AgentRole::QaWorker { index, parent })
    } else if role.starts_with("Fusion(") {
        let variant = role
            .trim_start_matches("Fusion(")
            .trim_end_matches(")")
            .to_string();
        Some(AgentRole::Fusion { variant })
    } else if role.starts_with("Judge(") {
        let session_id = role
            .trim_start_matches("Judge(")
            .trim_end_matches(")")
            .to_string();
        Some(AgentRole::Judge { session_id })
    } else {
        None
    }
}

fn parse_indexed_role(role: &str, prefix: &str) -> Option<(u8, Option<String>)> {
    let parts: Vec<&str> = role
        .trim_start_matches(prefix)
        .trim_end_matches(")")
        .split(',')
        .collect();
    let index = parts.first()?.parse::<u8>().ok()?;
    let parent = parts.get(1).and_then(|s| {
        let trimmed = s.trim();
        if trimmed == "None" {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    Some((index, parent))
}

fn parse_persisted_session_state(state: &str) -> SessionState {
    if let Some(iteration) = state.strip_prefix("QaInProgress:") {
        return SessionState::QaInProgress {
            iteration: iteration.parse::<u8>().ok().filter(|iteration| *iteration > 0),
        };
    }
    if let Some(iteration) = state.strip_prefix("QaFailed:") {
        let iteration = iteration
            .parse::<u8>()
            .ok()
            .filter(|iteration| *iteration > 0)
            .unwrap_or(1);
        return SessionState::QaFailed { iteration };
    }

    match state {
        "Planning" => SessionState::Planning,
        "PlanReady" => SessionState::PlanReady,
        "Starting" => SessionState::Starting,
        "SpawningWorker" => SessionState::SpawningWorker(0),
        "WaitingForWorker" => SessionState::WaitingForWorker(0),
        "SpawningPlanner" => SessionState::SpawningPlanner(0),
        "WaitingForPlanner" => SessionState::WaitingForPlanner(0),
        "SpawningFusionVariant" => SessionState::SpawningFusionVariant(0),
        "WaitingForFusionVariants" => SessionState::WaitingForFusionVariants,
        "SpawningJudge" => SessionState::SpawningJudge,
        "Judging" => SessionState::Judging,
        "AwaitingVerdictSelection" => SessionState::AwaitingVerdictSelection,
        "MergingWinner" => SessionState::MergingWinner,
        "SpawningEvaluator" => SessionState::SpawningEvaluator,
        "QaInProgress" => SessionState::QaInProgress { iteration: None },
        "QaPassed" => SessionState::QaPassed,
        "QaFailed" => SessionState::QaFailed { iteration: 1 },
        "QaMaxRetriesExceeded" => SessionState::QaMaxRetriesExceeded,
        "Running" => SessionState::Running,
        "Paused" => SessionState::Paused,
        "Closing" => SessionState::Closing,
        "Closed" => SessionState::Closed,
        "Failed" => SessionState::Failed("persisted".to_string()),
        _ => SessionState::Completed,
    }
}

fn serialize_session_state(state: &SessionState) -> String {
    match state {
        SessionState::Planning => "Planning".to_string(),
        SessionState::PlanReady => "PlanReady".to_string(),
        SessionState::Starting => "Starting".to_string(),
        SessionState::SpawningWorker(_) => "SpawningWorker".to_string(),
        SessionState::WaitingForWorker(_) => "WaitingForWorker".to_string(),
        SessionState::SpawningPlanner(_) => "SpawningPlanner".to_string(),
        SessionState::WaitingForPlanner(_) => "WaitingForPlanner".to_string(),
        SessionState::SpawningFusionVariant(_) => "SpawningFusionVariant".to_string(),
        SessionState::WaitingForFusionVariants => "WaitingForFusionVariants".to_string(),
        SessionState::SpawningJudge => "SpawningJudge".to_string(),
        SessionState::Judging => "Judging".to_string(),
        SessionState::AwaitingVerdictSelection => "AwaitingVerdictSelection".to_string(),
        SessionState::MergingWinner => "MergingWinner".to_string(),
        SessionState::SpawningEvaluator => "SpawningEvaluator".to_string(),
        SessionState::QaInProgress { iteration } => match iteration {
            Some(iteration) if *iteration > 0 => format!("QaInProgress:{}", iteration),
            _ => "QaInProgress".to_string(),
        },
        SessionState::QaPassed => "QaPassed".to_string(),
        SessionState::QaFailed { iteration } => format!("QaFailed:{}", iteration),
        SessionState::QaMaxRetriesExceeded => "QaMaxRetriesExceeded".to_string(),
        SessionState::Running => "Running".to_string(),
        SessionState::Paused => "Paused".to_string(),
        SessionState::Completed => "Completed".to_string(),
        SessionState::Closing => "Closing".to_string(),
        SessionState::Closed => "Closed".to_string(),
        SessionState::Failed(_) => "Failed".to_string(),
    }
}

fn serialize_persisted_agent_role(role: &AgentRole) -> String {
    match role {
        AgentRole::MasterPlanner => "MasterPlanner".to_string(),
        AgentRole::Queen => "Queen".to_string(),
        AgentRole::Planner { index } => format!("Planner({})", index),
        AgentRole::Worker { index, parent } => {
            format!("Worker({},{})", index, parent.as_deref().unwrap_or("None"))
        }
        AgentRole::Fusion { variant } => format!("Fusion({})", variant),
        AgentRole::Judge { session_id } => format!("Judge({})", session_id),
        AgentRole::Evaluator => "Evaluator".to_string(),
        AgentRole::QaWorker { index, parent } => {
            format!("QaWorker({},{})", index, parent.as_deref().unwrap_or("None"))
        }
    }
}

fn serialize_agent_role(role: &AgentRole) -> &'static str {
    match role {
        AgentRole::MasterPlanner => "master-planner",
        AgentRole::Queen => "queen",
        AgentRole::Planner { .. } => "planner",
        AgentRole::Worker { .. } => "worker",
        AgentRole::Fusion { .. } => "fusion",
        AgentRole::Judge { .. } => "judge",
        AgentRole::Evaluator => "evaluator",
        AgentRole::QaWorker { .. } => "qa-worker",
    }
}

fn format_agent_display(role: &AgentRole) -> String {
    match role {
        AgentRole::MasterPlanner => "MasterPlanner".to_string(),
        AgentRole::Queen => "Queen".to_string(),
        AgentRole::Planner { index } => format!("Planner-{}", index),
        AgentRole::Worker { index, .. } => format!("Worker-{}", index),
        AgentRole::Fusion { variant } => format!("Fusion-{}", variant),
        AgentRole::Judge { session_id } => format!("Judge-{}", session_id),
        AgentRole::Evaluator => "Evaluator".to_string(),
        AgentRole::QaWorker { index, .. } => format!("QaWorker-{}", index),
    }
}

fn include_in_worker_roster(role: &AgentRole) -> bool {
    !matches!(serialize_agent_role(role), "queen" | "evaluator" | "qa-worker")
}

#[cfg(test)]
mod tests {
    use super::{
        parse_persisted_session_state, serialize_session_state, AgentConfig, AgentInfo,
        AuthStrategy, CompletionError, QaWorkerConfig, Session, SessionController, SessionError,
        SessionState, SessionType,
    };
    use crate::pty::{AgentRole, AgentStatus, PtyManager, WorkerRole};
    use crate::workspace::git::current_head;
    use chrono::{Duration, Utc};
    use parking_lot::RwLock;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

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
        let _spawning_evaluator = SessionState::SpawningEvaluator;
        let _qa_in_progress = SessionState::QaInProgress { iteration: None };
        let _qa_passed = SessionState::QaPassed;
        let _qa_failed = SessionState::QaFailed { iteration: 1 };
        let _qa_max_retries = SessionState::QaMaxRetriesExceeded;
        let _running = SessionState::Running;
        let _paused = SessionState::Paused;
        let _completed = SessionState::Completed;
        let _closed = SessionState::Closed;
        let _failed = SessionState::Failed("error".to_string());
    }

    #[test]
    fn session_state_serialization() {
        let state = SessionState::SpawningWorker(3);
        let json = serde_json::to_string(&state).expect("serialize SessionState");
        assert!(json.contains("SpawningWorker"));
    }

    #[test]
    fn persisted_qa_state_round_trip_uses_safe_fallbacks() {
        assert_eq!(
            parse_persisted_session_state("QaInProgress"),
            SessionState::QaInProgress { iteration: None }
        );
        assert_eq!(
            parse_persisted_session_state("QaInProgress:2"),
            SessionState::QaInProgress { iteration: Some(2) }
        );
        assert_eq!(
            parse_persisted_session_state("QaFailed"),
            SessionState::QaFailed { iteration: 1 }
        );
        assert_eq!(
            parse_persisted_session_state("QaFailed:3"),
            SessionState::QaFailed { iteration: 3 }
        );
        assert_eq!(
            serialize_session_state(&SessionState::QaInProgress { iteration: Some(2) }),
            "QaInProgress:2"
        );
        assert_eq!(serialize_session_state(&SessionState::QaPassed), "QaPassed");
    }

    #[test]
    fn qa_worker_prompt_uses_requested_specialization() {
        let prompt = SessionController::build_qa_worker_prompt(
            "session-123",
            1,
            "a11y",
            &AgentConfig::default(),
            &AuthStrategy::default(),
        );

        assert!(prompt.contains("Accessibility Tester"));
        assert!(prompt.contains("axe-core"));
        assert!(!prompt.contains("UI Tester"));
    }

    #[test]
    fn worker_task_file_path_uses_worktree_local_hive_manager_dir() {
        let path = SessionController::task_file_path_for_worker(
            Path::new("/repo/.hive-manager/worktrees/session-123/worker-2"),
            2,
        );

        assert_eq!(
            path,
            PathBuf::from(
                "/repo/.hive-manager/worktrees/session-123/worker-2/.hive-manager/tasks/worker-2-task.md"
            )
        );
    }

    #[test]
    fn absolute_worker_task_file_path_matches_worktree_convention() {
        let path = SessionController::absolute_task_file_path_for_worker(
            Path::new("/repo"),
            "session-123",
            4,
        );

        assert_eq!(
            path,
            PathBuf::from(
                "/repo/.hive-manager/worktrees/session-123/worker-4/.hive-manager/tasks/worker-4-task.md"
            )
        );
    }

    #[test]
    fn write_tool_files_includes_spawn_qa_worker_doc() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        SessionController::write_tool_files(
            &PathBuf::from(temp_dir.path()),
            "session-123",
            "claude",
        )
        .expect("write tool files");

        let tool_path = temp_dir
            .path()
            .join(".hive-manager")
            .join("session-123")
            .join("tools")
            .join("spawn-qa-worker.md");
        let content = std::fs::read_to_string(tool_path).expect("read QA tool doc");

        assert!(content.contains("/api/sessions/session-123/qa-workers"));
        assert!(content.contains("\"specialization\": \"ui\""));
    }

    #[test]
    fn evaluator_prompt_uses_session_default_cli_and_model() {
        let prompt = SessionController::build_evaluator_prompt(
            "session-123",
            &AgentConfig {
                cli: "codex".to_string(),
                model: Some("gpt-5.4".to_string()),
                ..AgentConfig::default()
            },
            &[],
            false,
        );

        assert_eq!(
            extract_markdown_section(&prompt, "## Required Protocol"),
            SessionController::evaluator_required_protocol("session-123"),
        );
        assert!(prompt.contains(".hive-manager/session-123/peer/qa-verdict.json"));
        assert!(prompt.contains("This session uses CLI: codex, Model: gpt-5.4."));
        assert!(prompt.contains(r#""specialization": "api", "cli": "codex", "model": "gpt-5.4""#));
        assert!(!prompt.contains(r#""cli": "claude""#));
    }

    #[test]
    fn evaluator_prompt_uses_configured_qa_workers() {
        let prompt = SessionController::build_evaluator_prompt(
            "session-123",
            &AgentConfig {
                cli: "claude".to_string(),
                model: Some("opus-4-6".to_string()),
                ..AgentConfig::default()
            },
            &[QaWorkerConfig {
                specialization: "ui".to_string(),
                cli: "gemini".to_string(),
                model: Some("gemini-2.5-pro".to_string()),
                label: Some("Visual QA".to_string()),
                flags: None,
            }],
            false,
        );

        assert_eq!(
            extract_markdown_section(&prompt, "## Required Protocol"),
            SessionController::evaluator_required_protocol("session-123"),
        );
        assert!(prompt.contains("configured QA workers below (1 total) before you grade any criterion"));
        assert!(prompt.contains(r#""specialization": "ui", "cli": "gemini", "model": "gemini-2.5-pro""#));
        assert!(prompt.contains("You MUST spawn all 1 QA workers one at a time in this exact order:"));
        assert!(!prompt.contains(r#""specialization": "api", "cli": "claude""#));
    }

    #[test]
    fn scope_block_is_identical_across_worker_and_task_surfaces() {
        let session_id = "session-scope-equality";
        let worktree_path = PathBuf::from(format!(".hive-manager/worktrees/{session_id}/worker-1"));
        let session_worktree_root = worktree_path
            .parent()
            .and_then(|path| path.parent())
            .expect("session worktree root");

        let _ = std::fs::remove_dir_all(session_worktree_root);
        std::fs::create_dir_all(&worktree_path).expect("create worktree");

        let fusion_prompt = SessionController::build_fusion_worker_prompt(
            session_id,
            1,
            "Variant 1",
            "feat/test",
            ".hive-manager/worktrees/session-scope-equality/worker-1",
            "Test task",
            "claude",
        );
        let worker_prompt = SessionController::build_worker_prompt(
            1,
            &AgentConfig {
                role: Some(WorkerRole::new("backend", "Backend", "claude")),
                ..AgentConfig::default()
            },
            "session-scope-equality-queen",
            session_id,
        );
        let task_file_path = SessionController::write_task_file_with_status(
            &worktree_path,
            1,
            Some("Test task"),
            Some("ACTIVE"),
        )
        .expect("write task file");
        let task_file = std::fs::read_to_string(&task_file_path).expect("read task file");

        let expected = SessionController::scope_block(".");
        assert_eq!(extract_markdown_section(&worker_prompt, "## Scope"), expected);
        assert_eq!(extract_markdown_section(&fusion_prompt, "## Scope"), expected);
        assert_eq!(extract_markdown_section(&task_file, "## Scope"), expected);

        std::fs::remove_dir_all(session_worktree_root).expect("remove test worktree");
    }

    #[test]
    fn required_protocol_block_is_identical_across_queens() {
        let session_root = SessionController::session_root_path(Path::new("/repo"), "session-123");
        let queen_master_prompt = SessionController::build_queen_master_prompt(
            "claude",
            Path::new("/repo"),
            Path::new("/repo/.hive-manager/worktrees/session-123/queen"),
            "session-123",
            &[],
            None,
            false,
            true,
        );
        let fusion_queen_prompt = SessionController::build_fusion_queen_prompt(
            "claude",
            Path::new("/repo"),
            "session-123",
            &[],
            "Test task",
            true,
        );
        let swarm_queen_prompt = SessionController::build_swarm_queen_prompt(
            "claude",
            Path::new("/repo"),
            "session-123",
            &[],
            None,
            true,
        );
        let expected = SessionController::queen_required_protocol(&session_root, true);

        assert_eq!(
            extract_markdown_section(&queen_master_prompt, "## Required Protocol"),
            expected,
        );
        assert_eq!(
            extract_markdown_section(&fusion_queen_prompt, "## Required Protocol"),
            expected,
        );
        assert_eq!(
            extract_markdown_section(&swarm_queen_prompt, "## Required Protocol"),
            expected,
        );
    }

    #[test]
    fn evaluator_required_protocol_omits_queen_only_handoff_and_wait_text() {
        let evaluator_prompt = SessionController::build_evaluator_prompt(
            "session-123",
            &AgentConfig {
                cli: "claude".to_string(),
                model: Some("opus-4-6".to_string()),
                ..AgentConfig::default()
            },
            &[],
            false,
        );
        let required_protocol = extract_markdown_section(&evaluator_prompt, "## Required Protocol");

        assert_eq!(
            required_protocol,
            SessionController::evaluator_required_protocol("session-123"),
        );
        assert!(!required_protocol.contains("signal the existing Evaluator"));
        assert!(!required_protocol.contains("WAIT for"));
        assert!(required_protocol.contains("The Queen signals you via"));
    }

    fn extract_markdown_section(content: &str, heading: &str) -> String {
        let start = content
            .find(heading)
            .unwrap_or_else(|| panic!("missing heading: {heading}"));
        let rest = &content[start..];
        let search_offset = heading.len();
        let end = rest[search_offset..]
            .find("\n## ")
            .map(|offset| start + search_offset + offset)
            .unwrap_or(content.len());
        content[start..end].trim_end().to_string()
    }

    fn test_controller() -> SessionController {
        SessionController::new(Arc::new(RwLock::new(PtyManager::new())))
    }

    fn run_git(repo_path: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo_path)
            .status()
            .expect("run git command");
        assert!(status.success(), "git {:?} should succeed", args);
    }

    fn init_repo_with_worker_worktree(session_id: &str, worker_id: u8) -> (TempDir, PathBuf) {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let repo_path = temp_dir.path();

        for args in [
            ["init", "-b", "main"].as_slice(),
            ["config", "user.name", "Hive Test"].as_slice(),
            ["config", "user.email", "hive@example.com"].as_slice(),
        ] {
            run_git(repo_path, args);
        }

        std::fs::write(repo_path.join("README.md"), "base commit\n").expect("write file");
        run_git(repo_path, &["add", "README.md"]);
        run_git(repo_path, &["commit", "-m", "initial commit"]);

        let worktree_path = repo_path
            .join(".hive-manager")
            .join("worktrees")
            .join(session_id)
            .join(format!("worker-{worker_id}"));
        std::fs::create_dir_all(worktree_path.parent().unwrap()).expect("create worktree parent");
        let worker_branch = format!("hive/{session_id}/worker-{worker_id}");
        run_git(
            repo_path,
            &[
                "worktree",
                "add",
                "-b",
                &worker_branch,
                worktree_path.to_str().expect("utf8 path"),
                "HEAD",
            ],
        );

        (temp_dir, worktree_path)
    }

    fn waiting_worker_session(session_id: &str, repo_path: &Path, worker_id: u8) -> Session {
        let worker_worktree = repo_path
            .join(".hive-manager")
            .join("worktrees")
            .join(session_id)
            .join(format!("worker-{worker_id}"));
        Session {
            id: session_id.to_string(),
            name: None,
            color: None,
            session_type: SessionType::Hive { worker_count: 1 },
            project_path: repo_path.to_path_buf(),
            state: SessionState::WaitingForWorker(worker_id),
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents: vec![AgentInfo {
                id: format!("{session_id}-worker-{worker_id}"),
                role: AgentRole::Worker {
                    index: worker_id,
                    parent: Some(format!("{session_id}-queen")),
                },
                status: AgentStatus::Running,
                config: AgentConfig::default(),
                parent_id: Some(format!("{session_id}-queen")),
                commit_sha: None,
                base_commit_sha: current_head(&worker_worktree).ok(),
            }],
            default_cli: "claude".to_string(),
            default_model: None,
            qa_workers: Vec::new(),
            max_qa_iterations: 3,
            qa_timeout_secs: 300,
            auth_strategy: AuthStrategy::default(),
            worktree_path: None,
            worktree_branch: None,
        }
    }

    fn test_completion_session(
        id: &str,
        state: SessionState,
        last_activity_at: chrono::DateTime<Utc>,
        with_evaluator: bool,
    ) -> Session {
        let mut agents = Vec::new();
        if with_evaluator {
            agents.push(AgentInfo {
                id: format!("{id}-evaluator"),
                role: AgentRole::Evaluator,
                status: AgentStatus::Completed,
                config: AgentConfig::default(),
                parent_id: None,
                commit_sha: None,
                base_commit_sha: None,
            });
        }

        Session {
            id: id.to_string(),
            name: None,
            color: None,
            session_type: if with_evaluator {
                SessionType::Hive { worker_count: 1 }
            } else {
                SessionType::Fusion {
                    variants: vec!["alpha".to_string()],
                }
            },
            project_path: PathBuf::from("."),
            state,
            created_at: last_activity_at - Duration::minutes(1),
            last_activity_at,
            agents,
            default_cli: "claude".to_string(),
            default_model: None,
            qa_workers: Vec::new(),
            max_qa_iterations: 3,
            qa_timeout_secs: 300,
            auth_strategy: AuthStrategy::default(),
            worktree_path: None,
            worktree_branch: None,
        }
    }

    #[test]
    fn can_complete_session_allows_quiet_evaluator_backed_session() {
        let controller = test_controller();
        controller.insert_test_session(test_completion_session(
            "evaluator-ok",
            SessionState::QaPassed,
            Utc::now() - Duration::minutes(11),
            true,
        ));

        assert!(controller.can_complete_session("evaluator-ok").is_ok());
    }

    #[test]
    fn can_complete_session_rejects_non_quiet_or_missing_qa_pass() {
        let controller = test_controller();
        controller.insert_test_session(test_completion_session(
            "evaluator-blocked",
            SessionState::Running,
            Utc::now() - Duration::minutes(11),
            true,
        ));
        controller.insert_test_session(test_completion_session(
            "fusion-recent",
            SessionState::Running,
            Utc::now() - Duration::minutes(2),
            false,
        ));

        let blocked = controller
            .can_complete_session("evaluator-blocked")
            .expect_err("evaluator-backed session should require QaPassed");
        let blocked = match blocked {
            CompletionError::Blocked(blocked) => blocked,
            other => panic!("expected blocked completion error, got {:?}", other),
        };
        assert!(blocked.error.contains("QaPassed"));
        assert!(!blocked.unblock_paths.is_empty()); // Should have unblock paths for evaluator

        let recent = controller
            .can_complete_session("fusion-recent")
            .expect_err("fusion session should still require quiet period");
        let recent = match recent {
            CompletionError::Blocked(recent) => recent,
            other => panic!("expected blocked completion error, got {:?}", other),
        };
        assert!(recent.error.contains("10 minutes"));
        assert!(recent.remaining_quiescence_seconds.is_some());
    }

    #[test]
    fn can_complete_session_rejects_recent_qa_passed_session() {
        let controller = test_controller();
        controller.insert_test_session(test_completion_session(
            "evaluator-recent-pass",
            SessionState::QaPassed,
            Utc::now() - Duration::minutes(5),
            true,
        ));

        let blocked = controller
            .can_complete_session("evaluator-recent-pass")
            .expect_err("QaPassed session should still satisfy quiet period");
        let blocked = match blocked {
            CompletionError::Blocked(blocked) => blocked,
            other => panic!("expected blocked completion error, got {:?}", other),
        };
        assert!(blocked.error.contains("10 minutes"));
        assert!(blocked.remaining_quiescence_seconds.is_some());
    }

    #[test]
    fn can_complete_session_allows_quiet_fusion_session_without_evaluator() {
        let controller = test_controller();
        controller.insert_test_session(test_completion_session(
            "fusion-ok",
            SessionState::Running,
            Utc::now() - Duration::minutes(11),
            false,
        ));

        assert!(controller.can_complete_session("fusion-ok").is_ok());
    }

    #[test]
    fn worker_completion_commit_sha_is_none_when_branch_has_no_new_commit() {
        let session_id = "worker-commit-base";
        let (temp_dir, _) = init_repo_with_worker_worktree(session_id, 1);
        let session = waiting_worker_session(session_id, temp_dir.path(), 1);

        assert_eq!(
            SessionController::worker_completion_commit_sha(&session, 1),
            None
        );
    }

    #[test]
    fn worker_completion_commit_sha_ignores_upstream_project_head_movement() {
        let session_id = "worker-commit-stable-base";
        let (temp_dir, _) = init_repo_with_worker_worktree(session_id, 1);
        std::fs::write(temp_dir.path().join("main.txt"), "upstream change\n")
            .expect("write upstream change");
        run_git(temp_dir.path(), &["add", "main.txt"]);
        run_git(temp_dir.path(), &["commit", "-m", "upstream change"]);

        let session = waiting_worker_session(session_id, temp_dir.path(), 1);

        assert_eq!(
            SessionController::worker_completion_commit_sha(&session, 1),
            None
        );
    }

    #[test]
    fn worker_completion_commit_sha_returns_worker_head_after_commit() {
        let session_id = "worker-commit-head";
        let (temp_dir, worktree_path) = init_repo_with_worker_worktree(session_id, 1);
        std::fs::write(worktree_path.join("worker.txt"), "worker change\n").expect("write worker change");
        run_git(&worktree_path, &["add", "worker.txt"]);
        run_git(&worktree_path, &["commit", "-m", "worker change"]);

        let session = waiting_worker_session(session_id, temp_dir.path(), 1);
        let expected_head = current_head(&worktree_path).expect("worker HEAD");

        assert_eq!(
            SessionController::worker_completion_commit_sha(&session, 1),
            Some(expected_head)
        );
    }

    #[tokio::test]
    async fn on_worker_completed_rejects_missing_commit_when_gate_enabled() {
        let _env_guard = ENV_MUTEX.lock().unwrap();
        let session_id = "worker-gate-reject";
        let (temp_dir, _) = init_repo_with_worker_worktree(session_id, 1);
        let controller = test_controller();
        controller.insert_test_session(waiting_worker_session(session_id, temp_dir.path(), 1));

        unsafe {
            std::env::set_var("REQUIRE_COMMIT_SHA", "true");
        }
        let result = controller.on_worker_completed(session_id, 1).await;
        unsafe {
            std::env::remove_var("REQUIRE_COMMIT_SHA");
        }

        let err = result.expect_err("missing worker commit should block completion");
        assert!(matches!(
            err,
            SessionError::ConfigError(message) if message.contains("commit SHA required")
        ));

        let refreshed = controller.get_session(session_id).unwrap();
        assert_eq!(refreshed.state, SessionState::WaitingForWorker(1));
        assert_eq!(refreshed.agents[0].commit_sha, None);
    }

    #[tokio::test]
    async fn on_worker_completed_records_commit_sha_before_progression() {
        let session_id = "worker-gate-record";
        let (temp_dir, worktree_path) = init_repo_with_worker_worktree(session_id, 1);
        std::fs::write(worktree_path.join("worker.txt"), "worker change\n").expect("write worker change");
        run_git(&worktree_path, &["add", "worker.txt"]);
        run_git(&worktree_path, &["commit", "-m", "worker change"]);
        let expected_head = current_head(&worktree_path).expect("worker HEAD");

        let controller = test_controller();
        controller.insert_test_session(waiting_worker_session(session_id, temp_dir.path(), 1));

        controller
            .on_worker_completed(session_id, 1)
            .await
            .expect("missing pending config should not block commit capture");

        let refreshed = controller.get_session(session_id).unwrap();
        assert_eq!(refreshed.state, SessionState::WaitingForWorker(1));
        assert_eq!(refreshed.agents[0].commit_sha.as_deref(), Some(expected_head.as_str()));
    }

    /// Verifies that planning/swarm sessions (which never populate session.worktree_path)
    /// can still resolve a valid base_ref for late-spawned workers via the three-tier
    /// fallback ladder: session worktree HEAD -> project HEAD -> resolve_fresh_base.
    ///
    /// This test validates the fix from commit 21cce96 which added the current_head
    /// fallback for planning/swarm late spawns.
    #[test]
    fn planning_swarm_session_uses_project_head_as_base_when_no_session_worktree() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let repo_path = temp_dir.path();

        for args in [
            ["init", "-b", "main"].as_slice(),
            ["config", "user.name", "Hive Test"].as_slice(),
            ["config", "user.email", "hive@example.com"].as_slice(),
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(repo_path)
                .status()
                .expect("run git command");
            assert!(status.success(), "git {:?} should succeed", args);
        }

        std::fs::write(repo_path.join("README.md"), "base commit\n").expect("write file");
        for args in [["add", "README.md"].as_slice(), ["commit", "-m", "initial commit"].as_slice()] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(repo_path)
                .status()
                .expect("run git command");
            assert!(status.success(), "git {:?} should succeed", args);
        }

        let expected_head = current_head(repo_path).expect("project HEAD");

        let session = Session {
            id: "planning-session-123".to_string(),
            name: None,
            color: None,
            session_type: SessionType::Swarm { planner_count: 2 },
            project_path: repo_path.to_path_buf(),
            state: SessionState::Planning,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents: vec![],
            default_cli: "claude".to_string(),
            default_model: None,
            qa_workers: Vec::new(),
            max_qa_iterations: 3,
            qa_timeout_secs: 300,
            auth_strategy: AuthStrategy::default(),
            worktree_path: None, // Key: no session worktree for planning/swarm
            worktree_branch: None,
        };

        assert!(session.worktree_path.is_none());
        let base_ref = SessionController::resolve_worker_base_ref(
            &session,
            "spawn_next_worker",
            2,
        );

        assert_eq!(base_ref, expected_head);
    }
}


