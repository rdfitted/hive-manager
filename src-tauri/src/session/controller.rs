use crate::tauri_shim::{AppHandle, Emitter};
use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock, RwLockReadGuard};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::artifacts::collector::ArtifactCollector;
use crate::cli::{CliBehavior, CliRegistry};
use crate::coordination::queue_manager::{heartbeat_cadence_label, STUCK_CUTOFF_SECS};
use crate::coordination::{HierarchyNode, StateManager, WorkerStateInfo};
use crate::domain::{ArtifactBundle, HiveExecutionPolicy, HiveLaunchKind, WorkspaceStrategy};
use crate::events::{EventBus, EventEmitter};
use crate::orchestrator::session_orchestrator::SessionOrchestrator;
use crate::pty::{AgentConfig, AgentRole, AgentStatus, PtyManager, WorkerRole};
use crate::session::cell_status::{
    agent_in_cell, derive_cell_status_name, derive_cell_status_name_for_state, session_cell_ids,
    variant_to_cell_id, PRIMARY_CELL_ID, RESOLVER_CELL_ID,
};
use crate::session::polling_intervals::{
    format_poll_label, ACTIVATION_POLL_INTERVAL, SMOKE_ACTIVE_POLL_INTERVAL,
    SMOKE_EVALUATOR_FIRST_POLL_INTERVAL, SMOKE_IDLE_POLL_INTERVAL, STANDARD_ACTIVE_POLL_INTERVAL,
    STANDARD_EVALUATOR_FIRST_POLL_INTERVAL, STANDARD_IDLE_POLL_INTERVAL,
};
use crate::session::prompt_contract::{
    render_assignment_contract, render_capability_card, render_delegation_guidance,
    render_role_kernel, render_workspace_contract, AssignmentSpec, ContractRole,
};
use crate::storage::{SessionStorage, StorageError};
use crate::templates::{heartbeat_snippet, PromptContext, TemplateEngine};
use crate::watcher::TaskFileWatcher;
use crate::workspace::git::{
    cleanup_session_worktrees, create_session_worktree, current_head, remove_session_worktree_cell,
    resolve_fresh_base,
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

fn extract_model_arg(args: &[&str]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if *arg == "-m" || *arg == "--model" {
            return iter.next().map(|model| (*model).to_string());
        }

        if let Some(model) = arg.strip_prefix("--model=") {
            if !model.is_empty() {
                return Some(model.to_string());
            }
        }
    }

    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionType {
    Hive { worker_count: u8 },
    Swarm { planner_count: u8 },
    Fusion { variants: Vec<String> },
    Debate { variants: Vec<String> },
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
const MAX_DEBATE_ROUNDS: u8 = 20;

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

    fn apply_prompt_variables(&self, session_id: &str, variables: &mut HashMap<String, String>) {
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
                variables.insert(
                    "auth_bypass_url".to_string(),
                    "(not configured)".to_string(),
                );
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
    pub fn state_blocked(
        _session_id: &str,
        current_state: &SessionState,
        requires_evaluator: bool,
    ) -> Self {
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
                "Session completion blocked: session must be in Running or QaPassed state"
                    .to_string(),
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
    SpawningDebateRound(u8),
    WaitingForDebateRound(u8),
    SpawningJudge,
    Judging,
    AwaitingVerdictSelection,
    MergingWinner,
    SpawningEvaluator,
    QaInProgress {
        iteration: Option<u8>,
    },
    QaPassed,
    QaFailed {
        iteration: u8,
    },
    QaMaxRetriesExceeded,
    /// The Evaluator returned a verdict (or a non-zero set of findings) and a
    /// Prince peer is now resolving them with its own fix team. Blocks PR push /
    /// completion until the Prince self-certifies via `prince/verdict`.
    PrinceRemediation,
    /// QA could not produce a usable verdict — the verdict timed out with no
    /// response, or the Evaluator reported BLOCKED (e.g. a pass-criterion needs a
    /// UI that isn't present, or QA workers failed to report over HTTP). Terminal
    /// for the automated loop: never auto-ships. Operator unblocks via
    /// force-pass / force-fail.
    QaInconclusive,
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
                | SessionState::PrinceRemediation
                | SessionState::QaInconclusive
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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
    pub with_planning: bool, // If true, spawn Master Planner first
    #[serde(default)]
    pub with_evaluator: bool,
    #[serde(default)]
    pub evaluator_config: Option<AgentConfig>,
    #[serde(default)]
    pub qa_workers: Option<Vec<QaWorkerConfig>>,
    #[serde(default)]
    pub smoke_test: bool, // If true, create a minimal test plan without real investigation
    #[serde(default)]
    pub execution_policy: HiveExecutionPolicy,
}

/// Launch config for **Research** mode.
///
/// Research mode is a UI/launch *profile* that reuses the Hive launch path under
/// the hood (exactly like Solo mode reuses it). It is NOT a distinct
/// `SessionType`/`SessionMode` variant. It produces a Queen + N "researcher"
/// workers, renders the research-flavored `queen-research` / `roles/researcher`
/// prompts, and always launches with planning and evaluator disabled.
///
/// Mirrors the shape of [`HiveLaunchConfig`] so the frontend can build it the
/// same way it builds a Hive launch.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResearchLaunchConfig {
    pub project_path: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    pub queen_config: AgentConfig,
    pub workers: Vec<AgentConfig>,
    pub prompt: Option<String>,
    /// Minimal-plumbing smoke test: the Queen spawns exactly ONE researcher with a
    /// trivial canned task, confirms the round-trip, and reports — skipping the wiki
    /// load and the Draft -> PR capture (no side effects).
    #[serde(default)]
    pub smoke_test: bool,
}

/// Expand a leading `~` in a path to the user's home directory so the value can
/// be safely embedded in a shell command (a tilde inside quotes is NOT expanded
/// by the shell). Returns the input unchanged if it doesn't start with `~` or
/// the home directory cannot be determined.
fn expand_tilde(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_string();
    };
    // Only expand the current user's home (`~`, `~/...`, `~\...`). A bare `~user`
    // form refers to another user's home and is not something we resolve — leave
    // it untouched rather than mangling it into `<home>/user`.
    if !rest.is_empty() && !rest.starts_with('/') && !rest.starts_with('\\') {
        return path.to_string();
    }
    let home = if cfg!(windows) {
        std::env::var("USERPROFILE").ok()
    } else {
        std::env::var("HOME").ok()
    };
    match home {
        Some(home) => {
            let rest = rest.trim_start_matches(['/', '\\']);
            if rest.is_empty() {
                home
            } else {
                format!("{}/{}", home.trim_end_matches(['/', '\\']), rest)
            }
        }
        None => path.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SwarmLaunchConfig {
    pub project_path: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default = "default_swarm_cli")]
    pub default_cli: String,
    #[serde(default)]
    pub default_model: Option<String>,
    pub queen_config: AgentConfig,
    pub planner_count: u8,                     // How many planners
    pub planner_config: AgentConfig,           // Config shared by all planners
    pub workers_per_planner: Vec<AgentConfig>, // Workers shared config (applied to each planner)
    pub prompt: Option<String>,
    #[serde(default)]
    pub with_planning: bool, // If true, spawn Master Planner first
    #[serde(default)]
    pub with_evaluator: bool,
    #[serde(default)]
    pub evaluator_config: Option<AgentConfig>,
    #[serde(default)]
    pub qa_workers: Option<Vec<QaWorkerConfig>>,
    #[serde(default)]
    pub smoke_test: bool, // If true, create a minimal test plan without real investigation

    // Legacy support - if planners vec is provided, use it instead
    #[serde(default)]
    pub planners: Vec<PlannerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, schemars::JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlannerConfig {
    pub config: AgentConfig,
    pub domain: String,
    pub workers: Vec<AgentConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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

fn default_swarm_cli() -> String {
    "claude".to_string()
}

fn default_debate_rounds() -> u8 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DebateLaunchConfig {
    pub project_path: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    pub debaters: Vec<DebateDebaterConfig>,
    pub topic: String,
    #[serde(default = "default_debate_rounds")]
    pub rounds: u8,
    pub judge_config: AgentConfig,
    #[serde(default)]
    pub queen_config: Option<AgentConfig>,
    #[serde(default)]
    pub with_planning: bool,
    #[serde(default = "default_fusion_cli")]
    pub default_cli: String,
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DebateDebaterConfig {
    pub name: String,
    #[serde(default)]
    pub stance: Option<String>,
    pub cli: String,
    pub model: Option<String>,
    #[serde(default)]
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DebateSessionMetadata {
    base_branch: String,
    debaters: Vec<DebateDebaterMetadata>,
    judge_config: AgentConfig,
    topic: String,
    rounds: u8,
    verdict_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DebateDebaterMetadata {
    index: u8,
    name: String,
    stance: Option<String>,
    slug: String,
    branch: String,
    worktree_path: String,
    config: AgentConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateDebaterStatus {
    pub index: u8,
    pub name: String,
    pub stance: Option<String>,
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
    /// Default managed-principal CLI/model/flags. `None` CLI is the legacy sentinel:
    /// dynamic workers fall back to the historical session (Queen) defaults.
    #[serde(default)]
    pub default_principal_cli: Option<String>,
    #[serde(default)]
    pub default_principal_model: Option<String>,
    #[serde(default)]
    pub default_principal_flags: Vec<String>,
    #[serde(default)]
    pub execution_policy: HiveExecutionPolicy,
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
    /// No-git sessions (Research) never create worktrees or branches: every agent,
    /// including workers spawned later by the Queen, runs directly in `project_path`.
    /// Used by `add_worker` to skip worktree creation so on-demand spawning works on
    /// non-repo folders and honors the research "no git" contract.
    #[serde(default)]
    pub no_git: bool,
    /// Populated by `resume_session` (#125): per-step classification of a resumed run so
    /// the frontend can show a confirmation modal. `None` for freshly launched sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_report: Option<crate::domain::run_journal::ResumeReport>,
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
    /// Session-owned operator shells. These PTYs are deliberately separate from
    /// `Session::agents` so they never enter worker queues, artifacts, or agent trees.
    scratch_ptys: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    /// A short-lived creation barrier while stop/close snapshots and kills scratch PTYs.
    scratch_pty_cleanup_sessions: Arc<RwLock<HashSet<String>>>,
    /// Serialize scratch create/kill and stop/close for the same session so ownership,
    /// process lifecycle, cleanup barriers, and state transitions cannot interleave.
    session_lifecycle_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// QA timeout cancel handles: session_id -> abort handle
    qa_timeout_handles: Mutex<HashMap<String, tokio::task::AbortHandle>>,
    evaluator_respawns_inflight: Mutex<HashSet<String>>,
    /// Durable run journal + side-effect ledger (#125). Optional so tests/legacy
    /// construction paths can run without a SQLite DB; write-step seams no-op when unset.
    run_journal: Option<crate::storage::RunJournalStore>,
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
        SessionType::Fusion { .. } | SessionType::Debate { .. } => match &agent.role {
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
fn get_polling_instructions(
    cli: &str,
    task_file: &str,
    role_type: Option<&str>,
    heartbeat_command: Option<&str>,
) -> String {
    // #141: the cadence is derived from the reclaim cutoff, and EVERY behavior gets it. A
    // behavior that receives no cadence instruction produces a silent worker, and a silent
    // worker is indistinguishable from a dead one to `reclaim_stuck`.
    let cadence = heartbeat_cadence_label();
    let heartbeat_line = heartbeat_command
        .map(|command| format!("  {command}\n"))
        .unwrap_or_default();
    // Behaviors that get no bash loop need the same instruction in their own register.
    let heartbeat_block = |lead: &str| match heartbeat_command {
        Some(command) => format!("\n{lead}\n```bash\n{command}\n```\n"),
        None => String::new(),
    };

    match CliRegistry::get_behavior_for_role(cli, role_type) {
        CliBehavior::ExplicitPolling => {
            format!(
                r#"
## Polling Protocol (MANDATORY)
Run this bash loop to wait for task activation:
```bash
while true; do
  STATUS=$(grep "^## Status:" "{task_file}" | head -1)
  if [[ "$STATUS" == *"ACTIVE"* ]]; then break; fi
{heartbeat_line}
  sleep {poll_secs}
done
```
The `sleep {poll_secs}` keeps you inside the required heartbeat cadence ({cadence}). Do not
lengthen it: the orchestrator requeues a worker whose last heartbeat is over {cutoff_secs}s old.
"#,
                task_file = task_file,
                heartbeat_line = heartbeat_line,
                poll_secs = ACTIVATION_POLL_INTERVAL.as_secs(),
                cadence = cadence,
                cutoff_secs = STUCK_CUTOFF_SECS,
            )
        }
        CliBehavior::ActionProne => {
            format!(
                r#"
## WAIT FOR ACTIVATION (CRITICAL)
WARNING: You MUST wait for your task file Status to become ACTIVE.
WARNING: Do NOT start working just because you received this prompt.
WARNING: Read {task_file} - if Status is STANDBY, WAIT.
WARNING: Waiting is NOT silence. You MUST send a heartbeat {cadence} the entire time you
wait. The orchestrator requeues a worker whose last heartbeat is over {cutoff_secs}s old.
{heartbeat_block}
Check the file, heartbeat, then wait. Do not proceed until ACTIVE.
"#,
                task_file = task_file,
                cadence = cadence,
                cutoff_secs = STUCK_CUTOFF_SECS,
                heartbeat_block = heartbeat_block("Send this heartbeat while waiting:"),
            )
        }
        CliBehavior::InstructionFollowing => {
            format!(
                r#"
## Task Coordination
Read {task_file}. Begin work only when Status is ACTIVE.
While the status is still STANDBY, send a heartbeat {cadence}. A worker whose last heartbeat
is over {cutoff_secs}s old is treated as stuck and its run is requeued.
{heartbeat_block}"#,
                task_file = task_file,
                cadence = cadence,
                cutoff_secs = STUCK_CUTOFF_SECS,
                heartbeat_block = heartbeat_block("Heartbeat command:"),
            )
        }
        CliBehavior::Interactive => {
            format!(
                r#"
## Task Coordination
Read {task_file}. Begin work only when Status is ACTIVE.
Use the interactive interface to monitor your task file.
While you monitor, run this heartbeat {cadence} from the interactive shell. A worker whose
last heartbeat is over {cutoff_secs}s old is treated as stuck and its run is requeued.
{heartbeat_block}"#,
                task_file = task_file,
                cadence = cadence,
                cutoff_secs = STUCK_CUTOFF_SECS,
                heartbeat_block = heartbeat_block("Heartbeat command:"),
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
            scratch_ptys: Arc::new(RwLock::new(HashMap::new())),
            scratch_pty_cleanup_sessions: Arc::new(RwLock::new(HashSet::new())),
            session_lifecycle_locks: Mutex::new(HashMap::new()),
            qa_timeout_handles: Mutex::new(HashMap::new()),
            evaluator_respawns_inflight: Mutex::new(HashSet::new()),
            run_journal: None,
        }
    }

    /// Attach the run journal store (#125). Schema must already be ensured by the caller.
    pub fn set_run_journal(&mut self, store: crate::storage::RunJournalStore) {
        self.run_journal = Some(store);
    }

    /// Record a write-step as `Started`, returning its deterministic id. No-op (returns
    /// `None`) when no journal store is attached. Errors are logged, not propagated, so
    /// journaling never blocks the orchestration path.
    fn journal_step_started(
        &self,
        run_id: &str,
        kind: crate::domain::run_journal::StepKind,
        ordinal: u64,
        detail: Option<&str>,
    ) -> Option<String> {
        let store = self.run_journal.as_ref()?;
        match store.record_step_started(run_id, kind, ordinal, detail) {
            Ok(step_id) => Some(step_id),
            Err(e) => {
                tracing::warn!(
                    run_id,
                    ?kind,
                    ordinal,
                    "Failed to journal step start: {}",
                    e
                );
                None
            }
        }
    }

    /// Mark a previously-started write-step finished. No-op when journaling is unset.
    fn journal_step_finished(
        &self,
        run_id: &str,
        step_id: &str,
        status: crate::domain::run_journal::StepStatus,
    ) {
        if let Some(store) = self.run_journal.as_ref() {
            if let Err(e) = store.record_step_finished(run_id, step_id, status) {
                tracing::warn!(run_id, step_id, "Failed to journal step finish: {}", e);
            }
        }
    }

    /// Check the journal for a Completed write-step of the given kind/ordinal. Used by the
    /// resume guard so already-completed destructive ops are not re-executed.
    fn is_write_step_completed(
        &self,
        run_id: &str,
        kind: crate::domain::run_journal::StepKind,
        ordinal: u64,
    ) -> bool {
        let Some(store) = self.run_journal.as_ref() else {
            return false;
        };
        let step_id = crate::domain::run_journal::RunJournalEntry::deterministic_step_id(
            run_id, kind, ordinal,
        );
        match store.read_journal(run_id) {
            Ok(entries) => entries.iter().any(|e| {
                e.step_id == step_id
                    && matches!(
                        e.status,
                        crate::domain::run_journal::StepStatus::Completed
                            | crate::domain::run_journal::StepStatus::Skipped
                    )
            }),
            Err(_) => false,
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
        let launch_model = extract_model_arg(&base_args)
            .or_else(|| CliRegistry::default_model(cmd).map(ToString::to_string));

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent
            let queen_id = format!("{}-queen", session_id);
            let mut queen_args = base_args.clone();

            // Add prompt as positional argument if provided and command is claude
            if cmd == "claude" && !prompt_str.is_empty() {
                queen_args.push(&prompt_str);
            }

            tracing::info!(
                "Launching Queen agent: {} {:?} in {:?}",
                cmd,
                queen_args,
                project_path
            );

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
                model: launch_model.clone(),
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

                tracing::info!(
                    "Launching Worker {} agent: {} {:?} in {:?}",
                    i,
                    cmd,
                    worker_args,
                    project_path
                );

                pty_manager
                    .create_session(
                        worker_id.clone(),
                        AgentRole::Worker {
                            index: i,
                            parent: None,
                        },
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
                    model: launch_model.clone(),
                    flags: worker_args.iter().map(|s| s.to_string()).collect(),
                    label: None,
                    name: None,
                    description: None,
                    role: None,
                    initial_prompt: None,
                };

                agents.push(AgentInfo {
                    id: worker_id.clone(),
                    role: AgentRole::Worker {
                        index: i,
                        parent: Some(format!("{}-queen", session_id)),
                    },
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
            default_model: launch_model,
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy: HiveExecutionPolicy::default(),
            qa_workers: Vec::new(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: None,
            worktree_branch: None,
            no_git: false,
            resume_report: None,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        self.emit_agent_batch_launched(&session, &session.agents);

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: session.clone(),
                },
            );
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

    pub fn reload_session_from_storage(&self, session_id: &str) -> Result<Session, String> {
        let storage = self
            .storage
            .clone()
            .ok_or_else(|| "Session storage is not initialized".to_string())?;
        let persisted = storage
            .load_session(session_id)
            .map_err(|e| format!("Failed to load session from storage: {}", e))?;
        storage
            .mark_session_synced(session_id, &persisted)
            .map_err(|e| format!("Failed to track session storage state: {}", e))?;
        let session = self.session_from_persisted(&persisted)?;
        {
            let mut sessions = self.sessions.write();
            sessions.insert(session.id.clone(), session.clone());
        }
        Ok(session)
    }

    /// Get the default CLI for a session
    pub fn get_session_default_cli(&self, session_id: &str) -> Option<String> {
        let sessions = self.sessions.read();
        sessions.get(session_id).map(|s| s.default_cli.clone())
    }

    /// Return the durable defaults for a newly managed principal. Sessions from
    /// before this contract keep `default_principal_cli = None`, which deliberately
    /// falls back to their historical session/Queen defaults.
    pub fn get_session_principal_defaults(&self, session_id: &str) -> Option<AgentConfig> {
        let sessions = self.sessions.read();
        sessions.get(session_id).map(|session| {
            let has_explicit_principal_default = session
                .default_principal_cli
                .as_deref()
                .is_some_and(|cli| !cli.trim().is_empty());
            let cli = session
                .default_principal_cli
                .clone()
                .filter(|cli| !cli.trim().is_empty())
                .unwrap_or_else(|| session.default_cli.clone());
            let model = if has_explicit_principal_default {
                session
                    .default_principal_model
                    .clone()
                    .or_else(|| CliRegistry::default_model(&cli).map(ToString::to_string))
            } else {
                session
                    .default_model
                    .clone()
                    .or_else(|| CliRegistry::default_model(&cli).map(ToString::to_string))
            };

            AgentConfig {
                cli,
                model,
                flags: if has_explicit_principal_default {
                    session.default_principal_flags.clone()
                } else {
                    Vec::new()
                },
                label: None,
                name: None,
                description: None,
                role: None,
                initial_prompt: None,
            }
        })
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
            matches!(
                session.state,
                SessionState::Running | SessionState::QaPassed
            )
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
            let persisted = storage.load_session(session_id).map_err(|err| match err {
                StorageError::SessionNotFound(_) => CompletionError::not_found(session_id),
                _ => CompletionError::storage(format!("Storage error: {}", err)),
            })?;
            self.session_from_persisted(&persisted)
                .map_err(CompletionError::storage)?
        };

        if !Self::state_allows_completion(&session) {
            return Err(CompletionError::Blocked(
                CompletionBlockedError::state_blocked(
                    session_id,
                    &session.state,
                    Self::session_requires_internal_evaluator(&session),
                ),
            ));
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
                let _ = app_handle.emit(
                    "heartbeat-status-changed",
                    serde_json::json!({
                        "session_id": session_id,
                        "agent_id": agent_id,
                        "status": status,
                        "summary": summary,
                    }),
                );
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
        heartbeats.get(session_id).cloned().unwrap_or_default()
    }

    pub(crate) fn emit_session_update(&self, session_id: &str) {
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
            if let Err(error) = emitter
                .emit_cell_created(&session_id, &cell_id, &cell_type)
                .await
            {
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

    fn merge_primary_cell_artifact_bundles(
        existing: ArtifactBundle,
        incoming: ArtifactBundle,
    ) -> ArtifactBundle {
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
        let branch = Self::merge_primary_cell_branch_labels([
            existing.branch.clone(),
            incoming.branch.clone(),
        ]);
        let summary = Self::merge_primary_cell_summaries(existing.summary, incoming.summary);
        let test_results = incoming.test_results.or(existing.test_results);
        let diff_summary =
            Self::merge_primary_cell_diff_summaries(existing.diff_summary, incoming.diff_summary);
        let mut unresolved_issues = existing.unresolved_issues;
        for issue in incoming.unresolved_issues {
            if !unresolved_issues.iter().any(|existing| existing == &issue) {
                unresolved_issues.push(issue);
            }
        }
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
        let mut unique = Vec::new();
        for branch_group in branches {
            for branch in branch_group.split(" | ") {
                let trimmed = branch.trim();
                if !trimmed.is_empty() && !unique.iter().any(|value| value == trimmed) {
                    unique.push(trimmed.to_string());
                }
            }
        }

        match unique.len() {
            0 => String::new(),
            1 => unique.into_iter().next().unwrap_or_default(),
            len if len > MAX_PRIMARY_CELL_BRANCHES => {
                let mut limited = unique
                    .into_iter()
                    .take(MAX_PRIMARY_CELL_BRANCHES)
                    .collect::<Vec<_>>();
                limited.push(format!("+{} more", len - MAX_PRIMARY_CELL_BRANCHES));
                limited.join(" | ")
            }
            _ => unique.join(" | "),
        }
    }

    fn merge_primary_cell_summaries(
        existing: Option<String>,
        incoming: Option<String>,
    ) -> Option<String> {
        let mut unique = Vec::new();
        for summary in [existing, incoming].into_iter().flatten() {
            for segment in summary.split(" · ") {
                let trimmed = segment.trim();
                if !trimmed.is_empty() && !unique.iter().any(|value: &String| value == trimmed) {
                    unique.push(trimmed.to_string());
                }
            }
        }
        (!unique.is_empty()).then(|| unique.join(" · "))
    }

    fn merge_primary_cell_diff_summaries(
        existing: Option<String>,
        incoming: Option<String>,
    ) -> Option<String> {
        let mut unique = Vec::new();
        for summary in [existing, incoming].into_iter().flatten() {
            for segment in summary.split("\n---\n") {
                let trimmed = segment.trim();
                if !trimmed.is_empty() && !unique.iter().any(|value: &String| value == trimmed) {
                    unique.push(trimmed.to_string());
                }
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

    fn agent_git_worktree_path_for_artifacts(
        session: &Session,
        agent: &AgentInfo,
    ) -> Option<PathBuf> {
        if session.no_git {
            return None;
        }
        if matches!(&session.session_type, SessionType::Hive { .. })
            && session.execution_policy.workspace_strategy == WorkspaceStrategy::SharedCell
            && matches!(&agent.role, AgentRole::Queen | AgentRole::Worker { .. })
        {
            return session.worktree_path.as_ref().map(PathBuf::from);
        }

        match &agent.role {
            AgentRole::Fusion { variant } => match &session.session_type {
                SessionType::Debate { .. } => {
                    Self::read_debate_metadata(&session.project_path, &session.id)
                        .ok()
                        .and_then(|meta| {
                            meta.debaters
                                .iter()
                                .find(|d| &d.name == variant)
                                .map(|d| PathBuf::from(&d.worktree_path))
                        })
                }
                _ => Self::read_fusion_metadata(&session.project_path, &session.id)
                    .ok()
                    .and_then(|meta| {
                        meta.variants
                            .iter()
                            .find(|v| &v.name == variant || v.agent_id == agent.id)
                            .map(|v| PathBuf::from(&v.worktree_path))
                    }),
            },
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
            // Primary-cell artifacts are cumulative evidence. The merge helpers
            // deduplicate repeated shared-workspace snapshots while preserving an
            // earlier worker's evidence after the Queen commits and the live diff changes.
            let incoming_bundle = bundle;
            if let Err(err) =
                storage.atomic_update_artifact(session_id, &cell_id, move |existing| {
                    existing.map_or(incoming_bundle.clone(), |existing_bundle| {
                        Self::merge_primary_cell_artifact_bundles(existing_bundle, incoming_bundle)
                    })
                })
            {
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

    #[cfg(test)]
    pub(crate) fn register_scratch_pty(
        &self,
        session_id: &str,
        pty_id: String,
    ) -> Result<(), String> {
        let _creation_guard = self.reserve_scratch_pty(session_id, pty_id)?;
        Ok(())
    }

    pub(crate) fn reserve_scratch_pty(
        &self,
        session_id: &str,
        pty_id: String,
    ) -> Result<RwLockReadGuard<'_, HashSet<String>>, String> {
        // The caller holds this read guard until process creation completes. Cleanup takes
        // the write side, so it cannot snapshot between ownership publication and spawn.
        let cleanup_sessions = self.scratch_pty_cleanup_sessions.read();
        let sessions = self.sessions.read();
        Self::validate_scratch_pty_session_locked(session_id, &cleanup_sessions, &sessions)?;

        let expected_prefix = format!("scratch:{session_id}:");
        let unique_id = pty_id.strip_prefix(&expected_prefix).unwrap_or_default();
        if session_id.contains(':') || unique_id.is_empty() || unique_id.contains(':') {
            return Err(format!(
                "Scratch PTY id must use the namespace {expected_prefix}<unique-id-without-colons>"
            ));
        }

        let inserted = self
            .scratch_ptys
            .write()
            .entry(session_id.to_string())
            .or_default()
            .insert(pty_id.clone());
        if !inserted {
            return Err(format!("Scratch PTY {pty_id} is already registered"));
        }
        Ok(cleanup_sessions)
    }

    fn validate_scratch_pty_session_locked(
        session_id: &str,
        cleanup_sessions: &HashSet<String>,
        sessions: &HashMap<String, Session>,
    ) -> Result<(), String> {
        if cleanup_sessions.contains(session_id) {
            return Err(format!(
                "Session {session_id} is stopping; scratch PTYs cannot be created"
            ));
        }

        let session = sessions
            .get(session_id)
            .ok_or_else(|| format!("Session {session_id} not found for scratch PTY"))?;
        if is_terminal_session_state(&session.state)
            || matches!(session.state, SessionState::Closing)
        {
            return Err(format!(
                "Session {session_id} is not running; scratch PTYs cannot be created"
            ));
        }

        Ok(())
    }

    pub(crate) fn unregister_scratch_pty(&self, pty_id: &str) {
        self.scratch_ptys.write().retain(|_, owned_ptys| {
            owned_ptys.remove(pty_id);
            !owned_ptys.is_empty()
        });
    }

    #[cfg(test)]
    pub(crate) fn insert_scratch_pty_ownership_for_test(
        &self,
        session_id: &str,
        pty_id: &str,
    ) {
        self.scratch_ptys
            .write()
            .entry(session_id.to_string())
            .or_default()
            .insert(pty_id.to_string());
    }

    #[cfg(test)]
    pub(crate) fn owns_scratch_pty_for_test(&self, session_id: &str, pty_id: &str) -> bool {
        self.scratch_ptys
            .read()
            .get(session_id)
            .is_some_and(|owned_ptys| owned_ptys.contains(pty_id))
    }

    fn begin_scratch_pty_cleanup(&self, session_id: &str) -> Vec<String> {
        self.scratch_pty_cleanup_sessions
            .write()
            .insert(session_id.to_string());
        self.scratch_ptys
            .read()
            .get(session_id)
            .map(|owned_ptys| owned_ptys.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn finish_scratch_pty_cleanup(&self, session_id: &str) {
        self.scratch_pty_cleanup_sessions
            .write()
            .remove(session_id);
    }

    pub(crate) fn scratch_pty_lifecycle_lock(
        &self,
        pty_id: &str,
    ) -> Option<Arc<Mutex<()>>> {
        let remainder = pty_id.strip_prefix("scratch:")?;
        let (session_id, unique_id) = remainder.rsplit_once(':')?;
        if session_id.is_empty() || unique_id.is_empty() {
            return None;
        }
        Some(self.session_lifecycle_lock(session_id))
    }

    pub(crate) fn session_lifecycle_lock(&self, session_id: &str) -> Arc<Mutex<()>> {
        self.session_lifecycle_locks
            .lock()
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub fn stop_session(&self, id: &str) -> Result<(), String> {
        let lifecycle_lock = self.session_lifecycle_lock(id);
        let _lifecycle_guard = lifecycle_lock.lock();
        let session = {
            let sessions = self.sessions.read();
            sessions.get(id).cloned()
        };

        if let Some(session) = session {
            let scratch_pty_ids = self.begin_scratch_pty_cleanup(id);
            let pty_manager = self.pty_manager.read();
            for agent in &session.agents {
                let _ = pty_manager.kill(&agent.id);
            }
            for pty_id in &scratch_pty_ids {
                if pty_manager.kill(pty_id).is_ok() {
                    self.unregister_scratch_pty(pty_id);
                }
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

            if let Some(((previous_session_state, previous_auth_strategy), changes)) =
                previous_state
            {
                if let Err(err) = self.persist_then_emit_session_update(id, changes) {
                    let mut sessions = self.sessions.write();
                    if let Some(session) = sessions.get_mut(id) {
                        session.state = previous_session_state;
                        session.auth_strategy = previous_auth_strategy;
                    }
                    self.finish_scratch_pty_cleanup(id);
                    return Err(err);
                }
            }

            self.finish_scratch_pty_cleanup(id);
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
        let mut persisted = storage.load_session(session_id).map_err(|err| match err {
            StorageError::SessionNotFound(_) => CompletionError::not_found(session_id),
            _ => CompletionError::storage(format!("Storage error: {}", err)),
        })?;
        persisted.state = serialize_session_state(&SessionState::Completed);
        persisted.auth_strategy = AuthStrategy::None.persist_value();
        storage.save_session(&persisted).map_err(|e| {
            CompletionError::storage(format!("Failed to persist session completion: {}", e))
        })?;

        Ok(())
    }

    pub fn close_session(&self, id: &str) -> Result<(), String> {
        let lifecycle_lock = self.session_lifecycle_lock(id);
        let _lifecycle_guard = lifecycle_lock.lock();
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

        let scratch_pty_ids = self.begin_scratch_pty_cleanup(id);

        let kill_errors: Vec<String> = {
            let pty_manager = self.pty_manager.read();
            let mut errors = Vec::new();
            for pty_id in &agent_ids {
                if let Err(e) = pty_manager.kill(pty_id) {
                    errors.push(format!("{}: {}", pty_id, e));
                }
            }
            for pty_id in &scratch_pty_ids {
                match pty_manager.kill(pty_id) {
                    Ok(()) => self.unregister_scratch_pty(pty_id),
                    Err(e) => errors.push(format!("{}: {}", pty_id, e)),
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
        self.finish_scratch_pty_cleanup(id);
        if !kill_errors.is_empty() {
            tracing::warn!(
                "Session {} closed with PTY kill errors: {}",
                id,
                kill_errors.join(" | ")
            );
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
        remove_worktree: bool,
    ) {
        if let Some(prompt_file_path) = prompt_file_path {
            Self::remove_worker_launch_file(session_id, worker_cell_name, prompt_file_path);
        }
        Self::remove_worker_launch_file(session_id, worker_cell_name, task_file_path);
        if !remove_worktree {
            return;
        }
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
                    if message.is_empty() {
                        "git branch -D failed".to_string()
                    } else {
                        message
                    }
                );
            }
            Err(err) => {
                tracing::warn!("Rollback failed to delete branch {}: {}", branch_name, err);
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
            sessions
                .get_mut(session_id)
                .map(|session| self.set_session_state_with_events(session, previous_state.clone()))
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

    fn configured_principal_defaults(
        workers: &[AgentConfig],
    ) -> (Option<String>, Option<String>, Vec<String>) {
        if let Some(principal) = workers.first() {
            let model = principal
                .model
                .clone()
                .or_else(|| CliRegistry::default_model(&principal.cli).map(ToString::to_string));
            return (Some(principal.cli.clone()), model, principal.flags.clone());
        }

        (
            Some("codex".to_string()),
            Some("gpt-5.6-sol".to_string()),
            Vec::new(),
        )
    }

    fn session_principal_cli(session: &Session) -> &str {
        session
            .default_principal_cli
            .as_deref()
            .filter(|cli| !cli.trim().is_empty())
            .unwrap_or(&session.default_cli)
    }

    /// Code under review/remediation lives in the managed primary/Queen worktree.
    /// Control-plane files remain rooted at `project_path`, so QA peers keep their
    /// PTY CWD there and receive this path as explicit execution guidance.
    fn execution_workspace(session: &Session) -> String {
        if !session.no_git
            && matches!(
                &session.session_type,
                SessionType::Hive { .. } | SessionType::Solo { .. }
            )
        {
            if let Some(path) = session.worktree_path.as_ref() {
                return path.clone();
            }
        }
        session.project_path.to_string_lossy().to_string()
    }

    fn session_type_supports_dynamic_principals(session_type: &SessionType) -> bool {
        matches!(
            session_type,
            SessionType::Hive { .. } | SessionType::Swarm { .. }
        )
    }

    fn session_allows_dynamic_principal(
        session: &Session,
        role: &WorkerRole,
        parent_id: Option<&str>,
    ) -> bool {
        if Self::session_type_supports_dynamic_principals(&session.session_type) {
            return true;
        }

        let prince_id = format!("{}-prince", session.id);
        matches!(&session.session_type, SessionType::Solo { .. })
            && session.state == SessionState::PrinceRemediation
            && role.role_type.eq_ignore_ascii_case("prince-fixer")
            && parent_id == Some(prince_id.as_str())
    }

    /// Build command and args from AgentConfig
    /// Returns (command, args) with CLI-specific flags already added
    fn build_command(config: &AgentConfig) -> (String, Vec<String>) {
        let mut args = Vec::new();
        let (effective_model, extra_flags) = CliRegistry::resolve_model_and_flags(
            &config.cli,
            config.model.as_deref(),
            CliRegistry::default_model(&config.cli),
            &config.flags,
        );

        // Add CLI-specific flags
        match config.cli.as_str() {
            "claude" => {
                // Claude CLI requires --dangerously-skip-permissions for automated use
                args.push("--dangerously-skip-permissions".to_string());
                if let Some(ref model) = effective_model {
                    args.push("--model".to_string());
                    args.push(model.to_string());
                }
            }
            "codex" => {
                // Codex CLI uses --dangerously-bypass-approvals-and-sandbox
                args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
                if let Some(ref model) = effective_model {
                    args.push("-m".to_string());
                    args.push(model.to_string());
                }
            }
            "opencode" => {
                // OpenCode relies on OPENCODE_YOLO=true env var (set in batch file)
                if let Some(ref model) = effective_model {
                    args.push("-m".to_string());
                    args.push(model.to_string());
                }
            }
            "cursor" => {
                // Cursor Agent via WSL - interactive TUI mode
                args.push("-d".to_string());
                args.push("Ubuntu".to_string());
                args.push("/root/.local/bin/agent".to_string());
                args.push("--force".to_string()); // Auto-approve commands
                                                  // Cursor uses global model setting, no --model flag
            }
            "droid" => {
                // Droid CLI - interactive TUI mode
                // Model selected via /model command or config
                // No auto-approve flag available in interactive mode
            }
            "qwen" => {
                // Qwen Code CLI - interactive mode with auto-approve
                args.push("-y".to_string()); // YOLO mode for auto-approve
                if let Some(ref model) = effective_model {
                    args.push("-m".to_string());
                    args.push(model.to_string());
                }
            }
            _ => {
                // For other CLIs, just add model flag if specified
                if let Some(ref model) = effective_model {
                    args.push("--model".to_string());
                    args.push(model.to_string());
                }
            }
        }

        // Add any extra flags from config
        args.extend(extra_flags);

        // Determine the actual command to run
        let command = match config.cli.as_str() {
            "cursor" => "wsl".to_string(), // Cursor runs via WSL
            _ => config.cli.clone(),       // Others use CLI name as command
        };

        (command, args)
    }

    /// Add prompt argument to args based on CLI type
    /// Each CLI has different syntax for accepting initial prompts
    fn add_prompt_to_args(cli: &str, args: &mut Vec<String>, prompt_path: &str) {
        let prompt_path = if Self::cli_runs_under_wsl(cli) {
            Self::to_wsl_path(prompt_path)
        } else {
            prompt_path.to_string()
        };
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
        let (effective_model, extra_flags) = CliRegistry::resolve_model_and_flags(
            &config.cli,
            config.model.as_deref(),
            CliRegistry::default_model(&config.cli),
            &config.flags,
        );

        // Add CLI-specific auto-approve flags (matching build_command for hive/swarm modes)
        match config.cli.as_str() {
            "claude" => {
                args.push("--dangerously-skip-permissions".to_string());
                if let Some(ref model) = effective_model {
                    args.push("--model".to_string());
                    args.push(model.to_string());
                }
            }
            "codex" => {
                args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
                if let Some(ref model) = effective_model {
                    args.push("-m".to_string());
                    args.push(model.to_string());
                }
            }
            "qwen" => {
                args.push("-y".to_string());
                if let Some(ref model) = effective_model {
                    args.push("-m".to_string());
                    args.push(model.to_string());
                }
            }
            "opencode" => {
                if let Some(ref model) = effective_model {
                    args.push("-m".to_string());
                    args.push(model.to_string());
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
                if let Some(ref model) = effective_model {
                    args.push("--model".to_string());
                    args.push(model.to_string());
                }
            }
        }

        // Add inline task if provided
        if let Some(task) = task {
            Self::add_inline_task_to_args(&config.cli, &mut args, task);
        }

        args.extend(extra_flags);

        let command = match config.cli.as_str() {
            "cursor" => "wsl".to_string(),
            _ => config.cli.clone(),
        };
        (command, args)
    }

    fn qa_blocked_verdict_grep_pattern() -> &'static str {
        r#""verdict"[[:space:]]*:[[:space:]]*"BLOCKED"|\\\"verdict\\\"[[:space:]]*:[[:space:]]*\\\"BLOCKED\\\""#
    }

    fn build_solo_evaluator_prompt(
        session_id: &str,
        project_path: &Path,
        execution_workspace: &str,
        task: Option<&str>,
    ) -> String {
        let session_root = Self::session_root_path(project_path, session_id);
        let qa_handoff = Self::build_qa_milestone_handoff(
            session_id,
            &session_root,
            "the Solo implementation and its focused validation",
        );
        let qa_verdict = Self::prompt_path(&session_root.join("peer").join("qa-verdict.json"));
        let prince_verdict =
            Self::prompt_path(&session_root.join("peer").join("prince-verdict.json"));
        let qa_blocked_pattern = Self::qa_blocked_verdict_grep_pattern();
        let objective = task.unwrap_or("Complete the operator's bounded Solo assignment.");

        format!(
            r#"# Solo Implementation Contract

You are the sole implementation agent for session `{session_id}`. Work in
`{execution_workspace}`. The backend has already launched an Evaluator and a
Prince as verification peers; do not spawn either one.

## Objective

{objective}

## Required Delivery Protocol

1. Implement the objective and run focused validation in `{execution_workspace}`.
2. Review the diff and commit the completed Solo implementation on the current
   backend-created branch before signaling QA. Do not push or switch branches.
3. Execute the QA Milestone Handoff below exactly once.
4. Poll `{qa_verdict}` until the Evaluator responds. If the verdict is BLOCKED,
   stop immediately and escalate to the operator; do not wait for Prince or
   claim completion.
5. For PASS or FAIL, poll `{prince_verdict}` until the Prince has integrated and
   certified any required remediation. On PASS/DONE, re-run focused validation
   and report the final result. Do not create generic managed principals yourself.

{qa_handoff}

## Verification Wait

```bash
while [ ! -f "{qa_verdict}" ]; do
  curl -fsS -X POST "http://localhost:18800/api/sessions/{session_id}/heartbeat" \
    -H "Content-Type: application/json" \
    -d '{{"agent_id":"{session_id}-worker-1","status":"working","summary":"Waiting for Evaluator verdict"}}'
  sleep 30
done
cat "{qa_verdict}"

if grep -Eq '{qa_blocked_pattern}' "{qa_verdict}"; then
  echo "QA is BLOCKED; stop and escalate to the operator. Do not wait for Prince remediation." >&2
  exit 1
fi

while [ ! -f "{prince_verdict}" ]; do
  curl -fsS -X POST "http://localhost:18800/api/sessions/{session_id}/heartbeat" \
    -H "Content-Type: application/json" \
    -d '{{"agent_id":"{session_id}-worker-1","status":"working","summary":"Waiting for Prince remediation"}}'
  sleep 30
done
cat "{prince_verdict}"
```
"#,
        )
    }

    fn run_git_in_dir(project_path: &PathBuf, args: &[&str]) -> Result<String, String> {
        if !project_path.exists() {
            return Err(format!(
                "Project path does not exist: {}",
                project_path.display()
            ));
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
        if out.is_empty() {
            "variant".to_string()
        } else {
            out
        }
    }

    fn unique_variant_slug(name: &str, seen: &mut HashMap<String, u16>) -> String {
        let base = Self::slugify_variant_name(name);
        let count = seen
            .entry(base.clone())
            .and_modify(|v| *v += 1)
            .or_insert(1);
        if *count == 1 {
            base
        } else {
            format!("{}-{}", base, count)
        }
    }

    fn validate_debate_rounds(rounds: u8) -> Result<u8, String> {
        if rounds == 0 {
            return Err("Debate launch requires at least one round".to_string());
        }
        if rounds > MAX_DEBATE_ROUNDS {
            return Err(format!(
                "Debate launch supports at most {} rounds",
                MAX_DEBATE_ROUNDS
            ));
        }
        Ok(rounds)
    }

    fn debate_round_agent_id(session_id: &str, debater_index: u8, round: u8) -> String {
        format!("{}-debate-{}-r{}", session_id, debater_index, round)
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

    fn read_fusion_metadata(
        project_path: &PathBuf,
        session_id: &str,
    ) -> Result<FusionSessionMetadata, String> {
        let metadata_path = Self::fusion_metadata_path(project_path, session_id);
        let json = std::fs::read_to_string(&metadata_path).map_err(|e| {
            format!(
                "Failed to read fusion metadata {}: {}",
                metadata_path.display(),
                e
            )
        })?;
        serde_json::from_str(&json).map_err(|e| format!("Failed to parse fusion metadata: {}", e))
    }

    fn debate_metadata_path(project_path: &PathBuf, session_id: &str) -> PathBuf {
        project_path
            .join(".hive-manager")
            .join(session_id)
            .join("debate-config.json")
    }

    fn write_debate_metadata(
        project_path: &PathBuf,
        session_id: &str,
        metadata: &DebateSessionMetadata,
    ) -> Result<(), String> {
        let metadata_path = Self::debate_metadata_path(project_path, session_id);
        if let Some(parent) = metadata_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create debate metadata dir: {}", e))?;
        }

        let json = serde_json::to_string_pretty(metadata)
            .map_err(|e| format!("Failed to serialize debate metadata: {}", e))?;
        std::fs::write(&metadata_path, json)
            .map_err(|e| format!("Failed to write debate metadata: {}", e))
    }

    fn read_debate_metadata(
        project_path: &PathBuf,
        session_id: &str,
    ) -> Result<DebateSessionMetadata, String> {
        let metadata_path = Self::debate_metadata_path(project_path, session_id);
        let json = std::fs::read_to_string(&metadata_path).map_err(|e| {
            format!(
                "Failed to read debate metadata {}: {}",
                metadata_path.display(),
                e
            )
        })?;
        serde_json::from_str(&json).map_err(|e| format!("Failed to parse debate metadata: {}", e))
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

    fn debate_round_task_file_path(worktree_path: &Path, debater_index: u8, round: u8) -> PathBuf {
        worktree_path
            .join(".hive-manager")
            .join("tasks")
            .join(format!(
                "debate-debater-{}-round-{}-task.md",
                debater_index, round
            ))
    }

    fn debate_round_argument_file_path(
        project_path: &Path,
        session_id: &str,
        round: u8,
        debater_slug: &str,
    ) -> PathBuf {
        project_path
            .join(".hive-manager")
            .join(session_id)
            .join("debate")
            .join("rounds")
            .join(format!("round-{}", round))
            .join(format!("{}.md", debater_slug))
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

    fn session_task_file_path(
        project_path: &Path,
        session_id: &str,
        worker_index: usize,
    ) -> PathBuf {
        Self::session_root_path(project_path, session_id)
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

    pub(crate) fn task_file_path_for_session_worker(
        session: &Session,
        worker_index: usize,
    ) -> Result<PathBuf, String> {
        if session.no_git {
            return Ok(Self::session_task_file_path(
                &session.project_path,
                &session.id,
                worker_index,
            ));
        }

        if matches!(&session.session_type, SessionType::Hive { .. })
            && session.execution_policy.workspace_strategy == WorkspaceStrategy::SharedCell
        {
            let primary = session.worktree_path.as_deref().ok_or_else(|| {
                format!(
                    "Shared-cell session {} is missing its primary worktree path",
                    session.id
                )
            })?;
            return Ok(Self::task_file_path_for_worker(
                Path::new(primary),
                worker_index,
            ));
        }

        Ok(Self::absolute_task_file_path_for_worker(
            &session.project_path,
            &session.id,
            worker_index,
        ))
    }

    pub(crate) fn absolute_task_file_path_for_qa_worker(
        project_path: &Path,
        session_id: &str,
        worker_index: usize,
    ) -> PathBuf {
        Self::qa_task_file_path(project_path, session_id, worker_index)
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
        let task_file = format!(
            ".hive-manager/tasks/fusion-variant-{}-task.md",
            variant_index
        );
        let agent_id = format!("{}-fusion-{}", session_id, variant_index);
        let startup_heartbeat = heartbeat_snippet(
            "http://localhost:18800",
            session_id,
            &agent_id,
            "working",
            "Starting fusion variant",
        );
        let heartbeat_command = heartbeat_snippet(
            "http://localhost:18800",
            session_id,
            &agent_id,
            "idle",
            "Waiting for task activation",
        );
        let completed_heartbeat = heartbeat_snippet(
            "http://localhost:18800",
            session_id,
            &agent_id,
            "completed",
            "Completed fusion variant",
        );
        let polling_instructions =
            get_polling_instructions(cli, &task_file, None, Some(&heartbeat_command));
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

## Task Coordination
Send a startup heartbeat before reading the task file:
```bash
{startup_heartbeat}
```

Read {task_file}. Begin work only when Status is ACTIVE.{polling_instructions}

## Completion Protocol (MANDATORY)

1. Run the focused validation required for this variant and review the final diff.
2. Commit only the completed variant work on the current backend-created Fusion branch. Do not push or switch branches.
3. Update {task_file} to `Status: COMPLETED` and add the result summary.
4. Send this completed heartbeat exactly as shown:
   ```bash
   {completed_heartbeat}
   ```
5. Report the commit SHA and validation evidence, then stop. Do not replace the completed status with an idle or working heartbeat unless a new ACTIVE assignment is issued."#,
            variant_name = variant_name,
            worktree_path = worktree_path,
            branch = branch,
            task_description = task_description,
            scope_block = scope_block,
            task_file = task_file,
            startup_heartbeat = startup_heartbeat,
            polling_instructions = polling_instructions,
            completed_heartbeat = completed_heartbeat,
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

    fn write_debate_round_task_file(
        worktree_path: &Path,
        debater: &DebateDebaterMetadata,
        topic: &str,
        round: u8,
        total_rounds: u8,
        argument_file: &Path,
        opponent_files: &str,
    ) -> Result<PathBuf, String> {
        let tasks_dir = worktree_path.join(".hive-manager").join("tasks");
        std::fs::create_dir_all(&tasks_dir)
            .map_err(|e| format!("Failed to create debate tasks directory: {}", e))?;

        let file_path = Self::debate_round_task_file_path(worktree_path, debater.index, round);
        let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let stance = debater
            .stance
            .as_deref()
            .unwrap_or("No explicit stance provided");
        let argument_file = Self::prompt_path(argument_file);
        let content = format!(
            r#"# Task Assignment - Debate Debater {debater_index} ({debater_name}) Round {round}

## Status: ACTIVE

## Role Constraints

- **DEBATER**: Argue your assigned position only.
- **SCOPE**: Do not edit production source code. Write only your debate argument file and this task file.
- **GIT**: Do NOT commit or push.

## Debate Topic

{topic}

## Your Stance

{stance}

## Round

Round {round} of {total_rounds}

## Opponent Prior-Round Arguments

{opponent_files}

## Deliverable

Write your argument or rebuttal to:

`{argument_file}`

## Completion Protocol

When the argument file is written:
1. Change Status to: COMPLETED
2. Add a short Result section summarizing your position

If blocked, change Status to: BLOCKED and describe the issue.

---
Last updated: {timestamp}
"#,
            debater_index = debater.index,
            debater_name = debater.name,
            round = round,
            total_rounds = total_rounds,
            topic = topic,
            stance = stance,
            opponent_files = opponent_files,
            argument_file = argument_file,
            timestamp = timestamp,
        );

        std::fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write debate task file: {}", e))?;
        Ok(file_path)
    }

    /// Insert the `global_wiki_path` prompt variable plus the `{{#if}}` gate flags
    /// that wrap the "Prior Wiki Context" load phase in the debate templates.
    ///
    /// **Every** template that renders `{{global_wiki_path}}` — `queen-research`,
    /// `debater`, and `debate-judge` — MUST get the variable from here. All three embed
    /// it in quoted shell commands, so all three need the same separator/WSL handling;
    /// normalizing per-site is exactly the sibling divergence that produced the
    /// trailing-dot split fixed in #159 and the missing outer loop fixed in #169.
    /// `cli` is the CLI that will execute the rendered prompt (see
    /// [`Self::normalize_wiki_path_for_cli`]).
    ///
    /// The gate flags exist so an unset/blank wiki path renders a prompt containing no
    /// read of an empty path: the whole `cat "<path>/index.md"` block is dropped
    /// and a short skip notice renders in its place. A debate must still run to
    /// completion with no wiki configured.
    fn insert_wiki_path_variables(
        variables: &mut HashMap<String, String>,
        global_wiki_path: &str,
        cli: &str,
    ) {
        let normalized = Self::normalize_wiki_path_for_cli(global_wiki_path, cli);
        let configured = !normalized.trim().is_empty();
        variables.insert("global_wiki_path".to_string(), normalized);
        variables.insert("has_global_wiki".to_string(), configured.to_string());
        variables.insert("no_global_wiki".to_string(), (!configured).to_string());
    }

    #[allow(clippy::too_many_arguments)]
    fn build_debate_debater_prompt(
        session_id: &str,
        debater: &DebateDebaterMetadata,
        topic: &str,
        round: u8,
        total_rounds: u8,
        argument_file: &Path,
        previous_round_dir: Option<&Path>,
        opponent_files: &str,
        task_file: &Path,
        global_wiki_path: &str,
    ) -> String {
        let mut variables = HashMap::new();
        let agent_id = Self::debate_round_agent_id(session_id, debater.index, round);
        variables.insert(
            "api_base_url".to_string(),
            "http://localhost:18800".to_string(),
        );
        variables.insert("agent_id".to_string(), agent_id);
        variables.insert("heartbeat_status".to_string(), "working".to_string());
        variables.insert(
            "heartbeat_summary".to_string(),
            format!("Debating round {} as {}", round, debater.name),
        );
        variables.insert("debater_name".to_string(), debater.name.clone());
        variables.insert(
            "stance".to_string(),
            debater
                .stance
                .clone()
                .unwrap_or_else(|| "No explicit stance provided".to_string()),
        );
        variables.insert("round".to_string(), round.to_string());
        variables.insert("total_rounds".to_string(), total_rounds.to_string());
        variables.insert("worktree_path".to_string(), debater.worktree_path.clone());
        variables.insert("branch".to_string(), debater.branch.clone());
        variables.insert(
            "argument_file".to_string(),
            Self::prompt_path(argument_file),
        );
        variables.insert(
            "previous_round_dir".to_string(),
            previous_round_dir
                .map(Self::prompt_path)
                .unwrap_or_else(|| "(none; this is the opening round)".to_string()),
        );
        variables.insert("opponent_files".to_string(), opponent_files.to_string());
        variables.insert("task_file".to_string(), Self::prompt_path(task_file));
        // The debater's own CLI executes this prompt, so it decides the wiki path form.
        Self::insert_wiki_path_variables(&mut variables, global_wiki_path, &debater.config.cli);

        let engine = TemplateEngine::default();
        let context = PromptContext {
            session_id: session_id.to_string(),
            project_path: debater.worktree_path.clone(),
            task: Some(topic.to_string()),
            variables,
            ..PromptContext::default()
        };

        engine.render_debater_prompt(&context).unwrap_or_else(|_| {
            format!(
                "Debate debater prompt failed to render for session {}",
                session_id
            )
        })
    }

    /// `judge_cli` is the **resolved** CLI the judge will run under (i.e. after the
    /// session-default fallback for a blank `metadata.judge_config.cli`), because it
    /// decides how the wiki path must be spelled in the prompt's shell blocks.
    fn build_debate_judge_prompt(
        session_id: &str,
        metadata: &DebateSessionMetadata,
        global_wiki_path: &str,
        judge_cli: &str,
    ) -> String {
        let mut variables = HashMap::new();
        variables.insert(
            "api_base_url".to_string(),
            "http://localhost:18800".to_string(),
        );
        variables.insert("agent_id".to_string(), format!("{}-judge", session_id));
        variables.insert("heartbeat_status".to_string(), "working".to_string());
        variables.insert(
            "heartbeat_summary".to_string(),
            "Judging debate".to_string(),
        );
        variables.insert("topic".to_string(), metadata.topic.clone());
        variables.insert(
            "topic_slug".to_string(),
            Self::slugify_variant_name(&metadata.topic),
        );
        variables.insert("rounds".to_string(), metadata.rounds.to_string());
        variables.insert("verdict_file".to_string(), metadata.verdict_file.clone());
        Self::insert_wiki_path_variables(&mut variables, global_wiki_path, judge_cli);

        let debater_list = metadata
            .debaters
            .iter()
            .map(|d| {
                let stance = d.stance.as_deref().unwrap_or("No explicit stance");
                format!("- {}: {} ({})", d.name, stance, d.worktree_path)
            })
            .collect::<Vec<_>>()
            .join("\n");
        variables.insert("debater_list".to_string(), debater_list);

        let round_files = (1..=metadata.rounds)
            .flat_map(|round| {
                metadata.debaters.iter().map(move |debater| {
                    format!(
                        "- Round {} / {}: .hive-manager/{}/debate/rounds/round-{}/{}.md",
                        round, debater.name, session_id, round, debater.slug
                    )
                })
            })
            .collect::<Vec<_>>()
            .join("\n");
        variables.insert("round_files".to_string(), round_files);

        let engine = TemplateEngine::default();
        let context = PromptContext {
            session_id: session_id.to_string(),
            task: Some(metadata.topic.clone()),
            variables,
            ..PromptContext::default()
        };

        engine
            .render_debate_judge_prompt(&context)
            .unwrap_or_else(|_| {
                format!(
                    "Debate judge prompt failed to render for session {}",
                    session_id
                )
            })
    }

    fn prompt_path(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    /// Does `cli` execute its prompt inside WSL rather than on the Windows host?
    ///
    /// `build_command` maps `cli == "cursor"` to the `wsl` executable, and call sites
    /// pass the *remapped* command name (`&cmd`) to `add_prompt_to_args`, so both
    /// spellings must answer yes. Centralized so the "runs under WSL" set is defined
    /// once instead of being re-`matches!`-ed at every site that needs to translate a
    /// host path (the divergence class behind #159 and #169).
    fn cli_runs_under_wsl(cli: &str) -> bool {
        matches!(cli.trim(), "cursor" | "wsl")
    }

    /// Normalize a configured global wiki path for embedding in the **quoted shell
    /// commands** of a rendered prompt, for the CLI that will actually execute it.
    ///
    /// `expand_tilde` resolves `~` from `USERPROFILE` on Windows, so the value reaching
    /// a prompt is mixed-separator — `C:\Users\RDuff/.ai-docs/wiki` for the default
    /// `~/.ai-docs/wiki`. Inside bash double quotes a backslash is only special before
    /// `$`, a backtick, `"`, `\`, or a newline, so `\U` survives literally and Git Bash's
    /// MSYS layer usually still resolves it — which is why this never visibly broke.
    ///
    /// It genuinely breaks under WSL: neither `C:\Users\...` **nor** `C:/Users/...`
    /// resolves there, only `/mnt/c/Users/...`. A separator swap alone would therefore
    /// look fixed while leaving the one adapter that needs real translation still broken,
    /// so WSL-backed CLIs are routed through [`Self::to_wsl_path`] — the same translation
    /// `add_prompt_to_args` already applies to the prompt file path for cursor.
    ///
    /// A blank path is returned unchanged so the `{{#if has_global_wiki}}` gates and the
    /// queen-research "if empty, skip gracefully" prose keep seeing an empty string.
    fn normalize_wiki_path_for_cli(global_wiki_path: &str, cli: &str) -> String {
        if global_wiki_path.trim().is_empty() {
            return global_wiki_path.to_string();
        }
        if Self::cli_runs_under_wsl(cli) {
            Self::to_wsl_path(global_wiki_path)
        } else {
            global_wiki_path.replace('\\', "/")
        }
    }

    fn to_wsl_path(path: &str) -> String {
        let forward_slash_path = path.replace('\\', "/");
        let bytes = forward_slash_path.as_bytes();

        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            let drive = bytes[0].to_ascii_lowercase() as char;
            let rest = forward_slash_path[2..].trim_start_matches('/');
            if rest.is_empty() {
                format!("/mnt/{drive}")
            } else {
                format!("/mnt/{drive}/{rest}")
            }
        } else {
            forward_slash_path
        }
    }

    fn worktree_boundary_rules(worktree_path: &str) -> String {
        format!(
            r#"- **READ**: You MAY inspect any repository file and git history for context by running Bash commands from this worktree.
- **WRITE**: You MUST create and modify files only inside `{worktree_path}`. You MUST NOT edit files outside this worktree."#,
            worktree_path = worktree_path,
        )
    }

    fn scope_block(worktree_path: &str) -> String {
        format!(
            "## Scope\n\n{}",
            Self::worktree_boundary_rules(worktree_path)
        )
    }

    /// Read-only scope block for research workers. They investigate and report;
    /// they must not mutate the project or its git state. Used for BOTH the worker
    /// prompt and the task file so the two surfaces stay consistent.
    fn scope_block_read_only() -> String {
        "## Scope (Read-Only)\n\nThis is a research role. You MUST NOT create, modify, move, or delete project files, and you MUST NOT run commands that mutate the project or its git state. The only permitted filesystem write is updating the status/result fields in the exact Hive control-plane task file named by your prompt. Read freely and investigate, then report your findings to the Queen via the conversation API — your deliverable is knowledge.".to_string()
    }

    fn queen_quality_reconciliation_log_lines(has_evaluator: bool) -> &'static str {
        if has_evaluator {
            QUEEN_QUALITY_RECONCILIATION_LOG_LINES
        } else {
            QUEEN_QUALITY_RECONCILIATION_LOG_LINES_NO_EVALUATOR
        }
    }

    fn queen_required_protocol(session_root: &Path, has_evaluator: bool) -> String {
        let mark_worker_status_path =
            Self::prompt_path(&session_root.join("tools").join("mark-worker-status.md"));
        if !has_evaluator {
            return format!(
                r#"## Required Protocol
```text
1. You MUST follow every numbered protocol in this prompt exactly as written.
2. You MUST use the inline bash polling commands shown in this prompt. You MUST NOT use `/loop`.
3. When you independently verify a managed principal, researcher, or Fusion variant is complete, you MUST immediately mark its exact agent ID `completed` using `{mark_worker_status_path}`. The UI completion checkoff and stall monitor depend on it.
```"#,
                mark_worker_status_path = mark_worker_status_path,
            );
        }

        let milestone_ready_path =
            Self::prompt_path(&session_root.join("peer").join("milestone-ready.json"));
        let qa_verdict_path = Self::prompt_path(&session_root.join("peer").join("qa-verdict.json"));

        format!(
            r#"## Required Protocol
```text
1. You MUST follow every numbered protocol in this prompt exactly as written.
2. You MUST use the inline bash polling commands shown in this prompt. You MUST NOT use `/loop`.
3. The Evaluator is created PROGRAMMATICALLY by the backend at session launch (`spawn_launch_evaluator_agents`). It already exists as `AgentRole::Evaluator`.
4. You MUST NOT spawn an Evaluator yourself. DO NOT `curl POST /workers` with `role=evaluator`. DO NOT `curl POST /evaluators`.
5. You MUST signal the existing Evaluator via `{milestone_ready_path}` and WAIT for `{qa_verdict_path}`.
6. When you independently verify a managed principal, researcher, or Fusion variant is complete, you MUST immediately mark its exact agent ID `completed` using `{mark_worker_status_path}`. The UI completion checkoff and stall monitor depend on it.
```"#,
            milestone_ready_path = milestone_ready_path,
            qa_verdict_path = qa_verdict_path,
            mark_worker_status_path = mark_worker_status_path,
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

    fn prince_required_protocol(session_id: &str) -> String {
        format!(
            r#"## Required Protocol
```text
1. You MUST follow every numbered protocol in this prompt exactly as written.
2. You MUST use the inline bash polling commands shown in this prompt. You MUST NOT use `/loop`.
3. The backend already launched you as `AgentRole::Prince`. You MUST NOT spawn another Prince or an Evaluator.
4. You MUST wait for `.hive-manager/{session_id}/peer/qa-verdict.json` before you plan or spawn fixers.
5. You MUST spawn fixers via `POST /api/sessions/{session_id}/workers` using the session CLI, and self-certify via `POST /api/sessions/{session_id}/prince/verdict`.
6. You MUST NOT push the PR or call `/complete` — the Queen pushes after you certify.
```"#,
            session_id = session_id,
        )
    }

    fn queen_post_workers_protocol(
        session_id: &str,
        session_root: &Path,
        has_evaluator: bool,
    ) -> String {
        let milestone_ready_path =
            Self::prompt_path(&session_root.join("peer").join("milestone-ready.json"));
        let qa_verdict_path = Self::prompt_path(&session_root.join("peer").join("qa-verdict.json"));
        let prince_verdict_path =
            Self::prompt_path(&session_root.join("peer").join("prince-verdict.json"));

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

Hard rule: The Evaluator AND the Prince are created PROGRAMMATICALLY by the backend at session launch (`spawn_launch_evaluator_agents`). They already exist as `AgentRole::Evaluator` and `AgentRole::Prince`. You MUST NOT spawn either one. DO NOT `curl POST /workers` with `role=evaluator`, DO NOT `curl POST /evaluators`, and DO NOT spawn a Prince. Signal QA via `{milestone_ready_path}`, WAIT for `{qa_verdict_path}`, then WAIT for `{prince_verdict_path}` before you push.

1. You MUST execute the QA Milestone Handoff block below exactly as written. Treat Step 2 of that handoff as blocking.
2. You MUST wait for the Evaluator verdict by polling `{qa_verdict_path}` inline. You MUST NOT use `/loop`.
   ```bash
   while [ ! -f "{qa_verdict_path}" ]; do
     curl -fsS -X POST "http://localhost:18800/api/sessions/{session_id}/heartbeat" \
       -H "Content-Type: application/json" \
       -d '{{"agent_id":"queen","status":"working","summary":"Waiting for Evaluator verdict"}}'
     sleep 30
   done
   cat "{qa_verdict_path}"
   ```
3. You MUST inspect the verdict.
   - If it says `PASS` or `FAIL`, the Prince automatically takes over remediation of the QA findings. Continue to Step 4.
   - If it says `BLOCKED`, QA could not produce a usable verdict (read the rationale — typically a missing UI/host or a transport failure). STOP. Do NOT push. Surface to the operator (they will force-pass / force-fail).
4. You MUST wait for the Prince to finish remediation by polling `{prince_verdict_path}` inline. The Prince reads the QA findings, fixes them with its OWN fix team, and self-certifies. You MUST NOT spawn Reconciler or Resolver workers for QA findings — remediating QA findings is the Prince's job, not yours.
   ```bash
   while [ ! -f "{prince_verdict_path}" ]; do
     curl -fsS -X POST "http://localhost:18800/api/sessions/{session_id}/heartbeat" \
       -H "Content-Type: application/json" \
       -d '{{"agent_id":"queen","status":"working","summary":"Waiting for Prince remediation"}}'
     sleep 30
   done
   cat "{prince_verdict_path}"
   ```
   - If the Prince verdict is `PASS`/`DONE`, continue to Step 5.
   - If the Prince verdict is `BLOCKED`, STOP. Do NOT push. Surface to the operator.
5. You MUST commit and push the PR branch. This triggers CodeRabbit and Gemini external reviewers.
6. You MUST wait 10 minutes, then collect EXTERNAL PR review comments and resolve them. The Reconciler/Resolver workers here are for PR review comments ONLY — a separate concern from the QA findings the Prince already handled. Whenever unresolved PR comments remain, spawn them, integrate their fixes, and return to Step 5:
   ```bash
   gh api repos/<owner>/<repo>/issues/<pr-number>/comments
   gh api repos/<owner>/<repo>/pulls/<pr-number>/comments

   curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
     -H "Content-Type: application/json" \
     -d '{{"role_type":"reconciler","cli":"<configured-cli>","name":"Reconciler","description":"Consolidate external PR review comments into one fix list"}}'

   curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
     -H "Content-Type: application/json" \
     -d '{{"role_type":"resolver","cli":"<configured-cli>","name":"Resolver 1","description":"Fix HIGH/MEDIUM external PR review comments from the reconciled list"}}'
   ```
7. You MUST call `POST /api/sessions/{session_id}/complete` only after QA is resolved, the Prince has certified `PASS`, the latest push has aged at least 10 minutes, and there are no new unresolved PR comments.
"#,
            milestone_ready_path = milestone_ready_path,
            qa_verdict_path = qa_verdict_path,
            prince_verdict_path = prince_verdict_path,
            session_id = session_id,
        )
    }

    fn session_root_path(project_path: &Path, session_id: &str) -> PathBuf {
        project_path.join(".hive-manager").join(session_id)
    }

    /// Roughly one adversarial QA agent for every two of the Queen's coding workers
    /// (`ceil(worker_count / 2)`), computed without overflow. A hive with no coding
    /// workers gets none.
    fn adversarial_worker_count(worker_count: u8) -> u8 {
        (worker_count / 2) + (worker_count % 2)
    }

    fn build_evaluator_qa_plan(
        default_config: &AgentConfig,
        qa_workers: &[QaWorkerConfig],
        worker_count: u8,
    ) -> (String, String, String, String) {
        let mut configured_workers = if qa_workers.is_empty() {
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

        let configured_adversarial_count = configured_workers
            .iter()
            .filter(|worker| worker.specialization.eq_ignore_ascii_case("adversarial"))
            .count();
        let adversarial_target = Self::adversarial_worker_count(worker_count) as usize;

        // Adversarial agents (~1 per 2 coding workers) probe for the edge cases,
        // races, and unhandled errors the happy-path specialists miss. Manually
        // configured adversarial workers count toward, rather than suppress, the target.
        for _ in configured_adversarial_count..adversarial_target {
            configured_workers.push(QaWorkerConfig {
                specialization: "adversarial".to_string(),
                cli: default_config.cli.clone(),
                model: default_config.model.clone(),
                label: Some(Self::qa_worker_label("adversarial").to_string()),
                flags: None,
            });
        }

        let mut command_block = String::new();
        for (index, worker) in configured_workers.iter().enumerate() {
            let label = worker
                .label
                .as_deref()
                .unwrap_or(Self::qa_worker_label(&worker.specialization));
            let payload = serde_json::to_string(worker)
                .unwrap_or_else(|_| {
                    format!(
                        r#"{{"specialization":"{}","cli":"{}"}}"#,
                        worker.specialization, worker.cli
                    )
                })
                .replace('\'', "'\\''");

            command_block.push_str(&format!(
                "   # {}. {} worker\n   curl -X POST \"{{{{api_base_url}}}}/api/sessions/{{{{session_id}}}}/qa-workers\" \\\n     -H \"Content-Type: application/json\" \\\n     -d '{}'\n\n",
                index + 1,
                label,
                payload,
            ));
        }

        let intro = if qa_workers.is_empty() {
            format!(
                "You start with NO QA workers. You MUST spawn all {} QA workers listed below (UI, API, accessibility, plus adversarial coverage) before you grade any criterion.",
                configured_workers.len()
            )
        } else {
            format!(
                "You start with NO QA workers. You MUST spawn the configured QA workers below ({} total) before you grade any criterion.",
                configured_workers.len()
            )
        };
        let spawn_plan = format!("```bash\n{}   ```", command_block,);
        let coverage_rule = if qa_workers.is_empty() {
            "You MUST NOT skip any specialization. Every milestone requires full coverage."
                .to_string()
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
        worker_count: u8,
        execution_workspace: &str,
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
            Self::build_evaluator_qa_plan(config, qa_workers, worker_count);
        let required_protocol = Self::evaluator_required_protocol(session_id);

        let mut variables = HashMap::new();
        variables.insert(
            "custom_instructions".to_string(),
            custom_instructions.to_string(),
        );
        variables.insert("default_cli".to_string(), config.cli.clone());
        variables.insert("default_model".to_string(), default_model.to_string());
        variables.insert("default_model_field".to_string(), default_model_field);
        variables.insert("default_model_suffix".to_string(), default_model_suffix);
        variables.insert("required_protocol".to_string(), required_protocol);
        variables.insert("qa_worker_intro".to_string(), qa_worker_intro);
        variables.insert("qa_worker_spawn_plan".to_string(), qa_worker_spawn_plan);
        variables.insert("qa_worker_count".to_string(), qa_worker_count);
        variables.insert(
            "execution_workspace".to_string(),
            execution_workspace.to_string(),
        );
        variables.insert(
            "qa_worker_coverage_rule".to_string(),
            qa_worker_coverage_rule,
        );

        if smoke_test {
            variables.insert(
                "idle_poll_interval".to_string(),
                format_poll_label(SMOKE_IDLE_POLL_INTERVAL),
            );
            variables.insert(
                "idle_poll_secs".to_string(),
                SMOKE_IDLE_POLL_INTERVAL.as_secs().to_string(),
            );
            variables.insert(
                "active_poll_interval".to_string(),
                format_poll_label(SMOKE_ACTIVE_POLL_INTERVAL),
            );
            variables.insert(
                "active_poll_secs".to_string(),
                SMOKE_ACTIVE_POLL_INTERVAL.as_secs().to_string(),
            );
            variables.insert(
                "evaluator_first_poll_interval".to_string(),
                format_poll_label(SMOKE_EVALUATOR_FIRST_POLL_INTERVAL),
            );
            variables.insert(
                "evaluator_first_poll_secs".to_string(),
                SMOKE_EVALUATOR_FIRST_POLL_INTERVAL.as_secs().to_string(),
            );
        } else {
            variables.insert(
                "idle_poll_interval".to_string(),
                format_poll_label(STANDARD_IDLE_POLL_INTERVAL),
            );
            variables.insert(
                "idle_poll_secs".to_string(),
                STANDARD_IDLE_POLL_INTERVAL.as_secs().to_string(),
            );
            variables.insert(
                "active_poll_interval".to_string(),
                format_poll_label(STANDARD_ACTIVE_POLL_INTERVAL),
            );
            variables.insert(
                "active_poll_secs".to_string(),
                STANDARD_ACTIVE_POLL_INTERVAL.as_secs().to_string(),
            );
            variables.insert(
                "evaluator_first_poll_interval".to_string(),
                format_poll_label(STANDARD_EVALUATOR_FIRST_POLL_INTERVAL),
            );
            variables.insert(
                "evaluator_first_poll_secs".to_string(),
                STANDARD_EVALUATOR_FIRST_POLL_INTERVAL.as_secs().to_string(),
            );
        }

        Self::render_named_prompt("roles/evaluator", session_id, None, variables)
    }

    #[allow(dead_code)]
    fn build_prince_prompt(
        session_id: &str,
        config: &AgentConfig,
        principal_defaults: &AgentConfig,
        execution_workspace: &str,
        workspace_strategy: WorkspaceStrategy,
        smoke_test: bool,
    ) -> String {
        let custom_instructions = config.initial_prompt.as_deref().unwrap_or(
            "You MUST resolve every QA finding with your fix team before the Queen pushes, then self-certify PASS (or BLOCKED if you cannot).",
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
        let fixer_model = principal_defaults
            .model
            .as_deref()
            .or_else(|| CliRegistry::default_model(&principal_defaults.cli))
            .unwrap_or("");
        let fixer_model_field = if fixer_model.is_empty() {
            String::new()
        } else {
            format!(r#""model": "{}", "#, fixer_model)
        };
        let fixer_model_suffix = if fixer_model.is_empty() {
            String::new()
        } else {
            format!(" ({})", fixer_model)
        };
        let fixer_flags_field = format!(
            r#""flags": {}, "#,
            serde_json::to_string(&principal_defaults.flags).unwrap_or_else(|_| "[]".to_string())
        );
        let integration_protocol = match workspace_strategy {
            WorkspaceStrategy::SharedCell => format!(
                "Fixers run in the shared execution workspace `{execution_workspace}`. Their edits are already present there: do not merge or cherry-pick fixer branches. Wait for every fixer, inspect the shared diff, and rerun the relevant checks before certifying. The Queen owns final commit and push authority."
            ),
            WorkspaceStrategy::IsolatedCell => format!(
                "Each fixer runs in an isolated `hive/{session_id}/worker-N` worktree. Before certifying, obtain each completed fixer's commit SHA and integrate it into `{execution_workspace}` with `git -C \"{execution_workspace}\" cherry-pick <sha>` (or an equivalent explicit integration), resolve conflicts, and rerun the relevant checks there. The Queen owns final push authority."
            ),
            WorkspaceStrategy::None => format!(
                "This session has no managed git worktrees. Fixers edit `{execution_workspace}` directly. Do not invent branches, merges, or cherry-picks; inspect the resulting files and rerun the relevant checks before certifying."
            ),
        };

        let mut variables = HashMap::new();
        variables.insert(
            "custom_instructions".to_string(),
            custom_instructions.to_string(),
        );
        variables.insert("default_cli".to_string(), config.cli.clone());
        variables.insert("default_model".to_string(), default_model.to_string());
        variables.insert("default_model_field".to_string(), default_model_field);
        variables.insert("default_model_suffix".to_string(), default_model_suffix);
        variables.insert("fixer_cli".to_string(), principal_defaults.cli.clone());
        variables.insert("fixer_model".to_string(), fixer_model.to_string());
        variables.insert("fixer_model_field".to_string(), fixer_model_field);
        variables.insert("fixer_model_suffix".to_string(), fixer_model_suffix);
        variables.insert("fixer_flags_field".to_string(), fixer_flags_field);
        variables.insert(
            "execution_workspace".to_string(),
            execution_workspace.to_string(),
        );
        variables.insert("integration_protocol".to_string(), integration_protocol);
        variables.insert(
            "required_protocol".to_string(),
            Self::prince_required_protocol(session_id),
        );

        let (idle_secs, active_secs) = if smoke_test {
            (SMOKE_IDLE_POLL_INTERVAL, SMOKE_ACTIVE_POLL_INTERVAL)
        } else {
            (STANDARD_IDLE_POLL_INTERVAL, STANDARD_ACTIVE_POLL_INTERVAL)
        };
        variables.insert(
            "idle_poll_secs".to_string(),
            idle_secs.as_secs().to_string(),
        );
        variables.insert(
            "active_poll_secs".to_string(),
            active_secs.as_secs().to_string(),
        );

        Self::render_named_prompt("roles/prince", session_id, None, variables)
    }

    #[allow(dead_code)]
    fn build_qa_worker_prompt(
        session_id: &str,
        index: u8,
        specialization: &str,
        config: &AgentConfig,
        auth: &AuthStrategy,
        execution_workspace: &str,
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
            "adversarial" => (
                "roles/qa-worker-adversarial",
                "Attack the implementation: hunt edge cases, race conditions, malformed input, boundary values, and unhandled errors the happy-path QA workers miss. Report criterion-numbered defects with a concrete reproduction.",
            ),
            _ => (
                "roles/qa-worker-api",
                "Exercise the API surface directly, include concrete request and response evidence, and fail ambiguous behavior.",
            ),
        };

        let custom_instructions = config.initial_prompt.as_deref().unwrap_or(default_guidance);

        let mut variables = HashMap::new();
        variables.insert("qa_worker_index".to_string(), index.to_string());
        let qa_worker_agent_id = format!("{}-qa-worker-{}", session_id, index);
        variables.insert(
            "qa_worker_agent_id".to_string(),
            qa_worker_agent_id.clone(),
        );
        variables.insert(
            "qa_worker_completed_heartbeat".to_string(),
            heartbeat_snippet(
                "http://localhost:18800",
                session_id,
                &qa_worker_agent_id,
                "completed",
                "Completed QA assignment",
            ),
        );
        variables.insert(
            "custom_instructions".to_string(),
            custom_instructions.to_string(),
        );
        variables.insert(
            "supports_chrome".to_string(),
            (specialization == "ui" && config.cli == "claude").to_string(),
        );
        variables.insert(
            "execution_workspace".to_string(),
            execution_workspace.to_string(),
        );

        auth.apply_prompt_variables(session_id, &mut variables);

        Self::render_named_prompt(template_name, session_id, None, variables)
    }

    fn qa_worker_label(specialization: &str) -> &'static str {
        match specialization {
            "ui" => "UI QA",
            "api" => "API QA",
            "a11y" => "A11Y QA",
            "adversarial" => "Adversarial QA",
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
            .unwrap_or_else(|_| {
                format!(
                    "Template '{}' failed to render for session {}",
                    template_name, session_id
                )
            })
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
            let name = if v.name.trim().is_empty() {
                format!("Variant {}", index)
            } else {
                v.name.trim().to_string()
            };
            variant_table.push_str(&format!("| {} | {} | {} |\n", index, name, v.cli));
        }

        // Determine phase 0 based on whether a task was provided
        let phase0 = if task_description.trim().is_empty() {
            String::from(
                r#"## PHASE 0: Gather Task (FIRST STEP)

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

"#,
            )
        } else if task_description.trim().starts_with('#')
            || task_description.trim().parse::<u32>().is_ok()
        {
            let issue_num = task_description.trim().trim_start_matches('#');
            format!(
                r#"## PHASE 0: Fetch GitHub Issue

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

"#,
                issue_num, issue_num
            )
        } else {
            format!(
                r#"## PHASE 0: Task Provided

The user wants to work on:

**{}**

**Proceed directly to PHASE 1.**

---

"#,
                task_description
            )
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

    fn build_debate_master_planner_prompt(
        session_id: &str,
        topic: &str,
        debaters: &[DebateDebaterConfig],
        rounds: u8,
    ) -> String {
        let debater_table = debaters
            .iter()
            .enumerate()
            .map(|(idx, debater)| {
                let name = if debater.name.trim().is_empty() {
                    format!("Debater {}", idx + 1)
                } else {
                    debater.name.trim().to_string()
                };
                let stance = debater
                    .stance
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("No explicit stance");
                format!("| {} | {} | {} | {} |", idx + 1, name, stance, debater.cli)
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            r#"# Master Planner - Debate Mode

You are the Master Planner for a Debate session.

## Session Info

- Session ID: {session_id}
- Mode: Debate
- Rounds: {rounds}
- Plan Output: `.hive-manager/{session_id}/plan.md`

## Topic

{topic}

## Debaters

| # | Name | Stance | CLI |
|---|------|--------|-----|
{debater_table}

## Mission

Write a concise debate plan to `.hive-manager/{session_id}/plan.md`:

```markdown
# Debate Plan

## Topic
[topic]

## Debater Stances
[stance framing]

## Round Plan
[what each round should focus on]

## Judging Criteria
- [ ] Argument quality
- [ ] Rebuttal strength
- [ ] Evidence and specificity
- [ ] Consistency
```

Do not run the debate. Stop after writing the plan.
"#,
            session_id = session_id,
            rounds = rounds,
            topic = topic,
            debater_table = debater_table,
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
            variant_info.push_str(&format!(
                "| {} | {} | `{}` | {} | {} |\n",
                v.index, v.name, v.agent_id, v.branch, v.worktree_path
            ));
            task_files.push_str(&format!(
                "- Variant {} ({}): `{}`\n",
                v.index, v.name, v.task_file
            ));
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

| # | Name | Agent ID | Branch | Worktree |
|---|------|----------|--------|----------|
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
- `mark-worker-status.md` — Mark each independently verified variant complete
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
        let qa_verdict_path = Self::prompt_path(&session_root.join("peer").join("qa-verdict.json"));
        let contracts_dir = Self::prompt_path(&session_root.join("contracts"));
        let contract_path =
            Self::prompt_path(&session_root.join("contracts").join("milestone-1.md"));

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
    fn build_master_planner_prompt(
        session_id: &str,
        user_prompt: &str,
        planner_config: &AgentConfig,
        workers: &[AgentConfig],
        execution_policy: &HiveExecutionPolicy,
        project_path: &Path,
        planner_workspace_path: &Path,
    ) -> String {
        let role = ContractRole::MasterPlanner;
        let policy = &execution_policy.queen_delegation;
        let card = CliRegistry::infer_capabilities(&planner_config.cli);
        let delegation_authorized = CliRegistry::native_delegation_authorized(&card, policy);
        let role_kernel = render_role_kernel(role);
        let capability_card = render_capability_card(
            planner_config,
            role,
            &card,
            policy,
            &execution_policy.workspace_strategy,
            delegation_authorized,
        );
        let delegation = render_delegation_guidance(role, policy, delegation_authorized);
        let workspace = render_workspace_contract(role, &execution_policy.workspace_strategy);
        let objective = if user_prompt.trim().is_empty() {
            "No objective was supplied. Ask the operator for one, then stop until it is provided."
        } else {
            user_prompt.trim()
        };
        let plan_path =
            Self::prompt_path(&Self::session_root_path(project_path, session_id).join("plan.md"));
        let planner_workspace_path = Self::prompt_path(planner_workspace_path);
        let deliverables = [
            plan_path.as_str(),
            "One build-ready execution contract organized by coherent workstreams",
            "Evidence-backed ownership, dependency, validation, and stop-condition decisions",
        ];
        let validation = [
            "Every acceptance criterion maps to at least one validation gate",
            "Overlapping files and serialized hotspots have one explicit owner/order",
            "The plan is implementable without inventing missing authority",
        ];
        let stop_conditions = [
            "The objective or acceptance criteria remain materially ambiguous",
            "Required repository or issue context is unavailable",
            "A safe ownership boundary cannot be defined without operator input",
        ];
        let assignment = render_assignment_contract(&AssignmentSpec {
            objective,
            access: "Read-only repository investigation; write only the session plan artifact",
            owned_scope: "Planning artifacts under the current session; no production-code edits or git mutations",
            authoritative_input: "The operator objective, repository state, project DNA, learnings, and referenced issue/spec material",
            deliverables: &deliverables,
            validation: &validation,
            stop_conditions: &stop_conditions,
        });

        let policy_label = match policy.mode {
            crate::domain::NativeDelegationMode::Disabled => "disabled",
            crate::domain::NativeDelegationMode::Auto => "auto",
            crate::domain::NativeDelegationMode::Encouraged => "encouraged",
        };
        let mut principal_roster = String::new();
        for (index, principal) in workers.iter().enumerate() {
            let label = principal
                .role
                .as_ref()
                .map(|role| role.label.as_str())
                .unwrap_or("Coding Principal");
            let model = principal.model.as_deref().unwrap_or("harness default");
            let flags =
                serde_json::to_string(&principal.flags).unwrap_or_else(|_| "[]".to_string());
            let principal_card = CliRegistry::infer_capabilities(&principal.cli);
            let authorized = CliRegistry::native_delegation_authorized(
                &principal_card,
                &execution_policy.principal_delegation,
            );
            principal_roster.push_str(&format!(
                "| Principal {} | {} | `{}` | `{}` | `{}` | {} ({}) |\n",
                index + 1,
                label,
                principal.cli,
                model,
                flags,
                match execution_policy.principal_delegation.mode {
                    crate::domain::NativeDelegationMode::Disabled => "disabled",
                    crate::domain::NativeDelegationMode::Auto => "auto",
                    crate::domain::NativeDelegationMode::Encouraged => "encouraged",
                },
                if authorized {
                    "authorized"
                } else {
                    "not authorized"
                },
            ));
        }
        if principal_roster.is_empty() {
            principal_roster.push_str("| (none configured) | - | - | - | - | - |\n");
        }

        format!(
            r#"# Master Planner - Hive Execution Contract

{role_kernel}

{capability_card}

{delegation}

{workspace}

{assignment}

## Session

- Session ID: `{session_id}`
- Plan output: `{plan_path}`
- Runtime CWD: `{planner_workspace_path}`
- Queen delegation policy: {policy_label}

Before planning, inspect `.ai-docs/project-dna.md`, `.ai-docs/learnings.jsonl`, the current repository state, and any referenced issue or specification. If the objective is missing, ask once and stop. If it is an issue reference, resolve its requirements before partitioning work.

## Configured Managed Principals

This roster is available implementation capacity, not a required task count. Design workstreams from the objective and coupling boundaries; do not manufacture one task per roster slot.

| Slot | Role | CLI | Model | Flags | Native delegation |
|------|------|-----|-------|-------|-------------------|
{principal_roster}
## Planning Method

1. Establish the objective, non-goals, acceptance criteria, and authoritative evidence.
2. Investigate the repository directly. Use native read-only scouts only when the Capability Card says delegation is authorized; choose the number from genuinely independent questions and wait for every scout before synthesis. Never launch unmanaged CLI subprocesses.
3. Partition by coherent workstream and file ownership, not by agent count. Identify shared files, migrations, schemas, generated artifacts, lockfiles, and git operations that must be serialized.
4. Define dependency order, integration gates, validation commands, observable evidence, risks, and explicit stop/escalation conditions.
5. Write exactly one plan to `{plan_path}` and stop. Do not implement, edit production files, create branches, commit, push, or launch managed principals.

## Required Plan Shape

- Objective, constraints, non-goals, and acceptance criteria
- Evidence and repository findings
- Coherent workstreams with owned paths and authoritative inputs
- Ownership matrix and serialized hotspots
- Dependency and integration order
- Validation gates with commands/evidence
- Risks, unresolved decisions, and stop conditions
- Recommended principal assignment as a suggestion, not a roster-count invariant

End with `PLAN READY FOR REVIEW`. Produce no second plan and no implementation changes."#,
            role_kernel = role_kernel,
            capability_card = capability_card,
            delegation = delegation,
            workspace = workspace,
            assignment = assignment,
            session_id = session_id,
            plan_path = plan_path,
            planner_workspace_path = planner_workspace_path,
            policy_label = policy_label,
            principal_roster = principal_roster.trim_end(),
        )
    }

    /// Build the Master Planner's prompt for Swarm mode with planner and worker information
    fn build_swarm_master_planner_prompt(
        session_id: &str,
        user_prompt: &str,
        planner_count: u8,
        workers_per_planner: &[AgentConfig],
    ) -> String {
        let workers_per = workers_per_planner.len();
        let total_workers = planner_count as usize * workers_per;

        // Build planner table
        let mut planner_table = String::new();
        let domains = [
            "backend",
            "frontend",
            "testing",
            "infrastructure",
            "documentation",
            "security",
            "performance",
            "integration",
        ];

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
            let role_label = worker_config
                .role
                .as_ref()
                .map(|r| r.label.clone())
                .unwrap_or_else(|| format!("Worker {}", index));
            worker_info.push_str(&format!(
                "| {} | {} | {} |\n",
                index, role_label, worker_config.cli
            ));
        }

        // Determine phase 0 based on whether a task was provided
        let phase0 = if user_prompt.trim().is_empty() {
            String::from(
                r#"## PHASE 0: Gather Task (FIRST STEP)

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

"#,
            )
        } else if user_prompt.trim().starts_with('#') || user_prompt.trim().parse::<u32>().is_ok() {
            let issue_num = user_prompt.trim().trim_start_matches('#');
            format!(
                r#"## PHASE 0: Fetch GitHub Issue

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

"#,
                issue_num, issue_num
            )
        } else {
            format!(
                r#"## PHASE 0: Task Provided

The user wants to work on:

**{}**

**Proceed directly to PHASE 1.**

---

"#,
                user_prompt
            )
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

Spawn each scout via the Task tool calling Codex through Bash. Launch all 3 in PARALLEL via a single message with three Task calls.

### Scout 1 - Codex GPT-5.5 Low (Code Structure)

Task(subagent_type="general-purpose", prompt="You are a codebase investigation agent. IMMEDIATELY run: codex exec --dangerously-bypass-approvals-and-sandbox -m gpt-5.5 -c model_reasoning_effort=\"low\" 'Analyze the codebase structure for: [TASK]. List relevant files by priority.' Return file paths with priority notes.")

### Scout 2 - Codex GPT-5.5 Low (Implementation Patterns)

Task(subagent_type="general-purpose", prompt="You are a codebase investigation agent. IMMEDIATELY run: codex exec --dangerously-bypass-approvals-and-sandbox -m gpt-5.5 -c model_reasoning_effort=\"low\" 'Identify implementation patterns relevant to: [TASK]. Focus on existing conventions, helpers, and shared abstractions.' Return file paths with pattern notes.")

### Scout 3 - Codex GPT-5.5 Medium (Related Code)

Task(subagent_type="general-purpose", prompt="You are a codebase investigation agent. IMMEDIATELY run: codex exec --dangerously-bypass-approvals-and-sandbox -m gpt-5.5 -c model_reasoning_effort=\"medium\" 'Find code related to: [TASK]. Identify entry points, test files, dependencies.' Return file paths with notes.")

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
            let role_label = worker_config
                .role
                .as_ref()
                .map(|r| r.label.clone())
                .unwrap_or_else(|| format!("Worker {}", index));
            let cli = &worker_config.cli;

            worker_table.push_str(&format!(
                "| Worker {} | {} | {} |\n",
                index, role_label, cli
            ));

            let priority = if index == 1 {
                "HIGH"
            } else if index == 2 {
                "MEDIUM"
            } else {
                "LOW"
            };
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
                dependencies.push_str(&format!(
                    "- Task {} depends on Task {} completing.\n",
                    index,
                    index - 1
                ));
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
                let label = qw
                    .label
                    .as_deref()
                    .unwrap_or(Self::qa_worker_label(&qw.specialization));
                qa_table.push_str(&format!(
                    "| QA Worker {} | {} | {} | {} |\n",
                    idx, label, qw.specialization, qw.cli
                ));
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
and coordinates QA workers to validate the work. The Evaluator also auto-adds an **Adversarial**
QA agent (~1 per 2 coding workers) on top of the list below. A **Prince** peer is spawned
alongside the Evaluator: it owns remediation of QA findings and self-certifies before the PR is
pushed, so the QA verdict gates through Prince clearance.

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

### Prince Remediation (auto-spawned peer):
The QA verdict transitions the session to **PrinceRemediation** (not QaPassed). The Prince peer
reads the verdict from `.hive-manager/{session_id}/peer/qa-verdict.json`. For a clean smoke PASS there
are no findings, so the Prince self-certifies immediately, clearing the gate to QaPassed:
1. `curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/prince/verdict" -H "Content-Type: application/json" -d '{{"verdict":"PASS","rationale":"smoke - no findings to remediate"}}'`
The Queen waits for `.hive-manager/{session_id}/peer/prince-verdict.json` before completing.
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
                 5. {} QA worker(s) plus an auto-added adversarial agent exercise their specialization\n\
                 6. Evaluator submits verdict via POST /api/sessions/{session_id}/qa/verdict\n\
                 7. Prince peer spawns, reads the verdict, and self-certifies via POST /api/sessions/{session_id}/prince/verdict\n\
                 8. Session reaches QaPassed only after Prince clearance (PrinceRemediation -> QaPassed)",
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
1. Send heartbeat:
   ```bash
   {smoke_worker_start_heartbeat}
   ```
2. Post message to queen: `curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/conversations/queen/append" -H "Content-Type: application/json" -d '{{"from":"worker-1","content":"Worker 1 reporting in. Smoke test task started."}}'`
3. Post to shared: `curl -s -X POST "http://localhost:18800/api/sessions/{session_id}/conversations/shared/append" -H "Content-Type: application/json" -d '{{"from":"worker-1","content":"Worker 1 completed conversation smoke test."}}'`
4. Send completed heartbeat:
   ```bash
   {smoke_worker_completed_heartbeat}
   ```

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
            smoke_worker_start_heartbeat = heartbeat_snippet(
                "http://localhost:18800",
                session_id,
                &format!("{session_id}-worker-1"),
                "working",
                "Starting smoke test",
            ),
            smoke_worker_completed_heartbeat = heartbeat_snippet(
                "http://localhost:18800",
                session_id,
                &format!("{session_id}-worker-1"),
                "completed",
                "Smoke test done",
            ),
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

        let domains = [
            "backend",
            "frontend",
            "testing",
            "infrastructure",
            "documentation",
            "security",
            "performance",
            "integration",
        ];

        for i in 0..planner_count {
            let index = i + 1;
            let domain = domains.get(i as usize).unwrap_or(&"general");
            planner_table.push_str(&format!(
                "| Planner {} | {} | {} workers |\n",
                index, domain, workers_per
            ));

            let priority = if index == 1 {
                "HIGH"
            } else if index == 2 {
                "MEDIUM"
            } else {
                "LOW"
            };
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
            worker_breakdown.push_str(&format!(
                "\n### Planner {} - {} Domain\n\n",
                planner_index, domain
            ));

            for (wi, worker_config) in workers_per_planner.iter().enumerate() {
                let worker_index = wi + 1;
                let role_label = worker_config
                    .role
                    .as_ref()
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
                let label = qw
                    .label
                    .as_deref()
                    .unwrap_or(Self::qa_worker_label(&qw.specialization));
                qa_info.push_str(&format!(
                    "| QA Worker {} | {} | {} | {} |\n",
                    i + 1,
                    label,
                    qw.specialization,
                    qw.cli
                ));
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
    /// Render a Queen prompt from a named template (e.g. `queen-research`),
    /// supplying the standard Queen template variables plus any caller-provided
    /// extras (e.g. `global_wiki_path`).
    ///
    /// Standard variables match those used by the `queen-hive` template:
    /// `session_id`, `api_base_url`, `workers_list`, `queen_heartbeat_snippet`,
    /// and `task`. Caller extras win on key collision.
    fn build_templated_queen_prompt(
        template_name: &str,
        session_id: &str,
        workers: &[AgentConfig],
        user_prompt: Option<&str>,
        extra_vars: HashMap<String, String>,
    ) -> String {
        const API_BASE_URL: &str = "http://localhost:18800";

        // Build the researcher roster table. These workers are NOT pre-spawned: the
        // Queen spawns the ones it needs on demand via the spawn-worker tool, so the
        // table lists roster slots with the CLI + model to spawn each with, rather than
        // live worker IDs (which the system assigns sequentially at spawn time).
        let mut workers_list =
            String::from("| Slot | Role | CLI | Model |\n|------|------|-----|-------|\n");
        for (i, worker_config) in workers.iter().enumerate() {
            let slot = i + 1;
            let role_label = worker_config
                .role
                .as_ref()
                .map(|r| r.label.clone())
                .unwrap_or_else(|| "Researcher".to_string());
            let model = worker_config
                .model
                .clone()
                .unwrap_or_else(|| "(session default)".to_string());
            workers_list.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                slot, role_label, worker_config.cli, model
            ));
        }

        let mut variables = HashMap::new();
        variables.insert("api_base_url".to_string(), API_BASE_URL.to_string());
        variables.insert("workers_list".to_string(), workers_list);
        variables.insert(
            "queen_heartbeat_snippet".to_string(),
            heartbeat_snippet(
                API_BASE_URL,
                session_id,
                "queen",
                "working",
                "Coordinating researchers",
            ),
        );
        variables.insert(
            "task".to_string(),
            user_prompt
                .unwrap_or("Coordinate the researchers and synthesize their findings.")
                .to_string(),
        );
        // Caller-provided extras (e.g. global_wiki_path) take precedence.
        for (k, v) in extra_vars {
            variables.insert(k, v);
        }

        Self::render_named_prompt(
            template_name,
            session_id,
            user_prompt.map(|s| s.to_string()),
            variables,
        )
    }

    fn build_queen_master_prompt(
        queen_config: &AgentConfig,
        project_path: &Path,
        queen_workspace_path: &Path,
        session_id: &str,
        workers: &[AgentConfig],
        user_prompt: Option<&str>,
        has_plan: bool,
        has_evaluator: bool,
        execution_policy: &HiveExecutionPolicy,
    ) -> String {
        let role = ContractRole::Queen;
        let policy = &execution_policy.queen_delegation;
        let card = CliRegistry::infer_capabilities(&queen_config.cli);
        let delegation_authorized = CliRegistry::native_delegation_authorized(&card, policy);
        let role_kernel = render_role_kernel(role);
        let capability_card = render_capability_card(
            queen_config,
            role,
            &card,
            policy,
            &execution_policy.workspace_strategy,
            delegation_authorized,
        );
        let delegation = render_delegation_guidance(role, policy, delegation_authorized);
        let workspace_contract =
            render_workspace_contract(role, &execution_policy.workspace_strategy);

        let session_root = Self::session_root_path(project_path, session_id);
        let plan_path = Self::prompt_path(&session_root.join("plan.md"));
        let tools_dir = Self::prompt_path(&session_root.join("tools"));
        let coordination_log_path = Self::prompt_path(&session_root.join("coordination.log"));
        let queen_workspace = Self::prompt_path(queen_workspace_path);
        let queen_conversation =
            Self::prompt_path(&session_root.join("conversations").join("queen.md"));
        let shared_conversation =
            Self::prompt_path(&session_root.join("conversations").join("shared.md"));

        let objective = user_prompt
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Execute the approved plan or coordinate the configured objective.");
        let owned_scope = format!(
            "Orchestration artifacts, integration, validation, and git state for the managed session rooted at {}",
            queen_workspace
        );
        let deliverables = [
            "Clear, non-overlapping principal assignments",
            "One reconciled implementation with validation evidence",
            "Completed QA and external-review gates when configured",
        ];
        let validation = [
            "Every accepted workstream has evidence from its assigned principal",
            "Shared files and git operations were serialized",
            "The integrated result satisfies the plan and operator objective",
        ];
        let stop_conditions = [
            "The plan requires authority the operator did not grant",
            "A principal reports a blocker that changes scope or acceptance criteria",
            "QA or Prince returns BLOCKED",
        ];
        let assignment = render_assignment_contract(&AssignmentSpec {
            objective,
            access: "Coordinate managed principals, inspect all session workspaces, maintain session control artifacts, and perform integration operations",
            owned_scope: &owned_scope,
            authoritative_input: "The operator objective, approved plan, repository state, principal evidence, QA verdicts, and review findings",
            deliverables: &deliverables,
            validation: &validation,
            stop_conditions: &stop_conditions,
        });

        let plan_section = if has_plan {
            format!(
                "## Approved Plan\n\nRead {} before assigning work. Preserve its acceptance criteria and dependency order; adjust principal count only when coupling or capacity warrants it.",
                plan_path
            )
        } else {
            "## Planning Basis\n\nNo generated plan is present. Derive the smallest coherent workstream set from the operator objective and repository evidence.".to_string()
        };

        let principal_policy_label = match execution_policy.principal_delegation.mode {
            crate::domain::NativeDelegationMode::Disabled => "disabled",
            crate::domain::NativeDelegationMode::Auto => "auto",
            crate::domain::NativeDelegationMode::Encouraged => "encouraged",
        };
        let mut principal_roster = String::new();
        for (offset, principal) in workers.iter().enumerate() {
            let index = offset + 1;
            let principal_id = format!("{session_id}-worker-{index}");
            let label = principal
                .role
                .as_ref()
                .map(|worker_role| worker_role.label.as_str())
                .unwrap_or("Coding Principal");
            let model = principal.model.as_deref().unwrap_or("harness default");
            let flags =
                serde_json::to_string(&principal.flags).unwrap_or_else(|_| "[]".to_string());
            let principal_card = CliRegistry::infer_capabilities(&principal.cli);
            let support = match principal_card.native_delegation {
                crate::domain::CapabilitySupport::Supported => "supported",
                crate::domain::CapabilitySupport::Unsupported => "unsupported",
                crate::domain::CapabilitySupport::Unknown => "unknown",
            };
            let authorized = CliRegistry::native_delegation_authorized(
                &principal_card,
                &execution_policy.principal_delegation,
            );
            let principal_workspace = match execution_policy.workspace_strategy {
                WorkspaceStrategy::SharedCell => queen_workspace_path.to_path_buf(),
                WorkspaceStrategy::IsolatedCell => project_path
                    .join(".hive-manager")
                    .join("worktrees")
                    .join(session_id)
                    .join(format!("worker-{index}")),
                WorkspaceStrategy::None => project_path.to_path_buf(),
            };
            let principal_workspace = Self::prompt_path(&principal_workspace);
            let task_file = Self::prompt_path(
                &PathBuf::from(&principal_workspace)
                    .join(".hive-manager")
                    .join("tasks")
                    .join(format!("worker-{index}-task.md")),
            );
            principal_roster.push_str(&format!(
                "| {principal_id} | {label} | {cli} | {model} | {flags} | {support}; {principal_policy_label} ({authorization}) | {principal_workspace} | {task_file} |\n",
                cli = principal.cli,
                flags = flags,
                authorization = if authorized { "authorized" } else { "not authorized" },
            ));
        }
        if principal_roster.is_empty() {
            principal_roster.push_str("| None configured | - | - | - | - | - | - | - |\n");
        }

        let topology_instructions = match execution_policy.workspace_strategy {
            WorkspaceStrategy::SharedCell => format!(
                "## Shared Cell Integration\n\nThe Queen and managed principals run in the same backend-created worktree at {queen_workspace}. Assign explicit, non-overlapping paths and serialize shared files. Principal edits are immediately visible. Principals do not commit. Review the combined diff, run integration validation, then commit from the current backend-created hive/{session_id}/primary branch. Do not create, rename, or switch branches."
            ),
            WorkspaceStrategy::IsolatedCell => format!(
                "## Isolated Cell Integration\n\nThe Queen runs at {queen_workspace}. Each principal owns the workspace and task path in the roster and commits only its completed assignment on its backend-created hive/{session_id}/worker-N branch. Inspect and validate each commit, then integrate it into the current backend-created Queen branch in dependency order. Resolve conflicts centrally. Do not create, rename, or switch managed branches."
            ),
            WorkspaceStrategy::None => format!(
                "## Current Checkout Coordination\n\nAgents run in the operator checkout rooted at {queen_workspace}. Preserve operator changes. Do not create, switch, commit, or push branches without explicit operator authorization."
            ),
        };

        let required_protocol = Self::queen_required_protocol(&session_root, has_evaluator);
        let qa_milestone_handoff = if has_evaluator {
            Self::build_qa_milestone_handoff(session_id, &session_root, "managed principals")
        } else {
            String::new()
        };
        let post_workers_protocol =
            Self::queen_post_workers_protocol(session_id, &session_root, has_evaluator);
        let queen_heartbeat = heartbeat_snippet(
            "http://localhost:18800",
            session_id,
            "queen",
            "working",
            "Coordinating managed principals",
        );

        format!(
            r#"# Queen - Hive Meta-Harness

{role_kernel}

{capability_card}

{delegation}

{workspace_contract}

{assignment}

## Session

- Session ID: {session_id}
- Runtime CWD: {queen_workspace}
- Harness: {cli}
- Model: {model}
- Session tools: {tools_dir}
- Queen conversation: {queen_conversation}
- Shared conversation: {shared_conversation}

{required_protocol}

{plan_section}

## Managed Principal Roster

Managed principals are visible Hive agents with their own lifecycle and task contracts. Native children are private harness-managed lanes governed by the Capability Card; they are not substitutes for managed principals and must not create Hive Workers.

| ID | Role | Harness | Model | Flags (JSON) | Native delegation | Workspace | Task file |
|----|------|---------|-------|--------------|-------------------|-----------|-----------|
{principal_roster}

## Assignment and Coordination

1. Read the plan, project DNA, learnings, and current repository state.
2. Partition work by coherent ownership and dependencies, not by roster size.
3. Use the existing roster or POST /api/sessions/{session_id}/workers when a new visible principal is genuinely needed. Preserve that principal's exact harness, model, and flags array from the roster; do not drop effort or reasoning settings. Never launch unmanaged external CLI subprocesses.
4. Activate a principal by writing a precise objective, owned paths, authoritative inputs, deliverables, validation, and stop conditions to its task file, then set Status to ACTIVE.
5. Monitor heartbeats and the Queen/shared conversations. Review every principal result and evidence before integration.
6. Keep native Queen children read-only for planning, scouting, and review. Delegate implementation to managed principals.
7. The Queen coordinates and integrates; do not become a coding principal.

Heartbeat while coordinating:
{queen_heartbeat}

{topology_instructions}

## Learning Curation

Workers submit durable learnings through POST /api/sessions/{session_id}/learnings. Review GET /api/sessions/{session_id}/learnings and GET /api/sessions/{session_id}/project-dna after major phases and before the final PR. Curate durable conventions, decisions, failures, and architectural facts; remove duplicates and stale records.

{qa_milestone_handoff}

{post_workers_protocol}

Log every quality-reconciliation iteration to {coordination_log_path}:
{queen_quality_log}

## Operator Objective

{objective}

When the objective and every configured gate are complete, send an idle heartbeat and continue monitoring the Queen conversation."#,
            role_kernel = role_kernel,
            capability_card = capability_card,
            delegation = delegation,
            workspace_contract = workspace_contract,
            assignment = assignment,
            session_id = session_id,
            queen_workspace = queen_workspace,
            cli = queen_config.cli,
            model = queen_config.model.as_deref().unwrap_or("harness default"),
            tools_dir = tools_dir,
            queen_conversation = queen_conversation,
            shared_conversation = shared_conversation,
            required_protocol = required_protocol,
            plan_section = plan_section,
            principal_roster = principal_roster.trim_end(),
            queen_heartbeat = queen_heartbeat,
            topology_instructions = topology_instructions,
            qa_milestone_handoff = qa_milestone_handoff,
            post_workers_protocol = post_workers_protocol,
            coordination_log_path = coordination_log_path,
            queen_quality_log = Self::queen_quality_reconciliation_log_lines(has_evaluator),
            objective = objective,
        )
    }
    /// Build a worker's role prompt
    fn build_worker_prompt(
        index: u8,
        config: &AgentConfig,
        queen_id: &str,
        session_id: &str,
        project_path: &Path,
        workspace_path: &Path,
        execution_policy: &HiveExecutionPolicy,
    ) -> String {
        let role_name = config
            .role
            .as_ref()
            .map(|worker_role| worker_role.label.clone())
            .unwrap_or_else(|| format!("Coding Principal {index}"));
        let role_type = config
            .role
            .as_ref()
            .map(|worker_role| worker_role.role_type.to_ascii_lowercase())
            .unwrap_or_else(|| "general".to_string());
        let is_research = role_type == "researcher";
        let contract_role = if is_research {
            ContractRole::Researcher
        } else {
            ContractRole::Principal
        };
        let policy = &execution_policy.principal_delegation;
        let card = CliRegistry::infer_capabilities(&config.cli);
        let delegation_authorized = CliRegistry::native_delegation_authorized(&card, policy);
        let role_kernel = render_role_kernel(contract_role);
        let capability_card = render_capability_card(
            config,
            contract_role,
            &card,
            policy,
            &execution_policy.workspace_strategy,
            delegation_authorized,
        );
        let delegation = render_delegation_guidance(contract_role, policy, delegation_authorized);
        let workspace_contract =
            render_workspace_contract(contract_role, &execution_policy.workspace_strategy);

        let session_root = Self::session_root_path(project_path, session_id);
        let workspace_path = Self::prompt_path(workspace_path);
        let task_file_path = if execution_policy.workspace_strategy == WorkspaceStrategy::None {
            Self::session_task_file_path(project_path, session_id, index as usize)
        } else {
            PathBuf::from(&workspace_path)
                .join(".hive-manager")
                .join("tasks")
                .join(format!("worker-{index}-task.md"))
        };
        let task_file = Self::prompt_path(&task_file_path);
        let worker_conversation = Self::prompt_path(
            &session_root
                .join("conversations")
                .join(format!("worker-{index}.md")),
        );
        let queen_conversation =
            Self::prompt_path(&session_root.join("conversations").join("queen.md"));
        let shared_conversation =
            Self::prompt_path(&session_root.join("conversations").join("shared.md"));

        let role_description = match role_type.as_str() {
            "backend" => "Server-side logic, APIs, databases, and backend infrastructure.",
            "frontend" => "UI components, state management, styling, and user experience.",
            "coherence" => "Code consistency, API contracts, and cross-component integration.",
            "simplify" => "Code simplification, refactoring, and reducing complexity.",
            "reviewer" => "Deep code review across correctness, security, performance, architecture, and compatibility.",
            "reviewer-quick" => "Fast review for obvious defects, regressions, and maintainability issues.",
            "resolver" => "Resolve assigned review findings and document any intentionally skipped item with rationale.",
            "tester" => "Run the assigned validation suite, repair in-scope failures, and report unresolved evidence.",
            "code-quality" => "Resolve assigned external-review comments and verify the result.",
            "reconciler" => "Reconcile evaluator and external-review findings into one prioritized, deduplicated result.",
            "researcher" => "Investigate the assigned question read-only and return concise findings with evidence.",
            _ => "Complete the coherent implementation workstream assigned by the Queen.",
        };

        let scope_block = if is_research {
            Self::scope_block_read_only()
        } else {
            Self::scope_block(".")
        };
        let objective = config
            .initial_prompt
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Complete only the ACTIVE assignment in the authoritative task file.");
        let access = if is_research {
            "Read-only investigation; report through the session conversation and task file"
        } else {
            "Read the repository and modify only paths explicitly owned by the ACTIVE task contract"
        };
        let owned_scope = format!(
            "{} Workspace: {}. The task file is authoritative for narrower path ownership.",
            role_description, workspace_path
        );
        let authoritative_input = format!(
            "The ACTIVE task at {}, the approved plan, repository state, project DNA, and Queen messages",
            task_file
        );
        let principal_deliverables = [
            "Implemented changes inside the assigned ownership boundary",
            "Focused validation output and a concise completion report",
            "One durable learning record",
        ];
        let research_deliverables = [
            "Concise findings with file, source, or command evidence",
            "A clear answer to the assigned research question",
            "No project or git mutations",
        ];
        let principal_validation = [
            "Run the focused tests or checks named by the task",
            "Review the final diff for scope and unintended changes",
            "Confirm the delivery commit when using an isolated cell",
        ];
        let research_validation = [
            "Cite the evidence supporting each material conclusion",
            "Separate observed facts from inference",
            "Confirm that no project files or git state changed",
        ];
        let stop_conditions = [
            "The assignment is ambiguous or conflicts with another owner's paths",
            "Required inputs or permissions are unavailable",
            "A safe fix requires expanding scope beyond the task contract",
        ];
        let assignment = render_assignment_contract(&AssignmentSpec {
            objective,
            access,
            owned_scope: &owned_scope,
            authoritative_input: &authoritative_input,
            deliverables: if is_research {
                &research_deliverables
            } else {
                &principal_deliverables
            },
            validation: if is_research {
                &research_validation
            } else {
                &principal_validation
            },
            stop_conditions: &stop_conditions,
        });

        let agent_id = format!("{session_id}-worker-{index}");
        let activation_wait_heartbeat = heartbeat_snippet(
            "http://localhost:18800",
            session_id,
            &agent_id,
            "idle",
            "Waiting for task activation",
        );
        let polling_instructions = get_polling_instructions(
            &config.cli,
            &task_file,
            config
                .role
                .as_ref()
                .map(|worker_role| worker_role.role_type.as_str()),
            Some(&activation_wait_heartbeat),
        );
        let working_heartbeat = heartbeat_snippet(
            "http://localhost:18800",
            session_id,
            &agent_id,
            "working",
            "Executing assigned workstream",
        );
        let completed_heartbeat = heartbeat_snippet(
            "http://localhost:18800",
            session_id,
            &agent_id,
            "completed",
            "Completed assigned workstream",
        );

        let role_section = if is_research {
            "## Your Role: RESEARCHER (Read-Only)\n\nInvestigate and synthesize. Do not write production code, modify project files, or mutate git. Your deliverable is evidence-backed knowledge returned to the Queen."
        } else {
            "## Your Role: EXECUTOR\n\nYou are a managed coding principal with implementation authority only inside the ACTIVE assignment contract."
        };

        let validation_and_handoff_rule = if is_research {
            "Verify every material conclusion against cited evidence and confirm that the repository and git state remain unchanged. Do not commit."
        } else {
            match execution_policy.workspace_strategy {
                WorkspaceStrategy::SharedCell => {
                    "Run focused validation, review the owned diff, and leave the reviewed changes uncommitted for the Queen; the Queen owns the shared git state."
                }
                WorkspaceStrategy::IsolatedCell => {
                    "Run focused validation and commit only the completed assignment on the current backend-created cell branch. Do not push or switch branches."
                }
                WorkspaceStrategy::None => {
                    "Run focused validation and review the owned changes. Do not mutate git without explicit operator authorization."
                }
            }
        };

        let completion_protocol = if is_research {
            format!(
                r#"## Completion Protocol (MANDATORY)

1. {validation_and_handoff_rule}
2. Update the authoritative task file at {task_file} to `Status: COMPLETED` and add the evidence summary.
3. Send this completed heartbeat exactly as shown:
   ```bash
   {completed_heartbeat}
   ```
4. Send the Queen a concise findings summary with citations, then stop. Do not replace the completed status with an idle or working heartbeat unless the Queen issues a new ACTIVE assignment.
"#,
                validation_and_handoff_rule = validation_and_handoff_rule,
                task_file = task_file,
                completed_heartbeat = completed_heartbeat,
            )
        } else {
            format!(
                r#"## Completion Protocol (MANDATORY)

1. {validation_and_handoff_rule}
2. Complete the Learnings Protocol below before changing the task status.
3. Update the authoritative task file at {task_file} to `Status: COMPLETED` and add the result summary.
4. Send this completed heartbeat exactly as shown:
   ```bash
   {completed_heartbeat}
   ```
5. Send the Queen the commit SHA when applicable plus focused validation evidence, then stop. Do not replace the completed status with an idle or working heartbeat unless the Queen issues a new ACTIVE assignment.
"#,
                validation_and_handoff_rule = validation_and_handoff_rule,
                task_file = task_file,
                completed_heartbeat = completed_heartbeat,
            )
        };

        let learnings_section = if is_research {
            String::new()
        } else {
            format!(
                r#"## Learnings Protocol (MANDATORY)

Before marking the task COMPLETED, POST one durable learning record to /api/sessions/{session_id}/learnings with session, task, outcome, keywords, insight, and files_touched. If the API is unavailable, append the same valid JSON object as one line to .hive-manager/{session_id}/learnings.pending.jsonl in this workspace. Do not write .ai-docs/learnings.jsonl directly. The session API is the topology-neutral durable path.

"#
            )
        };
        let project_context = if is_research {
            String::new()
        } else {
            "## Project Context\n\nRead .ai-docs/project-dna.md before implementation and follow its current conventions.\n\n".to_string()
        };

        format!(
            r#"# Managed Principal {index} - {role_name}

{role_kernel}

{capability_card}

{delegation}

{workspace_contract}

{assignment}

{role_section}

## Runtime

- Session ID: {session_id}
- Principal ID: {session_id}-worker-{index}
- Queen: {queen_id}
- Harness: {cli}
- Model: {model}
- Runtime CWD: {workspace_path}
- Authoritative task file: {task_file}

Use only the native tools exposed by the configured harness. The Capability Card is authoritative for native delegation. Native children inherit this principal's assignment and workspace; they are not managed Hive Workers and must not widen ownership or perform git operations.

{scope_block}

## Task Lifecycle

1. Read {task_file}.
2. If Status is STANDBY, wait and re-check. Do not infer an assignment from this prompt.
3. Begin only when Status is ACTIVE.
4. Stay inside the objective and owned paths. Ask the Queen when ownership or acceptance criteria are unclear.
5. If blocked, set Status to BLOCKED and report the exact blocker.
6. When work is complete, follow the mandatory Completion Protocol below exactly.

{polling_instructions}

{completion_protocol}

## Communication

- Inbox: {worker_conversation}
- Queen channel: {queen_conversation}
- Shared channel: {shared_conversation}
- Read the shared channel before starting a new subtask.
- Send progress, blockers, and completion evidence to POST /api/sessions/{session_id}/conversations/queen/append.
- If the API is unavailable, append the same message to {queen_conversation}.

Heartbeat while active ({heartbeat_cadence} — REQUIRED). Long silent stretches (indexing, builds,
long tool calls) still need it: a run whose last heartbeat is over {stuck_cutoff_secs}s old is
treated as stuck and requeued.
{working_heartbeat}

{learnings_section}{project_context}After reporting completion, stop and continue monitoring the inbox without sending another heartbeat. Do not take a new task until its task file status is ACTIVE; once reactivated, send a working heartbeat."#,
            index = index,
            role_name = role_name,
            role_kernel = role_kernel,
            capability_card = capability_card,
            delegation = delegation,
            workspace_contract = workspace_contract,
            assignment = assignment,
            role_section = role_section,
            session_id = session_id,
            queen_id = queen_id,
            cli = config.cli,
            model = config.model.as_deref().unwrap_or("harness default"),
            workspace_path = workspace_path,
            task_file = task_file,
            scope_block = scope_block,
            polling_instructions = polling_instructions,
            completion_protocol = completion_protocol,
            worker_conversation = worker_conversation,
            queen_conversation = queen_conversation,
            shared_conversation = shared_conversation,
            working_heartbeat = working_heartbeat,
            heartbeat_cadence = heartbeat_cadence_label(),
            stuck_cutoff_secs = STUCK_CUTOFF_SECS,
            learnings_section = learnings_section,
            project_context = project_context,
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
            let role_label = worker_config
                .role
                .as_ref()
                .map(|r| r.label.clone())
                .unwrap_or_else(|| format!("Worker {}", worker_index));
            let cli_name = &worker_config.cli;
            worker_info.push_str(&format!(
                "| {} | {} | {} |\n",
                worker_index, role_label, cli_name
            ));
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
            planner_info.push_str(&format!(
                "| {} | {} | {} workers |\n",
                index, planner_config.domain, worker_count
            ));
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
| Mark Worker Status | `mark-worker-status.md` | Mark each independently verified worker complete |
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
    fn write_prompt_file(
        project_path: &PathBuf,
        session_id: &str,
        filename: &str,
        content: &str,
    ) -> Result<PathBuf, String> {
        let prompts_dir = project_path
            .join(".hive-manager")
            .join(session_id)
            .join("prompts");
        std::fs::create_dir_all(&prompts_dir)
            .map_err(|e| format!("Failed to create prompts directory: {}", e))?;

        let file_path = prompts_dir.join(filename);
        std::fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write prompt file: {}", e))?;

        Ok(file_path)
    }

    /// Write a worker prompt file inside the worker's own worktree.
    fn write_worker_prompt_file(
        worktree_root: &Path,
        worker_index: u8,
        filename: &str,
        content: &str,
    ) -> Result<PathBuf, String> {
        let prompts_dir = worktree_root.join(".hive-manager").join("prompts");
        std::fs::create_dir_all(&prompts_dir).map_err(|e| {
            format!(
                "Failed to create prompts directory for worker {}: {}",
                worker_index, e
            )
        })?;

        let file_path = prompts_dir.join(filename);
        std::fs::write(&file_path, content).map_err(|e| {
            format!(
                "Failed to write prompt file for worker {}: {}",
                worker_index, e
            )
        })?;

        Ok(file_path)
    }

    /// Write a tool documentation file to the session's tools directory
    fn write_tool_file(
        project_path: &PathBuf,
        session_id: &str,
        filename: &str,
        content: &str,
    ) -> Result<PathBuf, String> {
        let tools_dir = project_path
            .join(".hive-manager")
            .join(session_id)
            .join("tools");
        std::fs::create_dir_all(&tools_dir)
            .map_err(|e| format!("Failed to create tools directory: {}", e))?;

        let file_path = tools_dir.join(filename);
        std::fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write tool file: {}", e))?;

        Ok(file_path)
    }

    /// Write all standard tool documentation files for a session
    fn write_tool_files(
        project_path: &PathBuf,
        session_id: &str,
        default_cli: &str,
    ) -> Result<(), String> {
        let worker_task_file_example = "<absolute task path returned by the backend>".to_string();
        let qa_task_file_example =
            format!(".hive-manager/{}/tasks/qa-worker-N-task.md", session_id);
        let worker_one_task_file_example =
            "<absolute task path returned for worker 1>".to_string();

        // Spawn Worker tool
        let spawn_worker_tool = format!(
            r#"# Spawn Worker Tool

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
  "name": "Worker 2 (Frontend)",
  "description": "One-line task summary",
  "initial_task": "Optional task description"
}}
```

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| role_type | string | Yes | Worker role: backend, frontend, coherence, simplify, reviewer, resolver, tester, code-quality, researcher |
| cli | string | No | CLI override: codex, opencode, cursor, droid, qwen, or claude. Omit to inherit the session principal CLI (`{default_cli}`). |
| model | string | No | Model override (for example gpt-5.6-sol for Codex or fable/opus for Claude). Omit to inherit the principal model. |
| flags | string[] | No | CLI flag override. Omit to inherit principal flags; send `[]` to clear them. |
| name | string | No | Stable worker name; defaults to `Worker N (Role)` |
| description | string | No | One-line task summary used for deterministic labels |
| label | string | No | Legacy label field; kept as a fallback input |
| initial_task | string | No | Initial task/prompt for the worker |
| parent_id | string | No | Parent agent ID (defaults to Queen) |

## Example Usage

```bash
# Spawn a backend principal with the session's CLI/model/flags defaults
curl -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
  -H "Content-Type: application/json" \
  -d '{{"role_type": "backend"}}'

# Spawn a frontend worker with an initial task
curl -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
  -H "Content-Type: application/json" \
  -d '{{"role_type": "frontend", "name": "Worker 2 (Frontend)", "description": "Implement the login form UI", "initial_task": "Implement the login form UI"}}'

# Spawn a reviewer worker
curl -X POST "http://localhost:18800/api/sessions/{session_id}/workers" \
  -H "Content-Type: application/json" \
  -d '{{"role_type": "reviewer", "name": "Worker 3 (Reviewer)", "description": "Review the current implementation"}}'
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
- Treat the absolute `task_file` returned by the API as authoritative; do not reconstruct it from the worker ID
- Shared-cell Hive: the task file is under `.hive-manager/tasks/` in the shared primary workspace
- Isolated-cell Hive: the task file is under `.hive-manager/tasks/` in that worker's isolated workspace
- Research/no-worktree Hive: the task file is under `.hive-manager/{session_id}/tasks/` in the operator project
- Workers poll the returned task file for ACTIVE status
- Dynamic principals are supported by Hive/Research sessions. Fusion variants use their pre-created Fusion task files instead of this endpoint
- Use this to spawn workers sequentially as tasks complete
"#,
            session_id = session_id,
            default_cli = default_cli,
            worker_task_file_example = worker_task_file_example
        );

        Self::write_tool_file(
            project_path,
            session_id,
            "spawn-worker.md",
            &spawn_worker_tool,
        )?;

        let spawn_qa_worker_tool = format!(
            r#"# Spawn QA Worker Tool

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
| cli | string | No | CLI to use: {default_cli} (default), codex, opencode, cursor, droid, qwen |
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
"#,
            session_id = session_id,
            default_cli = default_cli,
            qa_task_file_example = qa_task_file_example
        );

        Self::write_tool_file(
            project_path,
            session_id,
            "spawn-qa-worker.md",
            &spawn_qa_worker_tool,
        )?;

        // List Workers tool
        let list_workers_tool = format!(
            r#"# List Workers Tool

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
"#,
            session_id = session_id,
            default_cli = default_cli,
            worker_one_task_file_example = worker_one_task_file_example
        );

        Self::write_tool_file(
            project_path,
            session_id,
            "list-workers.md",
            &list_workers_tool,
        )?;

        let completed_status_example = heartbeat_snippet(
            "http://localhost:18800",
            session_id,
            "<exact-agent-id>",
            "completed",
            "Queen verified completion: replace with concise gate evidence",
        );
        let mark_worker_status_tool = format!(
            r#"# Mark Worker Status Tool

Record an agent heartbeat/status after independently verifying its state. The Queen MUST use this tool after verifying a managed principal, researcher, or Fusion variant is complete because the UI completion checkoff and stall monitor read this status.

## HTTP API

**Endpoint:** `POST http://localhost:18800/api/sessions/{session_id}/heartbeat`

**Headers:**
```text
Content-Type: application/json
```

## Request Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| agent_id | string | Yes | Exact full agent ID from the roster or worker API, such as `{session_id}-worker-2` or `{session_id}-fusion-1` |
| status | string | Yes | `working`, `idle`, or `completed` |
| summary | string | No | Concise evidence-backed status summary |

## Mark a Verified Completion

Replace `<exact-agent-id>` with the verified agent's exact full ID and replace the summary with the gates you checked, then run:

```bash
{completed_status_example}
```

For a Fusion variant or another agent type, keep the request identical and use the exact ID shown in the Queen roster.

## Verification Rule

- Verify the deliverable and required gates before sending `completed`; a task-file claim alone is not sufficient.
- Use the exact full agent ID. A shortened ID such as `worker-N` will not drive that agent's UI status, and the `<exact-agent-id>` placeholder fails validation if left unchanged.
- Send `completed` immediately after verification. A later `working` or `idle` heartbeat replaces it, so do not downgrade a completed agent unless it has received a new ACTIVE assignment.
"#,
            session_id = session_id,
            completed_status_example = completed_status_example,
        );

        Self::write_tool_file(
            project_path,
            session_id,
            "mark-worker-status.md",
            &mark_worker_status_tool,
        )?;

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

        Self::write_tool_file(
            project_path,
            session_id,
            "submit-learning.md",
            submit_learning_tool,
        )?;

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

        Self::write_tool_file(
            project_path,
            session_id,
            "list-learnings.md",
            list_learnings_tool,
        )?;

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

        Self::write_tool_file(
            project_path,
            session_id,
            "delete-learning.md",
            delete_learning_tool,
        )?;

        Ok(())
    }

    /// Write tool documentation files for Swarm mode (includes planner tools)
    fn write_swarm_tool_files(
        project_path: &PathBuf,
        session_id: &str,
        planner_count: u8,
        default_cli: &str,
    ) -> Result<(), String> {
        // First write standard worker tools
        Self::write_tool_files(project_path, session_id, default_cli)?;

        // Spawn Planner tool
        let spawn_planner_tool = format!(
            r#"# Spawn Planner Tool

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
| cli | string | No | CLI to use: {default_cli} (default), codex, opencode, cursor, droid, qwen |
| model | string | No | Raw model identifier passed to the selected CLI's model flag (e.g., `opus`, `fable`, `gpt-5.6-sol`, `gpt-5.6-terra`, `glm-5.1`, `qwen3-coder`) |
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
"#,
            session_id = session_id,
            planner_count = planner_count,
            default_cli = default_cli
        );

        Self::write_tool_file(
            project_path,
            session_id,
            "spawn-planner.md",
            &spawn_planner_tool,
        )?;

        // List Planners tool
        let list_planners_tool = format!(
            r#"# List Planners Tool

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
"#,
            session_id = session_id,
            default_cli = default_cli
        );

        Self::write_tool_file(
            project_path,
            session_id,
            "list-planners.md",
            &list_planners_tool,
        )?;

        Ok(())
    }

    /// Write a task file for a worker (ACTIVE when pre-seeded with a task, otherwise STANDBY)
    fn write_task_file(
        worktree_path: &Path,
        worker_index: u8,
        initial_task: Option<&str>,
        read_only: bool,
    ) -> Result<PathBuf, String> {
        let status = initial_task.map(|_| "ACTIVE");
        Self::write_task_file_with_status(
            worktree_path,
            worker_index,
            initial_task,
            status,
            read_only,
        )
    }

    /// Write a task file with an optional status override (used for sequential spawning).
    /// `read_only` => research worker: read-only scope + role constraints (no
    /// implementation, no project mutation), matching build_worker_prompt.
    fn write_task_file_with_status(
        worktree_path: &Path,
        worker_index: u8,
        initial_task: Option<&str>,
        status: Option<&str>,
        read_only: bool,
    ) -> Result<PathBuf, String> {
        let file_path = Self::task_file_path_for_worker(worktree_path, worker_index as usize);
        Self::write_task_file_at_path(&file_path, worker_index, initial_task, status, read_only)
    }

    fn write_task_file_at_path(
        file_path: &Path,
        worker_index: u8,
        initial_task: Option<&str>,
        status: Option<&str>,
        read_only: bool,
    ) -> Result<PathBuf, String> {
        let tasks_dir = file_path
            .parent()
            .ok_or_else(|| format!("Task file has no parent directory: {}", file_path.display()))?;
        std::fs::create_dir_all(tasks_dir)
            .map_err(|e| format!("Failed to create tasks directory: {}", e))?;

        let scope_block = if read_only {
            Self::scope_block_read_only()
        } else {
            Self::scope_block(".")
        };
        let role_constraints = if read_only {
            "- **RESEARCHER (READ-ONLY)**: Investigate and synthesize; you have NO authority to implement, edit, or create project files.
- **SCOPE**: Stay within your assigned research sub-question.
- **NO MUTATION**: No code changes, no commits, no branches. Report findings to the Queen via the conversation API."
        } else {
            "- **EXECUTOR**: You have full authority to implement and fix issues.
- **SCOPE**: Stay within your assigned domain/specialization.
- **GIT**: Follow the launch prompt's Workspace Contract. Never push, create or switch branches, stash, or reset."
        };
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

{role_constraints}

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
            role_constraints = role_constraints,
            scope_block = scope_block,
            task_content = task_content,
            timestamp = timestamp
        );

        std::fs::write(file_path, content)
            .map_err(|e| format!("Failed to write task file: {}", e))?;

        Ok(file_path.to_path_buf())
    }

    fn write_qa_task_file(
        project_path: &PathBuf,
        session_id: &str,
        worker_index: u8,
        specialization: &str,
        initial_task: Option<&str>,
    ) -> Result<PathBuf, String> {
        let tasks_dir = project_path
            .join(".hive-manager")
            .join(session_id)
            .join("tasks");
        std::fs::create_dir_all(&tasks_dir)
            .map_err(|e| format!("Failed to create tasks directory: {}", e))?;

        let filename = format!("qa-worker-{}-task.md", worker_index);
        let file_path = tasks_dir.join(&filename);

        let (status, task_content) = if let Some(task) = initial_task {
            ("ACTIVE", task.to_string())
        } else {
            (
                "STANDBY",
                "Awaiting QA assignment from the Evaluator. Monitor this file for updates."
                    .to_string(),
            )
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
        execution_policy: HiveExecutionPolicy,
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
        self.emit_workspace_created(&session_id, PRIMARY_CELL_ID, &solo_branch, Some(&solo_cwd));
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
        let (cmd, mut args) = Self::build_solo_command(
            &solo_config,
            if with_evaluator {
                None
            } else {
                task_description.as_deref()
            },
        );
        if with_evaluator {
            let solo_prompt = Self::build_solo_evaluator_prompt(
                &session_id,
                &project_path,
                &solo_cwd,
                task_description.as_deref(),
            );
            let prompt_file = match Self::write_prompt_file(
                &project_path,
                &session_id,
                "solo-prompt.md",
                &solo_prompt,
            ) {
                Ok(path) => path,
                Err(err) => {
                    self.rollback_launch_allocations(
                        &project_path,
                        &session_id,
                        &created_cells,
                        &spawned_agent_ids,
                    );
                    return Err(err);
                }
            };
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_file.to_string_lossy());
        }
        let solo_id = format!("{}-worker-1", session_id);

        {
            let pty_manager = self.pty_manager.read();
            if let Err(e) = pty_manager.create_session(
                solo_id.clone(),
                AgentRole::Worker {
                    index: 1,
                    parent: None,
                },
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
                role: AgentRole::Worker {
                    index: 1,
                    parent: None,
                },
                status: AgentStatus::Running,
                config: solo_config.clone(),
                parent_id: None,
                commit_sha: None,
                base_commit_sha: None,
            }],
            default_cli: cli,
            default_model: model,
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy,
            qa_workers: qa_workers.clone().unwrap_or_default(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: Some(solo_cwd.clone()),
            worktree_branch: Some(solo_branch.clone()),
            no_git: false,
            resume_report: None,
        };

        if let Err(err) = Self::write_tool_files(
            &project_path,
            &session_id,
            Self::session_principal_cli(&session),
        ) {
            self.rollback_launch_allocations(
                &project_path,
                &session_id,
                &created_cells,
                &spawned_agent_ids,
            );
            return Err(err);
        }

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        self.emit_agent_batch_launched(&session, &session.agents);

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: session.clone(),
                },
            );
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
        let mut execution_policy = config.execution_policy.clone();
        execution_policy.launch_kind = HiveLaunchKind::Solo;
        // Solo always owns a dedicated worker worktree. Persist the effective
        // topology so Prince fixer integration cherry-picks into that worktree.
        execution_policy.workspace_strategy = WorkspaceStrategy::IsolatedCell;

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
            execution_policy,
        )
    }

    pub fn launch_hive_v2(&self, config: HiveLaunchConfig) -> Result<Session, String> {
        self.launch_hive_internal(config, None, HashMap::new(), true, true)
    }

    /// Shared Hive launch path. `launch_hive_v2` and `launch_research` both
    /// funnel through here so we keep a single orchestration body.
    ///
    /// Override hooks (used by Research mode):
    /// - `queen_template_override`: when `Some(name)`, the Queen prompt is rendered
    ///   from the named prompt template (e.g. `"queen-research"`) via
    ///   `render_named_prompt` instead of the hand-built `build_queen_master_prompt`.
    /// - `extra_queen_vars`: additional template variables merged into the
    ///   templated Queen prompt (e.g. `global_wiki_path`). Ignored when
    ///   `queen_template_override` is `None`.
    /// - `use_worktrees`: when `true`, Hive uses the operator-selected shared or
    ///   isolated managed-workspace topology. When `false` (Research), no git is
    ///   touched: every agent runs directly in `project_path`, so the launch
    ///   succeeds even on a non-git folder and never creates branches/worktrees.
    fn launch_hive_internal(
        &self,
        config: HiveLaunchConfig,
        queen_template_override: Option<&str>,
        extra_queen_vars: HashMap<String, String>,
        use_worktrees: bool,
        pre_spawn_workers: bool,
    ) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();
        let mut agents = Vec::new();
        let project_path = PathBuf::from(&config.project_path);
        let mut created_cells = Vec::new();
        let mut spawned_agent_ids = Vec::new();

        let topology = SessionOrchestrator::plan_hive_launch(
            &config.execution_policy,
            config.workers.len(),
            !use_worktrees,
        )
        .map_err(|error| error.to_string())?;

        if topology.launch_kind == HiveLaunchKind::Solo
            && (pre_spawn_workers || config.execution_policy.launch_kind == HiveLaunchKind::Solo)
        {
            return self.launch_solo(config);
        }

        // If with_planning is true, spawn Master Planner first
        if config.with_planning {
            return self.launch_planning_phase(session_id, config);
        }

        let shared_cell = use_worktrees && topology.uses_shared_cell();

        // Fetch latest from origin so all worktrees branch from the most
        // recent remote state, avoiding stale-base divergence. Skipped in
        // no-worktree mode (Research), which may run on a non-git folder.
        let base_ref = if use_worktrees {
            resolve_fresh_base(&project_path)
        } else {
            String::new()
        };

        // Create Queen agent
        let queen_id = format!("{}-queen", session_id);
        let (cmd, mut args) = Self::build_command(&config.queen_config);
        let queen_branch = if shared_cell {
            format!("hive/{}/primary", session_id)
        } else {
            format!("hive/{}/queen", session_id)
        };
        let queen_cwd = if use_worktrees {
            let queen_cell_id = if shared_cell { "primary" } else { "queen" };
            let (_, cwd) = create_session_worktree(
                &session_id,
                queen_cell_id,
                &queen_branch,
                &base_ref,
                &project_path,
            )?;
            created_cells.push((queen_cell_id.to_string(), queen_branch.clone()));
            cwd
        } else {
            // No-worktree mode: the Queen runs directly in the project directory.
            project_path.to_string_lossy().to_string()
        };
        if use_worktrees {
            self.emit_workspace_created(
                &session_id,
                PRIMARY_CELL_ID,
                &queen_branch,
                Some(&queen_cwd),
            );
        }

        // Check if plan.md exists (from previous planning phase)
        let plan_path = project_path
            .join(".hive-manager")
            .join(&session_id)
            .join("plan.md");
        let has_plan = plan_path.exists();

        // Write Queen prompt to file and pass to CLI.
        //
        // Research mode renders a research-flavored Queen prompt from a named
        // template; the default Hive path uses the hand-built master prompt.
        let master_prompt = if let Some(template_name) = queen_template_override {
            Self::build_templated_queen_prompt(
                template_name,
                &session_id,
                &config.workers,
                config.prompt.as_deref(),
                extra_queen_vars,
            )
        } else {
            Self::build_queen_master_prompt(
                &config.queen_config,
                &project_path,
                Path::new(&queen_cwd),
                &session_id,
                &config.workers,
                config.prompt.as_deref(),
                has_plan,
                config.with_evaluator,
                &config.execution_policy,
            )
        };
        let prompt_file = match Self::write_prompt_file(
            &project_path,
            &session_id,
            "queen-prompt.md",
            &master_prompt,
        ) {
            Ok(prompt_file) => prompt_file,
            Err(err) => {
                self.rollback_launch_allocations(
                    &project_path,
                    &session_id,
                    &created_cells,
                    &spawned_agent_ids,
                );
                return Err(err);
            }
        };
        let prompt_path = prompt_file.to_string_lossy().to_string();
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

        // Write tool documentation files
        let principal_cli = config
            .workers
            .first()
            .map(|principal| principal.cli.as_str())
            .unwrap_or("codex");
        if let Err(err) = Self::write_tool_files(&project_path, &session_id, principal_cli) {
            self.rollback_launch_allocations(
                &project_path,
                &session_id,
                &created_cells,
                &spawned_agent_ids,
            );
            return Err(err);
        }

        tracing::info!(
            "Launching Queen agent (v2): {} {:?} in {:?}",
            cmd,
            args,
            queen_cwd
        );

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
                self.rollback_launch_allocations(
                    &project_path,
                    &session_id,
                    &created_cells,
                    &spawned_agent_ids,
                );
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

        // Create Worker agents.
        //
        // Roster mode (Research, `pre_spawn_workers == false`): `workers_to_spawn` is
        // empty, so nothing comes up here at launch. The configured workers are a
        // roster rendered into the Queen prompt; the Queen spawns the ones it needs on
        // demand via the spawn-worker tool (`POST /api/sessions/{id}/workers`).
        let workers_to_spawn: &[AgentConfig] = if pre_spawn_workers {
            &config.workers
        } else {
            &[]
        };
        for (i, worker_config) in workers_to_spawn.iter().enumerate() {
            let index = (i + 1) as u8;
            let worker_id = format!("{}-worker-{}", session_id, index);
            let worker_role = worker_config
                .role
                .clone()
                .unwrap_or_else(|| WorkerRole::new("general", "Worker", &worker_config.cli));
            let worker_config =
                Self::apply_worker_identity(index, &worker_role, worker_config.clone());
            let (cmd, mut args) = Self::build_command(&worker_config);
            let worker_branch = if shared_cell {
                queen_branch.clone()
            } else {
                format!("hive/{}/worker-{}", session_id, index)
            };
            let worker_cell_id = format!("worker-{}", index);
            let worker_cwd = if use_worktrees {
                if shared_cell {
                    queen_cwd.clone()
                } else {
                    let (_, cwd) = match create_session_worktree(
                        &session_id,
                        &worker_cell_id,
                        &worker_branch,
                        &base_ref,
                        &project_path,
                    ) {
                        Ok(result) => result,
                        Err(err) => {
                            self.rollback_launch_allocations(
                                &project_path,
                                &session_id,
                                &created_cells,
                                &spawned_agent_ids,
                            );
                            return Err(err);
                        }
                    };
                    created_cells.push((worker_cell_id.clone(), worker_branch.clone()));
                    cwd
                }
            } else {
                // No-worktree mode: workers run directly in the project directory.
                project_path.to_string_lossy().to_string()
            };
            let worker_base_commit_sha = if use_worktrees {
                current_head(Path::new(&worker_cwd)).ok()
            } else {
                None
            };
            if use_worktrees && !shared_cell {
                self.emit_workspace_created(
                    &session_id,
                    PRIMARY_CELL_ID,
                    &worker_branch,
                    Some(&worker_cwd),
                );
            }

            // Write task file for this worker (STANDBY or with initial task).
            // Researcher workers get a read-only task file (no implementation authority).
            let worker_read_only = worker_config
                .role
                .as_ref()
                .map(|r| r.role_type.eq_ignore_ascii_case("researcher"))
                .unwrap_or(false);
            if let Err(err) = Self::write_task_file(
                Path::new(&worker_cwd),
                index,
                worker_config.initial_prompt.as_deref(),
                worker_read_only,
            ) {
                self.rollback_launch_allocations(
                    &project_path,
                    &session_id,
                    &created_cells,
                    &spawned_agent_ids,
                );
                return Err(err);
            }

            // Write worker prompt to file and pass to CLI
            let worker_prompt = Self::build_worker_prompt(
                index,
                &worker_config,
                &queen_id,
                &session_id,
                &project_path,
                Path::new(&worker_cwd),
                &config.execution_policy,
            );
            let filename = format!("worker-{}-prompt.md", index);
            let prompt_file = match Self::write_worker_prompt_file(
                Path::new(&worker_cwd),
                index,
                &filename,
                &worker_prompt,
            ) {
                Ok(prompt_file) => prompt_file,
                Err(err) => {
                    self.rollback_launch_allocations(
                        &project_path,
                        &session_id,
                        &created_cells,
                        &spawned_agent_ids,
                    );
                    return Err(err);
                }
            };
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!(
                "Launching Worker {} agent (v2): {} {:?} in {:?}",
                index,
                cmd,
                args,
                worker_cwd
            );

            {
                let pty_manager = self.pty_manager.read();
                if let Err(e) = pty_manager.create_session(
                    worker_id.clone(),
                    AgentRole::Worker {
                        index,
                        parent: Some(queen_id.clone()),
                    },
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(&worker_cwd),
                    120,
                    30,
                ) {
                    self.rollback_launch_allocations(
                        &project_path,
                        &session_id,
                        &created_cells,
                        &spawned_agent_ids,
                    );
                    return Err(format!("Failed to spawn Worker {}: {}", index, e));
                }
            }
            spawned_agent_ids.push(worker_id.clone());

            agents.push(AgentInfo {
                id: worker_id,
                role: AgentRole::Worker {
                    index,
                    parent: Some(queen_id.clone()),
                },
                status: AgentStatus::Running,
                config: worker_config.clone(),
                parent_id: Some(queen_id.clone()),
                commit_sha: None,
                base_commit_sha: worker_base_commit_sha,
            });
        }

        let (default_principal_cli, default_principal_model, default_principal_flags) =
            Self::configured_principal_defaults(&config.workers);
        let (max_qa_iterations, qa_timeout_secs, auth_strategy) = default_session_qa_settings();
        let session = Session {
            id: session_id.clone(),
            name: config.name.clone(),
            color: config.color.clone(),
            session_type: SessionType::Hive {
                // Roster mode starts with zero live workers; the count grows as the
                // Queen spawns researchers on demand.
                worker_count: if pre_spawn_workers {
                    config.workers.len() as u8
                } else {
                    0
                },
            },
            project_path: project_path.clone(),
            state: SessionState::Running,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli: config.queen_config.cli.clone(),
            default_model: config.queen_config.model.clone(),
            default_principal_cli,
            default_principal_model,
            default_principal_flags,
            execution_policy: config.execution_policy.clone(),
            qa_workers: config.qa_workers.clone().unwrap_or_default(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: use_worktrees.then_some(queen_cwd.clone()),
            worktree_branch: if use_worktrees {
                Some(queen_branch.clone())
            } else {
                None
            },
            no_git: !use_worktrees,
            resume_report: None,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        self.emit_agent_batch_launched(&session, &session.agents);

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: session.clone(),
                },
            );
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
            self.rollback_launch_allocations(
                &project_path,
                &session_id,
                &created_cells,
                &spawned_agent_ids,
            );
            err
        })?;

        Ok(session)
    }

    /// Launch a **Research** session.
    ///
    /// Research mode is a Hive profile (see [`ResearchLaunchConfig`]): it reuses
    /// the shared Hive launch path with research-specific overrides:
    /// - The Queen prompt is rendered from the `queen-research` template, with the
    ///   `global_wiki_path` variable (read from `AppConfig`) injected alongside the
    ///   standard Queen variables. The Queen drives wiki load/capture via prompt.
    /// - Workers without an explicit role are assigned the `researcher` role, which
    ///   resolves worker prompts/heartbeats to the `researcher` role type
    ///   (template key `roles/researcher`).
    /// - Planning and the evaluator are always disabled.
    pub fn launch_research(&self, config: ResearchLaunchConfig) -> Result<Session, String> {
        let smoke_test = config.smoke_test;

        // Assign the "researcher" role to any worker that doesn't already carry one,
        // so role-driven prompt/heartbeat resolution lands on roles/researcher.
        let workers = config
            .workers
            .into_iter()
            .map(|mut worker| {
                if worker.role.is_none() {
                    worker.role = Some(WorkerRole::new("researcher", "Researcher", &worker.cli));
                }
                worker
            })
            .collect();

        let hive_config = HiveLaunchConfig {
            project_path: config.project_path,
            name: config.name,
            color: config.color,
            queen_config: config.queen_config,
            workers,
            prompt: config.prompt,
            with_planning: false,
            with_evaluator: false,
            evaluator_config: None,
            qa_workers: None,
            // Research smoke is driven entirely by the Queen prompt (see `smoke_directive`
            // below); it must NOT trigger the evaluator-based smoke path.
            smoke_test: false,
            execution_policy: HiveExecutionPolicy {
                launch_kind: HiveLaunchKind::Hive,
                workspace_strategy: WorkspaceStrategy::None,
                ..HiveExecutionPolicy::default()
            },
        };

        // Resolve the global wiki path from AppConfig (falls back to the documented
        // default if config is unavailable or the field is unset).
        let global_wiki_path = self
            .storage
            .as_ref()
            .and_then(|storage| storage.load_config().ok())
            .and_then(|cfg| cfg.global_wiki_path)
            .unwrap_or_else(|| "~/.ai-docs/wiki/".to_string());
        // Expand a leading `~` so the path works inside the queen-research
        // template's quoted shell commands (`cd "{{global_wiki_path}}"`).
        let global_wiki_path = expand_tilde(&global_wiki_path);

        // The Queen executes this prompt, so the Queen's CLI decides how the wiki path
        // must be spelled in its shell blocks.
        let extra_queen_vars = Self::research_queen_extra_vars(
            &global_wiki_path,
            &hive_config.queen_config.cli,
            smoke_test,
        );

        // Research never touches git: no worktrees, no branches, and no pre-spawned
        // workers. The Queen comes up alone and spawns researchers from the roster on
        // demand, so it also works on non-repo folders.
        self.launch_hive_internal(
            hive_config,
            Some("queen-research"),
            extra_queen_vars,
            false,
            false,
        )
    }

    /// Assemble the `queen-research`-specific template variables.
    ///
    /// Extracted from [`Self::launch_research`] so the rendered research Queen prompt is
    /// reachable from a test without standing up storage and a PTY — a
    /// template-constant assertion would prove nothing about what the Queen receives.
    ///
    /// `queen_cli` is the CLI that will execute the prompt; the wiki path goes through
    /// the same [`Self::insert_wiki_path_variables`] the debate templates use, so the
    /// two insert sites cannot drift.
    fn research_queen_extra_vars(
        global_wiki_path: &str,
        queen_cli: &str,
        smoke_test: bool,
    ) -> HashMap<String, String> {
        let mut extra_queen_vars = HashMap::new();
        Self::insert_wiki_path_variables(&mut extra_queen_vars, global_wiki_path, queen_cli);
        // `smoke_directive` is rendered near the top of the queen-research prompt. It is
        // empty for a normal run and a hard override for a smoke run (spawn ONE
        // researcher, trivial canned task, no wiki load/capture).
        extra_queen_vars.insert(
            "smoke_directive".to_string(),
            if smoke_test {
                Self::research_smoke_directive()
            } else {
                String::new()
            },
        );
        extra_queen_vars
    }

    /// Hard-override banner injected at the top of the queen-research prompt for a
    /// smoke run. Keeps the smoke flow to the minimal end-to-end plumbing check the
    /// product owner asked for: one researcher, a trivial task, no wiki side effects.
    fn research_smoke_directive() -> String {
        r#"## ⚠️ SMOKE TEST MODE — OVERRIDES EVERYTHING BELOW

This is a **minimal plumbing smoke test**, not real research. Ignore the normal
phases and do EXACTLY this, then stop:

1. **Skip Phase 1 (wiki load) and Phase 4 (wiki capture).** Do not read or write the
   global wiki. No git, no PR.
2. **Spawn exactly ONE researcher** from the roster (slot #1) using the spawn-worker
   tool, with this trivial `initial_task`:
   > "Smoke test: reply in the conversation with the literal text `RESEARCH SMOKE OK`,
   > your current working directory, and today's date. Do not investigate anything else."
3. **Wait** for that researcher to report back in the conversation.
4. **Report the result:** post `RESEARCH SMOKE PASS` to the conversation if the
   researcher replied with `RESEARCH SMOKE OK`, otherwise post `RESEARCH SMOKE FAIL`
   followed by what went wrong. Then stop — do not spawn any further researchers.

---
"#
        .to_string()
    }

    pub fn launch_fusion(&self, config: FusionLaunchConfig) -> Result<Session, String> {
        tracing::info!(
            "launch_fusion called: with_planning={}, variants={}, task={}",
            config.with_planning,
            config.variants.len(),
            &config.task_description
        );

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
            let task_file =
                Self::fusion_variant_task_file_path(Path::new(&worktree_path), index as usize)
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
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy: HiveExecutionPolicy::default(),
            qa_workers: Vec::new(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: variants.first().map(|v| v.worktree_path.clone()),
            worktree_branch: variants.first().map(|v| v.branch.clone()),
            no_git: false,
            resume_report: None,
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
                model: source_variant
                    .model
                    .clone()
                    .or(config.default_model.clone()),
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
            let prompt_file = Self::write_worker_prompt_file(
                Path::new(&variant.worktree_path),
                variant.index,
                &prompt_filename,
                &worker_prompt,
            )?;
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
                    .map_err(|e| {
                        format!("Failed to spawn Fusion variant {}: {}", variant.name, e)
                    })?;
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

            let waiting_changes =
                {
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

    pub fn launch_debate(&self, mut config: DebateLaunchConfig) -> Result<Session, String> {
        tracing::info!(
            "launch_debate called: with_planning={}, debaters={}, rounds={}, topic={}",
            config.with_planning,
            config.debaters.len(),
            config.rounds,
            &config.topic
        );

        if config.debaters.is_empty() {
            return Err("Debate launch requires at least one debater".to_string());
        }
        config.rounds = Self::validate_debate_rounds(config.rounds)?;
        if config.topic.trim().is_empty() {
            return Err("Debate launch requires a non-empty topic".to_string());
        }

        if config.with_planning {
            let session_id = Uuid::new_v4().to_string();
            return self.launch_debate_planning_phase(session_id, config);
        }

        let session_id = Uuid::new_v4().to_string();
        let project_path = PathBuf::from(&config.project_path);
        let default_cli = if config.default_cli.trim().is_empty() {
            "claude".to_string()
        } else {
            config.default_cli.trim().to_string()
        };
        let debaters =
            Self::build_debate_debater_metadata(&session_id, &project_path, &config, &default_cli);

        let (max_qa_iterations, qa_timeout_secs, auth_strategy) = default_session_qa_settings();
        let session = Session {
            id: session_id.clone(),
            name: config.name.clone(),
            color: config.color.clone(),
            session_type: SessionType::Debate {
                variants: debaters.iter().map(|d| d.name.clone()).collect(),
            },
            project_path: project_path.clone(),
            state: SessionState::Starting,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents: Vec::new(),
            default_cli: default_cli.clone(),
            default_model: config.default_model.clone(),
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy: HiveExecutionPolicy::default(),
            qa_workers: Vec::new(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: debaters.first().map(|d| d.worktree_path.clone()),
            worktree_branch: debaters.first().map(|d| d.branch.clone()),
            no_git: false,
            resume_report: None,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session);
        }
        self.emit_session_update(&session_id);

        let fresh_base = resolve_fresh_base(&project_path);
        let base_branch = format!("debate/{}/base", session_id);
        Self::run_git_in_dir(&project_path, &["branch", &base_branch, &fresh_base])?;
        Self::create_debate_worktrees(&project_path, &session_id, &base_branch, &debaters, self)?;

        let verdict_file = project_path
            .join(".hive-manager")
            .join(&session_id)
            .join("evaluation")
            .join("verdict.md")
            .to_string_lossy()
            .to_string();
        if let Some(parent) = Path::new(&verdict_file).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create debate evaluation directory: {}", e))?;
        }
        std::fs::create_dir_all(
            project_path
                .join(".hive-manager")
                .join(&session_id)
                .join("debate")
                .join("rounds"),
        )
        .map_err(|e| format!("Failed to create debate rounds directory: {}", e))?;

        let metadata = DebateSessionMetadata {
            base_branch,
            debaters,
            judge_config: config.judge_config,
            topic: config.topic,
            rounds: config.rounds,
            verdict_file,
        };
        Self::write_debate_metadata(&project_path, &session_id, &metadata)?;

        self.spawn_debate_round(&session_id, 1)?;

        let session = self
            .get_session(&session_id)
            .ok_or_else(|| "Failed to read debate session after launch".to_string())?;
        self.init_session_storage(&session);
        self.update_session_storage(&session_id);
        self.ensure_task_watcher(&session_id, &project_path);

        Ok(session)
    }

    fn build_debate_debater_metadata(
        session_id: &str,
        project_path: &Path,
        config: &DebateLaunchConfig,
        default_cli: &str,
    ) -> Vec<DebateDebaterMetadata> {
        let mut seen_slugs: HashMap<String, u16> = HashMap::new();

        config
            .debaters
            .iter()
            .enumerate()
            .map(|(idx, debater)| {
                let index = (idx + 1) as u8;
                let name = if debater.name.trim().is_empty() {
                    format!("debater-{}", index)
                } else {
                    debater.name.trim().to_string()
                };
                let slug = Self::unique_variant_slug(&name, &mut seen_slugs);
                let branch = format!("debate/{}/{}", session_id, slug);
                let worktree_path = project_path
                    .join(".hive-debate")
                    .join(session_id)
                    .join(format!("debater-{}", slug))
                    .to_string_lossy()
                    .to_string();
                let cli = if debater.cli.trim().is_empty() {
                    default_cli.to_string()
                } else {
                    debater.cli.trim().to_string()
                };
                let agent_config = AgentConfig {
                    cli,
                    model: debater.model.clone().or(config.default_model.clone()),
                    flags: debater.flags.clone(),
                    label: Some(format!("Debate {}", name)),
                    name: None,
                    description: debater.stance.clone(),
                    role: None,
                    initial_prompt: Some(config.topic.clone()),
                };

                DebateDebaterMetadata {
                    index,
                    name,
                    stance: debater.stance.clone(),
                    slug,
                    branch,
                    worktree_path,
                    config: agent_config,
                }
            })
            .collect()
    }

    fn create_debate_worktrees(
        project_path: &PathBuf,
        session_id: &str,
        base_branch: &str,
        debaters: &[DebateDebaterMetadata],
        controller: &SessionController,
    ) -> Result<(), String> {
        for debater in debaters {
            let worktree_path = PathBuf::from(&debater.worktree_path);
            if let Some(parent) = worktree_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create debate worktree parent dir: {}", e))?;
            }

            Self::run_git_in_dir(
                project_path,
                &[
                    "worktree",
                    "add",
                    &debater.worktree_path,
                    "-b",
                    &debater.branch,
                    base_branch,
                ],
            )?;
            controller.emit_workspace_created(
                session_id,
                &variant_to_cell_id(&debater.name),
                &debater.branch,
                Some(&debater.worktree_path),
            );
        }

        Ok(())
    }

    fn debate_opponent_files(
        project_path: &Path,
        session_id: &str,
        metadata: &DebateSessionMetadata,
        debater_index: u8,
        round: u8,
    ) -> String {
        if round <= 1 {
            return "No prior opponent arguments. This is the opening round.".to_string();
        }

        metadata
            .debaters
            .iter()
            .filter(|debater| debater.index != debater_index)
            .map(|debater| {
                let path = Self::debate_round_argument_file_path(
                    project_path,
                    session_id,
                    round - 1,
                    &debater.slug,
                );
                format!("- {}: `{}`", debater.name, Self::prompt_path(&path))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn spawn_debate_round(&self, session_id: &str, round: u8) -> Result<(), String> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        if !matches!(session.session_type, SessionType::Debate { .. }) {
            return Err(format!("Session {} is not a Debate session", session_id));
        }

        let metadata = Self::read_debate_metadata(&session.project_path, session_id)?;
        if round == 0 || round > metadata.rounds {
            return Err(format!(
                "Invalid debate round {} for session {}",
                round, session_id
            ));
        }

        let previous_round_dir = if round > 1 {
            Some(
                session
                    .project_path
                    .join(".hive-manager")
                    .join(session_id)
                    .join("debate")
                    .join("rounds")
                    .join(format!("round-{}", round - 1)),
            )
        } else {
            None
        };

        let global_wiki_path = self
            .storage
            .as_ref()
            .and_then(|storage| storage.load_config().ok())
            .and_then(|cfg| cfg.global_wiki_path)
            .unwrap_or_default();
        let global_wiki_path = expand_tilde(&global_wiki_path);

        let mut new_agents = Vec::new();
        for debater in &metadata.debaters {
            let spawning_changes = {
                let mut sessions = self.sessions.write();
                sessions.get_mut(session_id).map(|s| {
                    self.set_session_state_with_events(s, SessionState::SpawningDebateRound(round))
                })
            };
            if let Some(changes) = spawning_changes {
                self.emit_cell_status_changes(session_id, changes);
            }
            self.emit_session_update(session_id);

            let worktree_path = PathBuf::from(&debater.worktree_path);
            let argument_file = Self::debate_round_argument_file_path(
                &session.project_path,
                session_id,
                round,
                &debater.slug,
            );
            if let Some(parent) = argument_file.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create debate argument directory: {}", e))?;
            }
            let opponent_files = Self::debate_opponent_files(
                &session.project_path,
                session_id,
                &metadata,
                debater.index,
                round,
            );
            let task_file = Self::write_debate_round_task_file(
                &worktree_path,
                debater,
                &metadata.topic,
                round,
                metadata.rounds,
                &argument_file,
                &opponent_files,
            )?;
            let prompt = Self::build_debate_debater_prompt(
                session_id,
                debater,
                &metadata.topic,
                round,
                metadata.rounds,
                &argument_file,
                previous_round_dir.as_deref(),
                &opponent_files,
                &task_file,
                &global_wiki_path,
            );
            let prompt_filename =
                format!("debate-debater-{}-round-{}-prompt.md", debater.index, round);
            let prompt_file = Self::write_worker_prompt_file(
                &worktree_path,
                debater.index,
                &prompt_filename,
                &prompt,
            )?;
            let prompt_path = prompt_file.to_string_lossy().to_string();

            let agent_config = debater.config.clone();
            let (cmd, mut args) = Self::build_command(&agent_config);
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            let agent_id = Self::debate_round_agent_id(session_id, debater.index, round);
            {
                let pty_manager = self.pty_manager.read();
                pty_manager
                    .create_session(
                        agent_id.clone(),
                        AgentRole::Fusion {
                            variant: debater.name.clone(),
                        },
                        &cmd,
                        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                        Some(&debater.worktree_path),
                        120,
                        30,
                    )
                    .map_err(|e| {
                        format!(
                            "Failed to spawn Debate debater {} round {}: {}",
                            debater.name, round, e
                        )
                    })?;
            }

            new_agents.push(AgentInfo {
                id: agent_id,
                role: AgentRole::Fusion {
                    variant: debater.name.clone(),
                },
                status: AgentStatus::Running,
                config: agent_config,
                parent_id: None,
                commit_sha: None,
                base_commit_sha: None,
            });
        }

        let (updated_session, changes) = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.agents.extend(new_agents.clone());
                self.emit_agent_batch_launched(s, &new_agents);
                let changes = self
                    .set_session_state_with_events(s, SessionState::WaitingForDebateRound(round));
                (s.clone(), changes)
            } else {
                return Err("Session disappeared".to_string());
            }
        };

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: updated_session,
                },
            );
        }
        self.update_session_storage(session_id);
        self.emit_cell_status_changes(session_id, changes);

        Ok(())
    }

    /// Launch the planning phase - spawns Master Planner only
    fn launch_planning_phase(
        &self,
        session_id: String,
        config: HiveLaunchConfig,
    ) -> Result<Session, String> {
        let project_path = PathBuf::from(&config.project_path);
        let mut agents = Vec::new();
        let topology = SessionOrchestrator::plan_hive_launch(
            &config.execution_policy,
            config.workers.len(),
            false,
        )
        .map_err(|error| error.to_string())?;
        let mut created_cells = Vec::new();
        let (workspace_cell, branch) = if topology.uses_shared_cell() {
            ("primary", format!("hive/{}/primary", session_id))
        } else {
            ("queen", format!("hive/{}/queen", session_id))
        };
        let base_ref = resolve_fresh_base(&project_path);
        let (_, cwd) = create_session_worktree(
            &session_id,
            workspace_cell,
            &branch,
            &base_ref,
            &project_path,
        )?;
        created_cells.push((workspace_cell.to_string(), branch.clone()));
        self.emit_workspace_created(&session_id, PRIMARY_CELL_ID, &branch, Some(&cwd));
        let worktree_path = Some(cwd.clone());
        let worktree_branch = Some(branch);

        // Build the appropriate prompt based on mode
        let planner_prompt = if config.smoke_test {
            tracing::info!("Running in SMOKE TEST mode - skipping real investigation");
            Self::build_smoke_test_prompt(
                &session_id,
                &config.workers,
                config.with_evaluator,
                config.qa_workers.as_deref(),
            )
        } else {
            let prompt = config.prompt.as_deref().unwrap_or("");
            Self::build_master_planner_prompt(
                &session_id,
                prompt,
                &config.queen_config,
                &config.workers,
                &config.execution_policy,
                &project_path,
                Path::new(&cwd),
            )
        };

        // Persist continuation input before spawning the planner. A failure here
        // must not leave a live PTY or an orphaned planning worktree.
        let pending_config_path = project_path
            .join(".hive-manager")
            .join(&session_id)
            .join("pending-config.json");
        let pending_result = (|| -> Result<(), String> {
            std::fs::create_dir_all(pending_config_path.parent().unwrap())
                .map_err(|e| format!("Failed to create session directory: {}", e))?;
            let config_json = serde_json::to_string_pretty(&config)
                .map_err(|e| format!("Failed to serialize config: {}", e))?;
            std::fs::write(&pending_config_path, config_json)
                .map_err(|e| format!("Failed to write pending config: {}", e))
        })();
        if let Err(error) = pending_result {
            self.rollback_launch_allocations(&project_path, &session_id, &created_cells, &[]);
            return Err(error);
        }

        {
            let pty_manager = self.pty_manager.read();

            // Create Master Planner agent
            let planner_id = format!("{}-master-planner", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config); // Use queen config for planner

            // Write Master Planner prompt to file
            let prompt_file = match Self::write_prompt_file(
                &project_path,
                &session_id,
                "master-planner-prompt.md",
                &planner_prompt,
            ) {
                Ok(prompt_file) => prompt_file,
                Err(error) => {
                    let _ = std::fs::remove_file(&pending_config_path);
                    self.rollback_launch_allocations(
                        &project_path,
                        &session_id,
                        &created_cells,
                        &[],
                    );
                    return Err(error);
                }
            };
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!("Launching Master Planner: {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    planner_id.clone(),
                    AgentRole::MasterPlanner,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(&cwd),
                    120,
                    30,
                )
                .map_err(|e| {
                    let _ = std::fs::remove_file(&pending_config_path);
                    self.rollback_launch_allocations(
                        &project_path,
                        &session_id,
                        &created_cells,
                        &[],
                    );
                    format!("Failed to spawn Master Planner: {}", e)
                })?;

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

        let (default_principal_cli, default_principal_model, default_principal_flags) =
            Self::configured_principal_defaults(&config.workers);
        let (max_qa_iterations, qa_timeout_secs, auth_strategy) = default_session_qa_settings();
        let session = Session {
            id: session_id.clone(),
            name: config.name.clone(),
            color: config.color.clone(),
            session_type: SessionType::Hive {
                worker_count: config.workers.len() as u8,
            },
            project_path,
            state: SessionState::Planning,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli: config.queen_config.cli.clone(),
            default_model: config.queen_config.model.clone(),
            default_principal_cli,
            default_principal_model,
            default_principal_flags,
            execution_policy: config.execution_policy.clone(),
            qa_workers: config.qa_workers.clone().unwrap_or_default(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path,
            worktree_branch,
            no_git: false,
            resume_report: None,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        self.emit_agent_batch_launched(&session, &session.agents);

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: session.clone(),
                },
            );
        }

        self.init_session_storage(&session);
        self.ensure_task_watcher(&session.id, &session.project_path);

        Ok(session)
    }

    /// Launch the planning phase for Fusion - spawns Master Planner only
    fn launch_fusion_planning_phase(
        &self,
        session_id: String,
        config: FusionLaunchConfig,
    ) -> Result<Session, String> {
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

            let prompt_file = Self::write_prompt_file(
                &project_path,
                &session_id,
                "master-planner-prompt.md",
                &planner_prompt,
            )?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!(
                "Launching Master Planner (fusion): {} {:?} in {:?}",
                cmd,
                args,
                cwd
            );

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
        let pending_config_path = project_path
            .join(".hive-manager")
            .join(&session_id)
            .join("pending-fusion-config.json");
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
            session_type: SessionType::Fusion {
                variants: variant_names,
            },
            project_path: project_path.clone(),
            state: SessionState::Planning,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli: if config.default_cli.trim().is_empty() {
                "claude".to_string()
            } else {
                config.default_cli.trim().to_string()
            },
            default_model: config.default_model.clone(),
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy: HiveExecutionPolicy::default(),
            qa_workers: Vec::new(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: None,
            worktree_branch: None,
            no_git: false,
            resume_report: None,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        self.emit_agent_batch_launched(&session, &session.agents);

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: session.clone(),
                },
            );
        }

        self.init_session_storage(&session);
        self.ensure_task_watcher(&session.id, &session.project_path);

        Ok(session)
    }

    fn launch_debate_planning_phase(
        &self,
        session_id: String,
        config: DebateLaunchConfig,
    ) -> Result<Session, String> {
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();
        let mut agents = Vec::new();

        let planner_prompt = Self::build_debate_master_planner_prompt(
            &session_id,
            &config.topic,
            &config.debaters,
            config.rounds,
        );

        {
            let pty_manager = self.pty_manager.read();

            let planner_id = format!("{}-master-planner", session_id);
            let queen_cfg = config.queen_config.as_ref().unwrap_or(&config.judge_config);
            let (cmd, mut args) = Self::build_command(queen_cfg);

            let prompt_file = Self::write_prompt_file(
                &project_path,
                &session_id,
                "master-planner-prompt.md",
                &planner_prompt,
            )?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!(
                "Launching Master Planner (debate): {} {:?} in {:?}",
                cmd,
                args,
                cwd
            );

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

        let pending_config_path = project_path
            .join(".hive-manager")
            .join(&session_id)
            .join("pending-debate-config.json");
        std::fs::create_dir_all(pending_config_path.parent().unwrap())
            .map_err(|e| format!("Failed to create session directory: {}", e))?;
        let config_json = serde_json::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        std::fs::write(&pending_config_path, config_json)
            .map_err(|e| format!("Failed to write pending config: {}", e))?;

        let debater_names: Vec<String> = config.debaters.iter().map(|v| v.name.clone()).collect();
        let (max_qa_iterations, qa_timeout_secs, auth_strategy) = default_session_qa_settings();
        let session = Session {
            id: session_id.clone(),
            name: config.name.clone(),
            color: config.color.clone(),
            session_type: SessionType::Debate {
                variants: debater_names,
            },
            project_path: project_path.clone(),
            state: SessionState::Planning,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli: if config.default_cli.trim().is_empty() {
                "claude".to_string()
            } else {
                config.default_cli.trim().to_string()
            },
            default_model: config.default_model.clone(),
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy: HiveExecutionPolicy::default(),
            qa_workers: Vec::new(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: None,
            worktree_branch: None,
            no_git: false,
            resume_report: None,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        self.emit_agent_batch_launched(&session, &session.agents);

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: session.clone(),
                },
            );
        }

        self.init_session_storage(&session);
        self.ensure_task_watcher(&session.id, &session.project_path);

        Ok(session)
    }

    /// Continue a Fusion session after planning phase - spawns Queen + Variants
    fn continue_fusion_after_planning(
        &self,
        session_id: &str,
        session: &Session,
    ) -> Result<Session, String> {
        let cwd = session.project_path.to_str().unwrap_or(".");

        // Load the pending Fusion config
        let pending_config_path = session
            .project_path
            .join(".hive-manager")
            .join(session_id)
            .join("pending-fusion-config.json");
        let config_json = std::fs::read_to_string(&pending_config_path)
            .map_err(|e| format!("Failed to read pending fusion config: {}", e))?;
        let config: FusionLaunchConfig = serde_json::from_str(&config_json)
            .map_err(|e| format!("Failed to parse pending fusion config: {}", e))?;

        // Clean up Master Planner PTY before spawning Queen
        let planner_id = format!("{}-master-planner", session_id);
        if let Err(e) = self.stop_agent(session_id, &planner_id) {
            tracing::warn!("Failed to stop Master Planner {}: {}", planner_id, e);
        } else {
            tracing::info!(
                "Stopped Master Planner {} before spawning Fusion Queen",
                planner_id
            );
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
            let worktree_path = session
                .project_path
                .join(".hive-fusion")
                .join(session_id)
                .join(format!("variant-{}", slug))
                .to_string_lossy()
                .to_string();
            let task_file =
                Self::fusion_variant_task_file_path(Path::new(&worktree_path), index as usize)
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
        Self::run_git_in_dir(
            &session.project_path,
            &["branch", &base_branch, &fresh_base],
        )?;

        let mut new_agents = Vec::new();

        // Spawn Queen agent
        let queen_cfg = config
            .queen_config
            .as_ref()
            .unwrap_or(&config.judge_config)
            .clone();
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
            let prompt_file = Self::write_prompt_file(
                &session.project_path,
                session_id,
                "fusion-queen-prompt.md",
                &queen_prompt,
            )?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            // Write tool docs for Queen
            Self::write_tool_files(
                &session.project_path,
                session_id,
                Self::session_principal_cli(&session),
            )?;

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
                    "worktree",
                    "add",
                    &variant.worktree_path,
                    "-b",
                    &variant.branch,
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
                model: source_variant
                    .model
                    .clone()
                    .or(config.default_model.clone()),
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
            let prompt_file = Self::write_worker_prompt_file(
                Path::new(&variant.worktree_path),
                variant.index,
                &prompt_filename,
                &worker_prompt,
            )?;
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
                    .map_err(|e| {
                        format!("Failed to spawn Fusion variant {}: {}", variant.name, e)
                    })?;
            }

            new_agents.push(AgentInfo {
                id: variant.agent_id.clone(),
                role: AgentRole::Fusion {
                    variant: variant.name.clone(),
                },
                status: AgentStatus::Running,
                config: variant_agent_config,
                parent_id: None,
                commit_sha: None,
                base_commit_sha: None,
            });
        }

        // Create evaluation directory
        let evaluation_dir = session
            .project_path
            .join(".hive-manager")
            .join(session_id)
            .join("evaluation");
        std::fs::create_dir_all(&evaluation_dir)
            .map_err(|e| format!("Failed to create fusion evaluation directory: {}", e))?;

        let decision_file = session
            .project_path
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
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: updated_session.clone(),
                },
            );
        }

        self.update_session_storage(session_id);
        self.emit_cell_status_changes(session_id, changes);
        self.ensure_task_watcher(session_id, &updated_session.project_path);

        // Clean up pending config
        let _ = std::fs::remove_file(&pending_config_path);

        Ok(updated_session)
    }

    fn continue_debate_after_planning(
        &self,
        session_id: &str,
        session: &Session,
    ) -> Result<Session, String> {
        let pending_config_path = session
            .project_path
            .join(".hive-manager")
            .join(session_id)
            .join("pending-debate-config.json");
        let config_json = std::fs::read_to_string(&pending_config_path)
            .map_err(|e| format!("Failed to read pending debate config: {}", e))?;
        let mut config: DebateLaunchConfig = serde_json::from_str(&config_json)
            .map_err(|e| format!("Failed to parse pending debate config: {}", e))?;
        config.rounds = Self::validate_debate_rounds(config.rounds)?;

        let planner_id = format!("{}-master-planner", session_id);
        if let Err(e) = self.stop_agent(session_id, &planner_id) {
            tracing::warn!("Failed to stop Master Planner {}: {}", planner_id, e);
        } else {
            tracing::info!(
                "Stopped Master Planner {} before spawning Debate debaters",
                planner_id
            );
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
        let debaters = Self::build_debate_debater_metadata(
            session_id,
            &session.project_path,
            &config,
            &default_cli,
        );

        let fresh_base = resolve_fresh_base(&session.project_path);
        let base_branch = format!("debate/{}/base", session_id);
        Self::run_git_in_dir(
            &session.project_path,
            &["branch", &base_branch, &fresh_base],
        )?;
        Self::create_debate_worktrees(
            &session.project_path,
            session_id,
            &base_branch,
            &debaters,
            self,
        )?;

        let verdict_file = session
            .project_path
            .join(".hive-manager")
            .join(session_id)
            .join("evaluation")
            .join("verdict.md")
            .to_string_lossy()
            .to_string();
        if let Some(parent) = Path::new(&verdict_file).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create debate evaluation directory: {}", e))?;
        }
        std::fs::create_dir_all(
            session
                .project_path
                .join(".hive-manager")
                .join(session_id)
                .join("debate")
                .join("rounds"),
        )
        .map_err(|e| format!("Failed to create debate rounds directory: {}", e))?;

        let metadata = DebateSessionMetadata {
            base_branch,
            debaters: debaters.clone(),
            judge_config: config.judge_config.clone(),
            topic: config.topic,
            rounds: config.rounds,
            verdict_file,
        };
        Self::write_debate_metadata(&session.project_path, session_id, &metadata)?;

        {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                if let Some(d) = debaters.first() {
                    s.worktree_path = Some(d.worktree_path.clone());
                    s.worktree_branch = Some(d.branch.clone());
                }
            }
        }

        self.spawn_debate_round(session_id, 1)?;

        let updated_session = self
            .get_session(session_id)
            .ok_or_else(|| "Session disappeared".to_string())?;
        self.ensure_task_watcher(session_id, &updated_session.project_path);
        let _ = std::fs::remove_file(&pending_config_path);

        Ok(updated_session)
    }

    /// Launch the planning phase for Swarm - spawns Master Planner only
    fn launch_swarm_planning_phase(
        &self,
        session_id: String,
        config: SwarmLaunchConfig,
    ) -> Result<Session, String> {
        let default_cli = config.default_cli.trim().to_string();
        let default_model = config.default_model.clone();
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();
        let mut agents = Vec::new();

        // Build the appropriate prompt based on mode
        let planner_count = if config.planners.is_empty() {
            config.planner_count
        } else {
            config.planners.len() as u8
        };
        let planner_prompt = if config.smoke_test {
            tracing::info!(
                "Running in SMOKE TEST mode (swarm) - {} planners, {} workers each",
                planner_count,
                config.workers_per_planner.len()
            );
            Self::build_swarm_smoke_test_prompt(
                &session_id,
                planner_count,
                &config.workers_per_planner,
                config.with_evaluator,
                config.qa_workers.as_deref(),
            )
        } else {
            // Pass planners and workers info to Master Planner so it knows the full scope
            let prompt = config.prompt.as_deref().unwrap_or("");
            Self::build_swarm_master_planner_prompt(
                &session_id,
                prompt,
                planner_count,
                &config.workers_per_planner,
            )
        };

        {
            let pty_manager = self.pty_manager.read();

            // Create Master Planner agent
            let planner_id = format!("{}-master-planner", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config); // Use queen config for planner

            // Write Master Planner prompt to file
            let prompt_file = Self::write_prompt_file(
                &project_path,
                &session_id,
                "master-planner-prompt.md",
                &planner_prompt,
            )?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!(
                "Launching Master Planner (swarm): {} {:?} in {:?}",
                cmd,
                args,
                cwd
            );

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
        let pending_config_path = project_path
            .join(".hive-manager")
            .join(&session_id)
            .join("pending-swarm-config.json");
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
            session_type: SessionType::Swarm {
                planner_count: if config.planners.is_empty() {
                    config.planner_count
                } else {
                    config.planners.len() as u8
                },
            },
            project_path,
            state: SessionState::Planning,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli,
            default_model,
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy: HiveExecutionPolicy::default(),
            qa_workers: config.qa_workers.clone().unwrap_or_default(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: None,
            worktree_branch: None,
            no_git: false,
            resume_report: None,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: session.clone(),
                },
            );
        }

        self.init_session_storage(&session);
        self.ensure_task_watcher(&session.id, &session.project_path);

        Ok(session)
    }

    /// Spawn the next worker sequentially
    async fn spawn_next_worker(
        &self,
        session_id: &str,
        worker_index: usize,
        config: &HiveLaunchConfig,
        queen_id: &str,
    ) -> Result<(), SessionError> {
        let session = self
            .get_session(session_id)
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

        // #125 resume guard: if this worker-spawn write-step is already journaled
        // Completed, a prior run already created its worktree/branch — do NOT re-run the
        // destructive spawn. Advance to the next worker instead.
        if self.is_write_step_completed(
            session_id,
            crate::domain::run_journal::StepKind::WorkerSpawn,
            index as u64,
        ) {
            tracing::info!(
                session_id,
                worker_index = index,
                "Skipping worker spawn — already journaled Completed (resume)"
            );
            return Ok(());
        }
        // #125: record the write-step as Started BEFORE any destructive worktree op.
        let journal_step_id = self.journal_step_started(
            session_id,
            crate::domain::run_journal::StepKind::WorkerSpawn,
            index as u64,
            Some(&worker_branch),
        );

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
                Some(self.set_session_state_with_events(s, SessionState::SpawningWorker(index)))
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
        let task_file_path =
            Self::task_file_path_for_worker(Path::new(&worker_cwd), index as usize);
        let worker_base_commit_sha = current_head(Path::new(&worker_cwd)).map_err(|err| {
            Self::rollback_worker_launch_artifacts(
                &session.project_path,
                session_id,
                &worker_cell_name,
                &task_file_path,
                None,
                true,
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

        // 2. Write task file (Status: ACTIVE since it's their turn)
        Self::write_task_file_with_status(
            Path::new(&worker_cwd),
            index,
            worker_config.initial_prompt.as_deref(),
            Some("ACTIVE"),
            worker_config
                .role
                .as_ref()
                .map(|r| r.role_type.eq_ignore_ascii_case("researcher"))
                .unwrap_or(false),
        )
        .map_err(|err| {
            Self::rollback_worker_launch_artifacts(
                &session.project_path,
                session_id,
                &worker_cell_name,
                &task_file_path,
                None,
                true,
            );
            self.restore_session_state_after_worker_spawn_failure(session_id, &previous_state);
            SessionError::ConfigError(err)
        })?;

        // 3. Write worker prompt to file
        let worker_prompt = Self::build_worker_prompt(
            index,
            worker_config,
            queen_id,
            session_id,
            &session.project_path,
            Path::new(&worker_cwd),
            &session.execution_policy,
        );
        let prompt_file = Self::write_worker_prompt_file(
            Path::new(&worker_cwd),
            index,
            &filename,
            &worker_prompt,
        )
        .map_err(|err| {
            Self::rollback_worker_launch_artifacts(
                &session.project_path,
                session_id,
                &worker_cell_name,
                &task_file_path,
                None,
                true,
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
                AgentRole::Worker {
                    index,
                    parent: Some(queen_id.to_string()),
                },
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
                    Some(&prompt_file),
                    true,
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
                    role: AgentRole::Worker {
                        index,
                        parent: Some(queen_id.to_string()),
                    },
                    status: AgentStatus::Running,
                    config: worker_config.clone(),
                    parent_id: Some(queen_id.to_string()),
                    commit_sha: None,
                    base_commit_sha: Some(worker_base_commit_sha.clone()),
                };
                s.agents.push(agent.clone());
                self.emit_agent_launched(s, &agent);
                Some(self.set_session_state_with_events(s, SessionState::WaitingForWorker(index)))
            } else {
                None
            }
        };
        if let Some(changes) = waiting_changes {
            self.persist_then_emit_session_update(session_id, changes)
                .map_err(SessionError::ConfigError)?;
        }

        // #125: the worker-spawn write-step landed — mark it Completed so a resume skips it.
        if let Some(step_id) = journal_step_id.as_deref() {
            self.journal_step_finished(
                session_id,
                step_id,
                crate::domain::run_journal::StepStatus::Completed,
            );
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
        // A shared-cell HEAD belongs to the cell and may move because of the Queen
        // or another principal. Never attribute it to an individual worker.
        if matches!(&session.session_type, SessionType::Hive { .. })
            && session.execution_policy.workspace_strategy == WorkspaceStrategy::SharedCell
        {
            return None;
        }

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
            return if head == base_commit_sha {
                None
            } else {
                Some(head)
            };
        }

        let base_ref =
            Self::resolve_worker_base_ref(session, "worker_completion_commit_sha", worker_id);
        if head == base_ref {
            None
        } else {
            Some(head)
        }
    }

    pub(crate) fn sync_agent_commit_sha(
        &self,
        session_id: &str,
        agent_id: &str,
        commit_sha: Option<String>,
    ) {
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
        route_to_prince: bool,
    ) -> (SessionState, Vec<(String, String, String)>) {
        if let Some(evaluator_id) = evaluator_id {
            if let Some(agent) = session
                .agents
                .iter_mut()
                .find(|agent| agent.id == evaluator_id)
            {
                agent.commit_sha = commit_sha.map(str::to_string);
            }
        }

        // When a Prince peer is present, every Evaluator verdict (PASS or FAIL) hands
        // off to Prince remediation before the Queen may push. The verdict label and
        // findings live in qa-verdict.json for the Prince to read; the state machine
        // only needs to know remediation is owed. Operator overrides (force-pass /
        // force-fail) bypass this with route_to_prince = false.
        if route_to_prince {
            let changes =
                self.set_session_state_with_events(session, SessionState::PrinceRemediation);
            return (SessionState::PrinceRemediation, changes);
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
                    let changes = self
                        .set_session_state_with_events(session, SessionState::QaMaxRetriesExceeded);
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
            let has_prince = session
                .agents
                .iter()
                .any(|agent| matches!(agent.role, AgentRole::Prince));
            let (new_state, changes) = self.apply_qa_verdict_to_session(
                session,
                normalized.as_str(),
                Some(evaluator_id),
                commit_sha,
                has_prince,
            );
            (previous_session, session.clone(), changes, new_state)
        };

        if let Some(storage) = self.storage.as_ref() {
            if let Err(err) = Self::persist_session_snapshot(storage, &updated_session, session_id)
            {
                let mut sessions = self.sessions.write();
                if let Some(session) = sessions.get_mut(session_id) {
                    *session = previous_session;
                }
                return Err(err);
            }
        }

        self.cancel_qa_timeout(session_id);

        if matches!(&new_state, SessionState::PrinceRemediation) {
            let prince_id = format!("{}-prince", session_id);
            let prince_alive = self.pty_manager.read().is_alive(&prince_id);
            if !prince_alive {
                tracing::warn!(
                    session_id = %session_id,
                    prince_id = %prince_id,
                    "Respawning Prince after QA verdict because PTY is not alive"
                );
                let prince_config = AgentConfig {
                    cli: String::new(),
                    model: None,
                    flags: vec![],
                    label: Some("Prince".to_string()),
                    name: None,
                    description: None,
                    role: None,
                    initial_prompt: None,
                };
                if let Err(err) = self.launch_prince(session_id, prince_config, false) {
                    tracing::warn!(
                        session_id = %session_id,
                        prince_id = %prince_id,
                        error = %err,
                        "Failed to respawn Prince after QA verdict"
                    );
                }
            }
        }

        self.emit_session_update(session_id);
        self.emit_cell_status_changes(session_id, changes);

        Ok(new_state)
    }

    /// Mark QA inconclusive — the Evaluator reported BLOCKED, or the verdict timed
    /// out with no usable response. Transitions QaInProgress -> QaInconclusive,
    /// which blocks PR push / completion and surfaces to the operator. Writes a
    /// BLOCKED verdict file so the Queen's poll loop terminates (instead of hanging)
    /// and escalates rather than pushing. Operator unblocks via force-pass / force-fail.
    pub fn mark_qa_inconclusive(
        &self,
        session_id: &str,
        reason: &str,
    ) -> Result<SessionState, String> {
        self.cancel_qa_timeout(session_id);

        let (previous_session, updated_session, changes) = {
            let mut sessions = self.sessions.write();
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?;
            if !matches!(session.state, SessionState::QaInProgress { .. }) {
                return Err(format!(
                    "Cannot mark QA inconclusive: session is in {:?} state, expected QaInProgress",
                    session.state
                ));
            }
            let previous_session = session.clone();
            let changes = self.set_session_state_with_events(session, SessionState::QaInconclusive);
            (previous_session, session.clone(), changes)
        };

        if let Some(storage) = self.storage.as_ref() {
            if let Err(err) = Self::persist_session_snapshot(storage, &updated_session, session_id)
            {
                let mut sessions = self.sessions.write();
                if let Some(session) = sessions.get_mut(session_id) {
                    *session = previous_session;
                }
                return Err(err);
            }
        }

        // Write a BLOCKED verdict so the Queen's poll loop terminates and escalates.
        let verdict_content = serde_json::json!({
            "kind": "qa-verdict",
            "verdict": "BLOCKED",
            "blocked_reason": "inconclusive",
            "rationale": reason,
        })
        .to_string();
        let state_manager = StateManager::new(
            updated_session
                .project_path
                .join(".hive-manager")
                .join(session_id),
        );
        if let Err(err) = state_manager.write_qa_verdict(
            &format!("{}-evaluator", session_id),
            &format!("{}-queen", session_id),
            &verdict_content,
            None,
        ) {
            tracing::warn!(
                session_id = %session_id,
                error = %err,
                "Failed to persist BLOCKED verdict file while marking QA inconclusive"
            );
        }

        self.emit_session_update(session_id);
        self.emit_cell_status_changes(session_id, changes);
        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(
                "qa-inconclusive",
                serde_json::json!({
                    "session_id": session_id,
                    "action": "blocked",
                    "reason": reason,
                }),
            );
        }

        Ok(SessionState::QaInconclusive)
    }

    /// Record the Prince's remediation verdict. The Prince self-certifies after its
    /// fix team resolves the QA findings: PASS/DONE clears the gate (PrinceRemediation
    /// -> QaPassed, allowing the Queen to push), while BLOCKED escalates to the
    /// operator (PrinceRemediation -> QaInconclusive). Requires PrinceRemediation so
    /// a Prince can't jump the session straight to QaPassed before QA has run.
    pub fn record_prince_verdict(
        &self,
        session_id: &str,
        verdict: &str,
    ) -> Result<SessionState, String> {
        let normalized = verdict.trim().to_ascii_uppercase();
        let target_state = match normalized.as_str() {
            "PASS" | "DONE" | "RESOLVED" => SessionState::QaPassed,
            "BLOCKED" | "FAIL" | "ESCALATE" => SessionState::QaInconclusive,
            other => return Err(format!("Unsupported Prince verdict '{}'", other)),
        };

        let (previous_session, updated_session, changes) = {
            let mut sessions = self.sessions.write();
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?;
            if !matches!(session.state, SessionState::PrinceRemediation) {
                return Err(format!(
                    "Cannot record Prince verdict: session is in {:?} state, expected PrinceRemediation",
                    session.state
                ));
            }
            let previous_session = session.clone();
            let now = Utc::now();
            if now > session.last_activity_at {
                session.last_activity_at = now;
            }
            let changes = self.set_session_state_with_events(session, target_state.clone());
            (previous_session, session.clone(), changes)
        };

        if let Some(storage) = self.storage.as_ref() {
            if let Err(err) = Self::persist_session_snapshot(storage, &updated_session, session_id)
            {
                let mut sessions = self.sessions.write();
                if let Some(session) = sessions.get_mut(session_id) {
                    *session = previous_session;
                }
                return Err(err);
            }
        }

        self.emit_session_update(session_id);
        self.emit_cell_status_changes(session_id, changes);

        Ok(target_state)
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
    pub async fn on_worker_completed(
        &self,
        session_id: &str,
        worker_id: u8,
    ) -> Result<(), SessionError> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| SessionError::NotFound(format!("Session not found: {}", session_id)))?;

        // Verify we're in sequential mode and this is the expected worker
        if session.state != SessionState::WaitingForWorker(worker_id) {
            tracing::warn!(
                "Worker {} completed but session in state {:?}",
                worker_id,
                session.state
            );
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

        // #125: journal the worker's git commit as a confirmable side-effect. The commit
        // SHA is captured here, so the destructive op already landed: record Started +
        // ledger (effect_ref=SHA) then immediately Completed + confirm. On resume an
        // interrupted commit (Started, unconfirmed) is verified via `git cat-file -e`.
        if let Some(ref sha) = worker_commit_sha {
            use crate::domain::run_journal::{Confidence, StepKind, StepStatus};
            if let Some(step_id) = self.journal_step_started(
                session_id,
                StepKind::GitCommit,
                worker_id as u64,
                Some(&worker_agent_id),
            ) {
                if let Some(store) = self.run_journal.as_ref() {
                    let _ = store.record_ledger(
                        session_id,
                        &step_id,
                        "git_commit",
                        Some(sha),
                        Confidence::Uncertain,
                    );
                    let _ = store.confirm_ledger(session_id, &step_id, Some(sha), Confidence::High);
                }
                self.journal_step_finished(session_id, &step_id, StepStatus::Completed);
            }
        }

        let shared_cell_principal = matches!(&session.session_type, SessionType::Hive { .. })
            && session.execution_policy.workspace_strategy == WorkspaceStrategy::SharedCell;
        if Self::require_commit_sha_gate_enabled()
            && !shared_cell_principal
            && worker_commit_sha.is_none()
        {
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
        let pending_config_path = session
            .project_path
            .join(".hive-manager")
            .join(session_id)
            .join("pending-config.json");
        if !pending_config_path.exists() {
            tracing::info!("No pending config found for session {} - workers may have been spawned via HTTP API", session_id);
            return Ok(());
        }

        let config_json = std::fs::read_to_string(&pending_config_path).map_err(|e| {
            SessionError::ConfigError(format!("Failed to read pending config: {}", e))
        })?;
        let config: HiveLaunchConfig = serde_json::from_str(&config_json).map_err(|e| {
            SessionError::ConfigError(format!("Failed to parse pending config: {}", e))
        })?;

        // Get queen_id
        let queen_id = format!("{}-queen", session_id);

        // 1. Terminate the completed worker's PTY
        self.terminate_worker(session_id, worker_id)?;

        // 2. Spawn next worker
        let next_worker_index = worker_id as usize;
        self.spawn_next_worker(session_id, next_worker_index, &config, &queen_id)
            .await?;

        Ok(())
    }

    #[allow(dead_code)]
    pub fn on_milestone_ready(&self, session_id: &str) -> Result<(), String> {
        let (maybe_evaluator, config) = {
            let sessions = self.sessions.read();
            let session = sessions
                .get(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?;

            if matches!(
                &session.state,
                SessionState::PrinceRemediation
                    | SessionState::QaInconclusive
                    | SessionState::QaPassed
                    | SessionState::QaMaxRetriesExceeded
            ) {
                tracing::debug!(
                    session_id = %session_id,
                    state = ?session.state,
                    "Ignoring duplicate milestone-ready signal for gated QA session"
                );
                return Ok(());
            }

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
            // Operator overrides (force-pass / force-fail) and the legacy timeout path
            // are explicit decisions to resolve QA directly — they bypass the Prince.
            self.apply_qa_verdict_to_session(session, normalized.as_str(), None, None, false)
        };

        self.emit_session_update(session_id);
        self.update_session_storage(session_id);
        self.emit_cell_status_changes(session_id, changes);

        Ok(new_state)
    }

    #[allow(dead_code)]
    pub fn on_qa_timeout(&self, session_id: &str) -> Result<(), String> {
        let timeout_secs = self
            .get_session(session_id)
            .map(|session| session.qa_timeout_secs)
            .unwrap_or(DEFAULT_QA_TIMEOUT_SECS);
        tracing::warn!(
            "QA timed out for session {} after {} seconds; marking inconclusive (no auto-pass)",
            session_id,
            timeout_secs
        );
        let reason = format!(
            "QA verdict timed out after {}s with no response. Likely a verdict that could not be delivered over HTTP, or a pass-criterion that needs a UI/host that isn't running. Operator action required (force-pass / force-fail).",
            timeout_secs
        );
        self.mark_qa_inconclusive(session_id, &reason)?;
        Ok(())
    }

    /// Start a QA timeout timer. On expiry, marks QA inconclusive, writes a
    /// BLOCKED verdict, and surfaces the session for operator action. Cancel by
    /// calling `cancel_qa_timeout`.
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
                tracing::warn!(
                    "QA timeout fired for session {} after {}s — marking QA inconclusive (no auto-pass)",
                    sid,
                    timeout_secs
                );

                // A timed-out QA must NOT silently ship. Transition to QaInconclusive,
                // which blocks PR push / completion and surfaces to the operator. The
                // operator unblocks with force-pass / force-fail.
                let transition = {
                    let mut sessions = sessions.write();
                    if let Some(session) = sessions.get_mut(&sid) {
                        let previous_state = session.state.clone();
                        let changes = cell_status_changes_for_transition(
                            session,
                            &SessionState::QaInconclusive,
                        );
                        session.state = SessionState::QaInconclusive;
                        Some((previous_state, changes, session.clone()))
                    } else {
                        None
                    }
                };

                if let Some((previous_state, changes, updated_session)) = transition {
                    if let Some(storage) = storage.as_ref() {
                        if let Err(error) = SessionController::persist_session_snapshot(
                            storage,
                            &updated_session,
                            &sid,
                        ) {
                            tracing::warn!(
                                "Failed to persist QA timeout state for {}: {}",
                                sid,
                                error
                            );
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

                    // Write a BLOCKED verdict file so the Queen's poll loop terminates
                    // (instead of hanging forever) and she escalates rather than pushes.
                    let reason = format!(
                        "QA verdict timed out after {}s with no response. Likely a verdict that could not be delivered over HTTP, or a pass-criterion that needs a UI/host that isn't running. Operator action required (force-pass / force-fail).",
                        timeout_secs
                    );
                    let verdict_content = serde_json::json!({
                        "kind": "qa-verdict",
                        "verdict": "BLOCKED",
                        "blocked_reason": "timeout",
                        "rationale": reason,
                    })
                    .to_string();
                    let state_manager = StateManager::new(
                        updated_session
                            .project_path
                            .join(".hive-manager")
                            .join(&sid),
                    );
                    if let Err(err) = state_manager.write_qa_verdict(
                        &format!("{}-evaluator", sid),
                        &format!("{}-queen", sid),
                        &verdict_content,
                        None,
                    ) {
                        tracing::warn!(
                            "Failed to persist BLOCKED verdict file on QA timeout for {}: {}",
                            sid,
                            err
                        );
                    }

                    if let Some(ref app_handle) = app_handle {
                        let _ = app_handle.emit(
                            "session-update",
                            SessionUpdate {
                                session: updated_session,
                            },
                        );
                        let _ = app_handle.emit(
                            "qa-inconclusive",
                            serde_json::json!({
                                "session_id": sid,
                                "action": "blocked-on-timeout",
                                "reason": reason,
                            }),
                        );
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

    pub async fn on_fusion_variant_completed(
        &self,
        session_id: &str,
        variant_index: u8,
    ) -> Result<(), SessionError> {
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
            .ok_or_else(|| {
                SessionError::ConfigError(format!(
                    "Unknown fusion variant index: {}",
                    variant_index
                ))
            })?;

        {
            let pty_manager = self.pty_manager.read();
            if let Err(e) = pty_manager.kill(&variant.agent_id) {
                tracing::warn!(
                    "Failed to stop fusion variant PTY {}: {}",
                    variant.agent_id,
                    e
                );
            }
        }

        let completed_agent = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                if let Some(index) = s
                    .agents
                    .iter()
                    .position(|agent| agent.id == variant.agent_id)
                {
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
            sessions
                .get(session_id)
                .map(|s| {
                    matches!(
                        s.state,
                        SessionState::SpawningJudge
                            | SessionState::Judging
                            | SessionState::AwaitingVerdictSelection
                            | SessionState::MergingWinner
                            | SessionState::Completed
                    )
                })
                .unwrap_or(false)
        };
        if already_judging {
            return Ok(());
        }

        if metadata
            .variants
            .iter()
            .all(|v| Self::is_task_completed(&v.task_file))
        {
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

        let judge_prompt = Self::build_fusion_judge_prompt(
            session_id,
            &metadata.variants,
            &metadata.decision_file,
        );
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

    pub fn get_fusion_variant_statuses(
        &self,
        session_id: &str,
    ) -> Result<Vec<FusionVariantStatus>, String> {
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

    pub fn get_fusion_evaluation(
        &self,
        session_id: &str,
    ) -> Result<(String, Option<String>), String> {
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

    pub async fn on_debate_round_completed(
        &self,
        session_id: &str,
        debater_index: u8,
        round: u8,
    ) -> Result<(), SessionError> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| SessionError::NotFound(format!("Session not found: {}", session_id)))?;

        if !matches!(session.session_type, SessionType::Debate { .. }) {
            return Ok(());
        }

        let metadata = Self::read_debate_metadata(&session.project_path, session_id)
            .map_err(SessionError::ConfigError)?;
        let debater = metadata
            .debaters
            .iter()
            .find(|d| d.index == debater_index)
            .ok_or_else(|| {
                SessionError::ConfigError(format!(
                    "Unknown debate debater index: {}",
                    debater_index
                ))
            })?;
        let agent_id = Self::debate_round_agent_id(session_id, debater.index, round);

        {
            let pty_manager = self.pty_manager.read();
            if let Err(e) = pty_manager.kill(&agent_id) {
                tracing::warn!("Failed to stop debate debater PTY {}: {}", agent_id, e);
            }
        }

        let completed_agent = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                if let Some(index) = s.agents.iter().position(|agent| agent.id == agent_id) {
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
            sessions
                .get(session_id)
                .map(|s| {
                    matches!(
                        s.state,
                        SessionState::SpawningJudge
                            | SessionState::Judging
                            | SessionState::AwaitingVerdictSelection
                            | SessionState::Completed
                    )
                })
                .unwrap_or(false)
        };
        if already_judging {
            return Ok(());
        }

        let all_round_tasks_complete = metadata.debaters.iter().all(|d| {
            let path =
                Self::debate_round_task_file_path(Path::new(&d.worktree_path), d.index, round)
                    .to_string_lossy()
                    .to_string();
            Self::is_task_completed(&path)
        });

        if all_round_tasks_complete {
            if round < metadata.rounds {
                let next_round = round + 1;
                let next_round_started = {
                    let sessions = self.sessions.read();
                    sessions
                        .get(session_id)
                        .map(|s| {
                            metadata.debaters.iter().any(|d| {
                                let id =
                                    Self::debate_round_agent_id(session_id, d.index, next_round);
                                s.agents.iter().any(|agent| agent.id == id)
                            })
                        })
                        .unwrap_or(false)
                };
                if !next_round_started {
                    self.spawn_debate_round(session_id, next_round)
                        .map_err(SessionError::SpawnError)?;
                }
            } else {
                self.spawn_debate_judge(session_id)
                    .map_err(SessionError::SpawnError)?;
            }
        }

        Ok(())
    }

    fn spawn_debate_judge(&self, session_id: &str) -> Result<(), String> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        if !matches!(session.session_type, SessionType::Debate { .. }) {
            return Err(format!("Session {} is not a Debate session", session_id));
        }

        let metadata = Self::read_debate_metadata(&session.project_path, session_id)?;
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
            sessions
                .get_mut(session_id)
                .map(|s| self.set_session_state_with_events(s, SessionState::SpawningJudge))
        };
        if let Some(changes) = spawning_changes {
            self.emit_cell_status_changes(session_id, changes);
        }
        self.emit_session_update(session_id);

        let global_wiki_path = self
            .storage
            .as_ref()
            .and_then(|storage| storage.load_config().ok())
            .and_then(|cfg| cfg.global_wiki_path)
            .unwrap_or_default();
        let global_wiki_path = expand_tilde(&global_wiki_path);

        // Resolve the judge's effective CLI/model BEFORE rendering: the prompt spells the
        // wiki path differently for a WSL-backed CLI, so it must see the post-fallback
        // value, not a blank `judge_config.cli`.
        let mut judge_config = metadata.judge_config.clone();
        if judge_config.cli.trim().is_empty() {
            judge_config.cli = session.default_cli.clone();
        }
        if judge_config.model.is_none() {
            judge_config.model = session.default_model.clone();
        }

        let judge_prompt = Self::build_debate_judge_prompt(
            session_id,
            &metadata,
            &global_wiki_path,
            &judge_config.cli,
        );
        let prompt_file = Self::write_prompt_file(
            &session.project_path,
            session_id,
            "debate-judge-prompt.md",
            &judge_prompt,
        )?;
        let prompt_path = prompt_file.to_string_lossy().to_string();

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
                .map_err(|e| format!("Failed to spawn debate judge: {}", e))?;
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

    pub fn get_debate_debater_statuses(
        &self,
        session_id: &str,
    ) -> Result<Vec<DebateDebaterStatus>, String> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        if !matches!(session.session_type, SessionType::Debate { .. }) {
            return Err(format!("Session {} is not a Debate session", session_id));
        }

        let metadata = Self::read_debate_metadata(&session.project_path, session_id)?;
        Ok(metadata
            .debaters
            .iter()
            .map(|d| {
                let latest_task = (1..=metadata.rounds)
                    .rev()
                    .map(|round| {
                        Self::debate_round_task_file_path(
                            Path::new(&d.worktree_path),
                            d.index,
                            round,
                        )
                    })
                    .find(|path| path.exists())
                    .unwrap_or_else(|| {
                        Self::debate_round_task_file_path(Path::new(&d.worktree_path), d.index, 1)
                    });
                DebateDebaterStatus {
                    index: d.index,
                    name: d.name.clone(),
                    stance: d.stance.clone(),
                    branch: d.branch.clone(),
                    worktree_path: d.worktree_path.clone(),
                    status: Self::read_task_status(&latest_task.to_string_lossy()),
                }
            })
            .collect())
    }

    pub fn get_debate_evaluation(
        &self,
        session_id: &str,
    ) -> Result<(String, Option<String>), String> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        if !matches!(session.session_type, SessionType::Debate { .. }) {
            return Err(format!("Session {} is not a Debate session", session_id));
        }

        let metadata = Self::read_debate_metadata(&session.project_path, session_id)?;
        let report = match std::fs::read_to_string(&metadata.verdict_file) {
            Ok(content) => Some(content),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(format!("Failed to read debate verdict: {}", err)),
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

        Ok((metadata.verdict_file, report))
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
            .ok_or_else(|| {
                format!(
                    "Variant '{}' not found for session {}",
                    requested, session_id
                )
            })?;

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

        Self::run_git_in_dir(
            &session.project_path,
            &["merge", "--squash", &winner.branch],
        )?;

        // Commit the squash merge (--squash only stages changes, doesn't commit)
        Self::run_git_in_dir(
            &session.project_path,
            &[
                "commit",
                "-m",
                &format!("Merge fusion winner: {}", winner.name),
            ],
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
        pty_manager.kill(&worker_agent_id).map_err(|e| {
            SessionError::TerminationError(format!("Failed to kill worker {}: {}", worker_id, e))
        })?;

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
        }
        .ok_or_else(|| format!("Session not found: {}", session_id))?;

        // Verify session is in Planning or PlanReady state
        if session.state != SessionState::Planning && session.state != SessionState::PlanReady {
            return Err(format!(
                "Session is not in planning phase: {:?}",
                session.state
            ));
        }

        // Dispatch based on session type
        match &session.session_type {
            SessionType::Swarm { .. } => {
                return self.continue_swarm_after_planning(session_id, &session);
            }
            SessionType::Fusion { .. } => {
                return self.continue_fusion_after_planning(session_id, &session);
            }
            SessionType::Debate { .. } => {
                return self.continue_debate_after_planning(session_id, &session);
            }
            SessionType::Solo { .. } => {
                return Err("Solo sessions do not support planning continuation".to_string());
            }
            _ => {} // Continue with Hive logic below
        }

        // Load the pending config
        let pending_config_path = session
            .project_path
            .join(".hive-manager")
            .join(session_id)
            .join("pending-config.json");
        let config_json = std::fs::read_to_string(&pending_config_path)
            .map_err(|e| format!("Failed to read pending config: {}", e))?;
        let config: HiveLaunchConfig = serde_json::from_str(&config_json)
            .map_err(|e| format!("Failed to parse pending config: {}", e))?;
        let mut continuation_created_cells = Vec::new();
        let (cwd, worktree_branch) = match session.execution_policy.workspace_strategy {
            WorkspaceStrategy::SharedCell => (
                session.worktree_path.clone().ok_or_else(|| {
                    format!(
                        "Shared-cell session {} is missing its primary worktree path",
                        session_id
                    )
                })?,
                session
                    .worktree_branch
                    .clone()
                    .unwrap_or_else(|| format!("hive/{}/primary", session_id)),
            ),
            WorkspaceStrategy::IsolatedCell => {
                let branch = session
                    .worktree_branch
                    .clone()
                    .unwrap_or_else(|| format!("hive/{}/queen", session_id));
                if let Some(path) = session.worktree_path.clone() {
                    (path, branch)
                } else {
                    // Compatibility for planning sessions persisted before isolated
                    // Queen worktrees were allocated during the planning phase.
                    let base_ref = resolve_fresh_base(&session.project_path);
                    let (_, path) = create_session_worktree(
                        session_id,
                        "queen",
                        &branch,
                        &base_ref,
                        &session.project_path,
                    )?;
                    continuation_created_cells.push(("queen".to_string(), branch.clone()));
                    self.emit_workspace_created(session_id, PRIMARY_CELL_ID, &branch, Some(&path));
                    (path, branch)
                }
            }
            WorkspaceStrategy::None => {
                return Err("Planning Hive sessions require a managed git workspace".to_string())
            }
        };

        // Clean up Master Planner PTY before spawning Queen (fixes terminal corruption)
        let planner_id = format!("{}-master-planner", session_id);
        if let Err(e) = self.stop_agent(session_id, &planner_id) {
            tracing::warn!("Failed to stop Master Planner {}: {}", planner_id, e);
        } else {
            tracing::info!(
                "Stopped Master Planner {} before spawning Queen",
                planner_id
            );
            // Remove Master Planner from agents list to prevent resource leaks
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.agents.retain(|a| a.id != planner_id);
            }
        }

        let mut new_agents = Vec::new();

        // Create Queen agent
        let queen_id = format!("{}-queen", session_id);
        let (cmd, mut args) = Self::build_command(&config.queen_config);

        // Plan should exist now
        let has_plan = session
            .project_path
            .join(".hive-manager")
            .join(session_id)
            .join("plan.md")
            .exists();

        // Write Queen prompt with plan reference
        let master_prompt = Self::build_queen_master_prompt(
            &config.queen_config,
            &session.project_path,
            Path::new(&cwd),
            session_id,
            &config.workers,
            config.prompt.as_deref(),
            has_plan,
            config.with_evaluator,
            &session.execution_policy,
        );
        let prompt_file = match Self::write_prompt_file(
            &session.project_path,
            session_id,
            "queen-prompt.md",
            &master_prompt,
        ) {
            Ok(path) => path,
            Err(error) => {
                self.rollback_launch_allocations(
                    &session.project_path,
                    session_id,
                    &continuation_created_cells,
                    &[],
                );
                return Err(error);
            }
        };
        let prompt_path = prompt_file.to_string_lossy().to_string();
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

        // Write tool documentation files
        let principal_cli = config
            .workers
            .first()
            .map(|principal| principal.cli.as_str())
            .unwrap_or("codex");
        if let Err(error) = Self::write_tool_files(&session.project_path, session_id, principal_cli)
        {
            self.rollback_launch_allocations(
                &session.project_path,
                session_id,
                &continuation_created_cells,
                &[],
            );
            return Err(error);
        }

        tracing::info!(
            "Launching Queen agent (after planning): {} {:?} in {:?}",
            cmd,
            args,
            cwd
        );

        if let Err(error) = self.pty_manager.read().create_session(
            queen_id.clone(),
            AgentRole::Queen,
            &cmd,
            &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            Some(&cwd),
            120,
            30,
        ) {
            self.rollback_launch_allocations(
                &session.project_path,
                session_id,
                &continuation_created_cells,
                &[],
            );
            return Err(format!("Failed to spawn Queen: {}", error));
        }

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

        // Update session with new agents - Queen will spawn workers
        let (updated_session, changes) = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.worktree_path = Some(cwd.clone());
                s.worktree_branch = Some(worktree_branch);
                if s.default_principal_cli.is_none() {
                    let (cli, model, flags) = Self::configured_principal_defaults(&config.workers);
                    s.default_principal_cli = cli;
                    s.default_principal_model = model;
                    s.default_principal_flags = flags;
                }
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
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: updated_session.clone(),
                },
            );
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
                    let _ = app_handle.emit(
                        "session-update",
                        SessionUpdate {
                            session: session.clone(),
                        },
                    );
                }
                self.emit_cell_status_changes(session_id, changes);
                Ok(())
            } else {
                Err(format!(
                    "Session is not in planning state: {:?}",
                    session.state
                ))
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
        let persisted = storage
            .load_session(session_id)
            .map_err(|e| format!("Failed to load session from storage: {}", e))?;
        storage
            .mark_session_synced(session_id, &persisted)
            .map_err(|e| format!("Failed to track session storage state: {}", e))?;

        // Convert persisted session to active session
        let mut session = self.session_from_persisted(&persisted)?;

        // #125: classify the run journal for this resumed session — mark completed
        // write-steps Skipped (the spawn/commit guards keep them from re-running) and
        // verify unconfirmed ledger effects. Attach the report for the frontend modal.
        let report = self.build_resume_report(session_id, &session.project_path);
        if !report.is_empty() {
            session.resume_report = Some(report);
        }

        // Add to in-memory sessions
        {
            let mut sessions = self.sessions.write();
            sessions.insert(session.id.clone(), session.clone());
        }

        self.ensure_task_watcher(&session.id, &session.project_path);

        // Emit session-update event to frontend
        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: session.clone(),
                },
            );
        }

        Ok(session)
    }

    /// #125: read the run journal, classify each step, mark completed write-steps as
    /// Skipped, and verify unconfirmed ledger effects against the repo. Returns a
    /// [`ResumeReport`](crate::domain::run_journal::ResumeReport). Empty (and cheap) when
    /// no journal store is attached or the run has no journaled steps.
    fn build_resume_report(
        &self,
        run_id: &str,
        project_path: &Path,
    ) -> crate::domain::run_journal::ResumeReport {
        use crate::domain::run_journal::{Confidence, ResumeReport, StepStatus};

        let mut report = ResumeReport::default();
        let Some(store) = self.run_journal.as_ref() else {
            return report;
        };
        let entries = match store.read_journal(run_id) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(run_id, "Failed to read run journal on resume: {}", e);
                return report;
            }
        };

        for entry in entries {
            let ledger = store
                .read_ledger_for_step(run_id, &entry.step_id)
                .ok()
                .flatten();
            let has_unconfirmed = ledger.as_ref().map(|l| !l.confirmed).unwrap_or(false);
            let classified = crate::storage::run_journal::classify_step(&entry, has_unconfirmed);

            match classified {
                StepStatus::Completed => {
                    // Completed write-steps are skipped on resume so destructive ops
                    // (git commit/branch, worker/evaluator spawn) are never re-run.
                    if entry.kind.is_write_step() {
                        let _ =
                            store.record_step_finished(run_id, &entry.step_id, StepStatus::Skipped);
                        let mut skipped = entry.clone();
                        skipped.status = StepStatus::Skipped;
                        report.skipped.push(skipped);
                    }
                }
                StepStatus::Unknown => {
                    // An interrupted write-step with an unconfirmed ledger effect:
                    // verify the side-effect actually landed and set Confidence.
                    if let Some(led) = ledger {
                        let confidence = self.verify_ledger_effect(
                            project_path,
                            entry.kind,
                            led.effect_ref.as_deref(),
                        );
                        let _ = store.confirm_ledger(run_id, &entry.step_id, None, confidence);
                        if confidence == Confidence::High {
                            let _ = store.record_step_finished(
                                run_id,
                                &entry.step_id,
                                StepStatus::Completed,
                            );
                        } else {
                            let mut recovered = led;
                            recovered.confidence = confidence;
                            report.uncertain.push(recovered);
                            report.interrupted.push(entry);
                        }
                    } else {
                        report.interrupted.push(entry);
                    }
                }
                StepStatus::Interrupted => {
                    report.interrupted.push(entry);
                }
                _ => {}
            }
        }

        report
    }

    /// Verify a journaled side-effect still exists in the repo. Commit SHAs are checked
    /// with `git cat-file -e`; branch names with `git rev-parse --verify`. Returns
    /// [`Confidence::High`] when found, [`Confidence::Uncertain`] otherwise.
    fn verify_ledger_effect(
        &self,
        project_path: &Path,
        kind: crate::domain::run_journal::StepKind,
        effect_ref: Option<&str>,
    ) -> crate::domain::run_journal::Confidence {
        use crate::domain::run_journal::{Confidence, StepKind};
        let Some(reference) = effect_ref else {
            return Confidence::Uncertain;
        };
        let project_path = project_path.to_path_buf();
        let found = match kind {
            StepKind::GitCommit => {
                Self::run_git_in_dir(&project_path, &["cat-file", "-e", reference]).is_ok()
            }
            StepKind::GitBranch | StepKind::WorkerSpawn => {
                Self::run_git_in_dir(&project_path, &["rev-parse", "--verify", reference]).is_ok()
            }
            _ => false,
        };
        if found {
            Confidence::High
        } else {
            Confidence::Uncertain
        }
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
            crate::storage::SessionTypeInfo::Debate { variants } => SessionType::Debate {
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

        let mut execution_policy = persisted.execution_policy.clone();
        if persisted.no_git {
            // Legacy Research snapshots predate execution_policy and therefore
            // deserialize to IsolatedCell. No-git sessions always execute in the
            // project checkout and use session-qualified task files.
            execution_policy.workspace_strategy = WorkspaceStrategy::None;
        }

        Ok(Session {
            id: persisted.id.clone(),
            name: persisted.name.clone(),
            color: persisted.color.clone(),
            session_type,
            project_path: PathBuf::from(&persisted.project_path),
            state,
            created_at: persisted.created_at,
            last_activity_at: persisted.last_activity_at.unwrap_or(persisted.created_at),
            agents,
            default_cli: persisted.default_cli.clone(),
            default_model: persisted.default_model.clone(),
            default_principal_cli: persisted.default_principal_cli.clone(),
            default_principal_model: persisted.default_principal_model.clone(),
            default_principal_flags: persisted.default_principal_flags.clone(),
            execution_policy,
            qa_workers: persisted.qa_workers.clone(),
            max_qa_iterations: persisted.max_qa_iterations,
            qa_timeout_secs: persisted.qa_timeout_secs,
            auth_strategy,
            worktree_path: persisted.worktree_path.clone(),
            worktree_branch: persisted.worktree_branch.clone(),
            no_git: persisted.no_git,
            resume_report: None,
        })
    }

    /// Continue a Swarm session after planning phase
    fn continue_swarm_after_planning(
        &self,
        session_id: &str,
        session: &Session,
    ) -> Result<Session, String> {
        let cwd = session.project_path.to_str().unwrap_or(".");

        // Load the pending Swarm config
        let pending_config_path = session
            .project_path
            .join(".hive-manager")
            .join(session_id)
            .join("pending-swarm-config.json");
        let config_json = std::fs::read_to_string(&pending_config_path)
            .map_err(|e| format!("Failed to read pending swarm config: {}", e))?;
        let config: SwarmLaunchConfig = serde_json::from_str(&config_json)
            .map_err(|e| format!("Failed to parse pending swarm config: {}", e))?;
        let default_cli = if config.default_cli.trim().is_empty() {
            session.default_cli.clone()
        } else {
            config.default_cli.trim().to_string()
        };

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
            tracing::info!(
                "Stopped Master Planner {} before spawning Queen",
                planner_id
            );
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
                &default_cli,
                &session.project_path,
                session_id,
                &planners,
                config.prompt.as_deref(),
                config.with_evaluator,
            );
            let prompt_file = Self::write_prompt_file(
                &session.project_path,
                session_id,
                "queen-prompt.md",
                &master_prompt,
            )?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            // Write Swarm tool documentation files (includes spawn-planner.md)
            Self::write_swarm_tool_files(
                &session.project_path,
                session_id,
                planners.len() as u8,
                &default_cli,
            )?;

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
        let swarm_config_path = session
            .project_path
            .join(".hive-manager")
            .join(session_id)
            .join("swarm-planners.json");
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
                (session.clone(), changes) // Queen will spawn planners sequentially
            } else {
                return Err("Session disappeared".to_string());
            }
        };

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: updated_session.clone(),
                },
            );
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
        let default_cli = config.default_cli.trim().to_string();
        let default_model = config.default_model.clone();

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
                &default_cli,
                &project_path,
                &session_id,
                &planners,
                config.prompt.as_deref(),
                config.with_evaluator,
            );
            let prompt_file = Self::write_prompt_file(
                &project_path,
                &session_id,
                "queen-prompt.md",
                &master_prompt,
            )?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            // Write Swarm tool documentation files (includes spawn-planner.md)
            Self::write_swarm_tool_files(
                &project_path,
                &session_id,
                planners.len() as u8,
                &default_cli,
            )?;

            tracing::info!(
                "Launching Queen agent (swarm - sequential planner spawning): {} {:?} in {:?}",
                cmd,
                args,
                cwd
            );

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
        let swarm_config_path = project_path
            .join(".hive-manager")
            .join(&session_id)
            .join("swarm-planners.json");
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
            session_type: SessionType::Swarm {
                planner_count: planners.len() as u8,
            },
            project_path,
            state: SessionState::Running, // Queen will spawn planners sequentially
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli,
            default_model,
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy: HiveExecutionPolicy::default(),
            qa_workers: config.qa_workers.clone().unwrap_or_default(),
            max_qa_iterations,
            qa_timeout_secs,
            auth_strategy,
            worktree_path: None,
            worktree_branch: None,
            no_git: false,
            resume_report: None,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit(
                "session-update",
                SessionUpdate {
                    session: session.clone(),
                },
            );
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

        // The Prince is a peer to the Queen and Evaluator, auto-spawned alongside the
        // Evaluator. It idles until the QA verdict lands, then resolves the findings
        // with its own fix team and gates the Queen's PR push. Inherits the session
        // default CLI (empty cli -> resolved in launch_prince).
        let prince_config = AgentConfig {
            cli: String::new(),
            model: None,
            flags: vec![],
            label: Some("Prince".to_string()),
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        };
        let _prince = self.launch_prince(session_id, prince_config, smoke_test)?;

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
        }
        .ok_or_else(|| format!("Session not found: {}", session_id))?;

        if !Self::session_allows_dynamic_principal(&session, &role, parent_id.as_deref()) {
            return Err(format!(
                "Cannot add a managed principal to {:?}; dynamic principals are supported only by Hive sessions",
                session.session_type
            ));
        }
        if session.no_git && !role.role_type.eq_ignore_ascii_case("researcher") {
            return Err("Research sessions accept only read-only researcher workers".to_string());
        }
        if let Some(explicit_parent) = parent_id.as_deref() {
            let parent = session
                .agents
                .iter()
                .find(|agent| agent.id == explicit_parent)
                .ok_or_else(|| {
                    format!(
                        "Parent agent {} does not belong to session {}",
                        explicit_parent, session_id
                    )
                })?;
            if !matches!(
                parent.role,
                AgentRole::Queen | AgentRole::Planner { .. } | AgentRole::Prince
            ) {
                return Err(format!(
                    "Agent {} cannot parent a managed principal",
                    explicit_parent
                ));
            }
        }

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
                | SessionState::PrinceRemediation
        );
        if !can_add_worker {
            return Err(format!(
                "Cannot add worker to session in state {:?}",
                session.state
            ));
        }

        // Determine worker index
        let existing_workers = session
            .agents
            .iter()
            .filter(|a| matches!(a.role, AgentRole::Worker { .. }))
            .count();
        let worker_index = (existing_workers + 1) as u8;

        // Determine parent (default to Queen)
        let actual_parent_id = parent_id.unwrap_or_else(|| format!("{}-queen", session_id));

        // Generate worker ID
        let worker_id = format!("{}-worker-{}", session_id, worker_index);

        let config_with_role = Self::apply_worker_identity(worker_index, &role, config);
        let (cmd, mut args) = Self::build_command(&config_with_role);
        let uses_shared_workspace = !session.no_git
            && matches!(&session.session_type, SessionType::Hive { .. })
            && session.execution_policy.workspace_strategy == WorkspaceStrategy::SharedCell;
        let creates_worker_worktree = !session.no_git && !uses_shared_workspace;
        let worker_branch = if uses_shared_workspace {
            session
                .worktree_branch
                .clone()
                .unwrap_or_else(|| format!("hive/{}/primary", session_id))
        } else {
            format!("hive/{}/worker-{}", session_id, worker_index)
        };
        // Research (no-git) sessions never create worktrees or branches: the worker
        // runs directly in the project directory, mirroring the no-worktree launch path
        // in `launch_hive_internal`. This keeps the Queen's on-demand spawning working
        // on non-repo folders and honors the research "no git" contract.
        let worker_cwd = if session.no_git {
            session.project_path.to_string_lossy().to_string()
        } else if uses_shared_workspace {
            session.worktree_path.clone().ok_or_else(|| {
                format!(
                    "Shared-cell session {} is missing its primary worktree path",
                    session_id
                )
            })?
        } else {
            // Late-spawned workers should branch from the most recent session-integrated commit when possible.
            let base_ref = Self::resolve_worker_base_ref(&session, "add_worker", worker_index);
            let (_, cwd) = create_session_worktree(
                session_id,
                &format!("worker-{}", worker_index),
                &worker_branch,
                &base_ref,
                &session.project_path,
            )?;
            cwd
        };
        if creates_worker_worktree {
            self.emit_workspace_created(
                session_id,
                PRIMARY_CELL_ID,
                &worker_branch,
                Some(&worker_cwd),
            );
        }

        let worker_cell_name = format!("worker-{worker_index}");
        let worker_base_commit_sha = if session.no_git {
            None
        } else {
            current_head(Path::new(&worker_cwd)).ok()
        };
        let task_file_path =
            Self::task_file_path_for_session_worker(&session, worker_index as usize)?;

        // Write task file for this worker (STANDBY or with initial task)
        let task_status = config_with_role.initial_prompt.as_deref().map(|_| "ACTIVE");
        let _task_file = match Self::write_task_file_at_path(
            &task_file_path,
            worker_index,
            config_with_role.initial_prompt.as_deref(),
            task_status,
            config_with_role
                .role
                .as_ref()
                .map(|r| r.role_type.eq_ignore_ascii_case("researcher"))
                .unwrap_or(false),
        ) {
            Ok(task_file) => task_file,
            Err(err) => {
                Self::rollback_worker_launch_artifacts(
                    &session.project_path,
                    session_id,
                    &worker_cell_name,
                    &task_file_path,
                    None,
                    creates_worker_worktree,
                );
                return Err(err);
            }
        };

        // Write worker prompt to file and add to args
        let worker_prompt = Self::build_worker_prompt(
            worker_index,
            &config_with_role,
            &actual_parent_id,
            session_id,
            &session.project_path,
            Path::new(&worker_cwd),
            &session.execution_policy,
        );
        let filename = format!("worker-{}-prompt.md", worker_index);
        let prompt_file = match Self::write_worker_prompt_file(
            Path::new(&worker_cwd),
            worker_index,
            &filename,
            &worker_prompt,
        ) {
            Ok(prompt_file) => prompt_file,
            Err(err) => {
                Self::rollback_worker_launch_artifacts(
                    &session.project_path,
                    session_id,
                    &worker_cell_name,
                    &task_file_path,
                    None,
                    creates_worker_worktree,
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

        let worker_role = AgentRole::Worker {
            index: worker_index,
            parent: Some(actual_parent_id.clone()),
        };

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
                    creates_worker_worktree,
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
            base_commit_sha: worker_base_commit_sha,
        };

        // Update session
        {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                session.agents.push(agent_info.clone());
                let live_worker_count = session
                    .agents
                    .iter()
                    .filter(|agent| matches!(agent.role, AgentRole::Worker { .. }))
                    .count()
                    .min(u8::MAX as usize) as u8;
                if let SessionType::Hive { worker_count } = &mut session.session_type {
                    *worker_count = (*worker_count).max(live_worker_count);
                }
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
    pub fn launch_evaluator(
        &self,
        session_id: &str,
        mut config: AgentConfig,
        smoke_test: bool,
    ) -> Result<AgentInfo, String> {
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

        let uses_session_default_cli = config.cli.trim().is_empty();
        if uses_session_default_cli {
            config.cli = session.default_cli.clone();
        }
        let uses_session_cli = uses_session_default_cli || config.cli.trim() == session.default_cli;
        if config.model.is_none() {
            config.model = if uses_session_cli {
                session.default_model.clone()
            } else {
                CliRegistry::default_model(&config.cli)
                    .map(ToString::to_string)
                    .or_else(|| session.default_model.clone())
            };
        }
        if config.label.is_none() {
            config.label = Some("Evaluator".to_string());
        }

        let spawning_changes = {
            let mut sessions = self.sessions.write();
            if let Some(current) = sessions.get_mut(session_id) {
                current.agents.retain(|agent| agent.id != evaluator_id);
                Some(self.set_session_state_with_events(current, SessionState::SpawningEvaluator))
            } else {
                None
            }
        };
        self.emit_session_update(session_id);
        self.update_session_storage(session_id);
        if let Some(changes) = spawning_changes {
            self.emit_cell_status_changes(session_id, changes);
        }

        Self::write_tool_files(
            &session.project_path,
            session_id,
            Self::session_principal_cli(&session),
        )?;

        let worker_count = match session.session_type {
            SessionType::Hive { worker_count } => worker_count,
            _ => 0,
        };
        let execution_workspace = Self::execution_workspace(&session);
        let evaluator_prompt = Self::build_evaluator_prompt(
            session_id,
            &config,
            &session.qa_workers,
            worker_count,
            &execution_workspace,
            smoke_test,
        );
        let prompt_file = Self::write_prompt_file(
            &session.project_path,
            session_id,
            "evaluator-prompt.md",
            &evaluator_prompt,
        )?;

        let (cmd, mut args) = Self::build_command(&config);
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_file.to_string_lossy());

        // #125: record the evaluator-spawn write-step as Started before the PTY spawn.
        let evaluator_journal_step = self.journal_step_started(
            session_id,
            crate::domain::run_journal::StepKind::EvaluatorSpawn,
            0,
            Some("evaluator"),
        );

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
                .map_err(|e| {
                    if let Some(step_id) = evaluator_journal_step.as_deref() {
                        self.journal_step_finished(
                            session_id,
                            step_id,
                            crate::domain::run_journal::StepStatus::Failed,
                        );
                    }
                    format!("Failed to spawn Evaluator: {}", e)
                })?;
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

        // #125: evaluator spawned successfully — mark the write-step Completed.
        if let Some(step_id) = evaluator_journal_step.as_deref() {
            self.journal_step_finished(
                session_id,
                step_id,
                crate::domain::run_journal::StepStatus::Completed,
            );
        }

        Ok(agent_info)
    }

    /// Launch the Prince peer. Mirrors `launch_evaluator` but does NOT touch the
    /// session state or arm the QA timeout: the Prince idles, polling for the QA
    /// verdict, and only acts once findings exist. Inherits the session default CLI
    /// so its fix team uses the single agent type the operator selected.
    pub fn launch_prince(
        &self,
        session_id: &str,
        mut config: AgentConfig,
        smoke_test: bool,
    ) -> Result<AgentInfo, String> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        let prince_id = format!("{}-prince", session_id);
        if let Some(existing) = session.agents.iter().find(|agent| agent.id == prince_id) {
            let prince_alive = self.pty_manager.read().is_alive(&prince_id);
            if prince_alive {
                return Ok(existing.clone());
            }
            tracing::info!(
                session_id = %session_id,
                prince_id = %prince_id,
                "Respawning stale prince after PTY exit"
            );
        }

        let uses_session_default_cli = config.cli.trim().is_empty();
        if uses_session_default_cli {
            config.cli = session.default_cli.clone();
        }
        let uses_session_cli = uses_session_default_cli || config.cli.trim() == session.default_cli;
        if config.model.is_none() {
            config.model = if uses_session_cli {
                session.default_model.clone()
            } else {
                CliRegistry::default_model(&config.cli)
                    .map(ToString::to_string)
                    .or_else(|| session.default_model.clone())
            };
        }
        if config.label.is_none() {
            config.label = Some("Prince".to_string());
        }

        let principal_defaults = self
            .get_session_principal_defaults(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;
        Self::write_tool_files(&session.project_path, session_id, &principal_defaults.cli)?;
        let execution_workspace = Self::execution_workspace(&session);
        let prince_prompt = Self::build_prince_prompt(
            session_id,
            &config,
            &principal_defaults,
            &execution_workspace,
            session.execution_policy.workspace_strategy,
            smoke_test,
        );
        let prompt_file = Self::write_prompt_file(
            &session.project_path,
            session_id,
            "prince-prompt.md",
            &prince_prompt,
        )?;

        let (cmd, mut args) = Self::build_command(&config);
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_file.to_string_lossy());

        {
            let mut sessions = self.sessions.write();
            if let Some(current) = sessions.get_mut(session_id) {
                current.agents.retain(|agent| agent.id != prince_id);
            }
        }

        let cwd = session.project_path.to_str().unwrap_or(".");
        {
            let pty_manager = self.pty_manager.read();
            pty_manager
                .create_session(
                    prince_id.clone(),
                    AgentRole::Prince,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Prince: {}", e))?;
        }

        let agent_info = AgentInfo {
            id: prince_id,
            role: AgentRole::Prince,
            status: AgentStatus::Running,
            config,
            parent_id: None,
            commit_sha: None,
            base_commit_sha: None,
        };

        {
            let mut sessions = self.sessions.write();
            let current = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?;
            current.agents.push(agent_info.clone());
            self.emit_agent_launched(current, &agent_info);
        }

        self.emit_session_update(session_id);
        self.update_session_storage(session_id);
        self.ensure_task_watcher(session_id, &session.project_path);

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
            &Self::execution_workspace(&session),
        );
        // QA workers spawned after evaluator launch run from the project root, not
        // isolated worker worktrees, so their prompts stay in the session prompt dir.
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
        }
        .ok_or_else(|| format!("Session not found: {}", session_id))?;

        // Allow adding planners when Running or WaitingForPlanner
        let can_add_planner = matches!(
            session.state,
            SessionState::Running | SessionState::WaitingForPlanner(_)
        );
        if !can_add_planner {
            return Err(format!(
                "Cannot add planner to session in state {:?}",
                session.state
            ));
        }

        // Determine planner index
        let existing_planners = session
            .agents
            .iter()
            .filter(|a| matches!(a.role, AgentRole::Planner { .. }))
            .count();
        let planner_index = (existing_planners + 1) as u8;

        // Get queen ID as parent
        let queen_id = format!("{}-queen", session_id);

        // Generate planner ID
        let planner_id = format!("{}-planner-{}", session_id, planner_index);

        // Build command
        let (cmd, mut args) = Self::build_command(&config);

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
        let prompt_file = Self::write_prompt_file(
            &session.project_path,
            session_id,
            &filename,
            &planner_prompt,
        )?;
        let prompt_path = prompt_file.to_string_lossy().to_string();
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

        // Write tool files for the planner (spawn-worker.md)
        Self::write_tool_files(
            &session.project_path,
            session_id,
            Self::session_principal_cli(&session),
        )?;

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
                    AgentRole::Planner {
                        index: planner_index,
                    },
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
            role: AgentRole::Planner {
                index: planner_index,
            },
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
                let _ = app_handle.emit(
                    "session-update",
                    SessionUpdate {
                        session: session.clone(),
                    },
                );
            }
        }

        // Update session storage
        self.update_session_storage(session_id);
        if let Some(changes) = waiting_changes {
            self.emit_cell_status_changes(session_id, changes);
        }
        self.ensure_task_watcher(session_id, &session.project_path);

        // Store planner's worker config for sequential spawning
        let planner_workers_path = session
            .project_path
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
        use crate::storage::{
            PersistedAgentConfig, PersistedAgentInfo, PersistedSession, SessionTypeInfo,
        };

        let session_type = match &session.session_type {
            SessionType::Hive { worker_count } => SessionTypeInfo::Hive {
                worker_count: *worker_count,
            },
            SessionType::Swarm { planner_count } => SessionTypeInfo::Swarm {
                planner_count: *planner_count,
            },
            SessionType::Fusion { variants } => SessionTypeInfo::Fusion {
                variants: variants.clone(),
            },
            SessionType::Debate { variants } => SessionTypeInfo::Debate {
                variants: variants.clone(),
            },
            SessionType::Solo { cli, model } => SessionTypeInfo::Solo {
                cli: cli.clone(),
                model: model.clone(),
            },
        };

        let agents: Vec<PersistedAgentInfo> = session
            .agents
            .iter()
            .map(|a| {
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
            })
            .collect();

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
            default_principal_cli: session.default_principal_cli.clone(),
            default_principal_model: session.default_principal_model.clone(),
            default_principal_flags: session.default_principal_flags.clone(),
            execution_policy: session.execution_policy.clone(),
            qa_workers: session.qa_workers.clone(),
            max_qa_iterations: session.max_qa_iterations,
            qa_timeout_secs: session.qa_timeout_secs,
            auth_strategy: auth_strategy.persist_value(),
            worktree_path: session.worktree_path.clone(),
            worktree_branch: session.worktree_branch.clone(),
            no_git: session.no_git,
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

            // Build worker state info
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
        let debate_worktrees_path = project_path.join(".hive-debate").join(session_id);

        match TaskFileWatcher::new(
            &session_path,
            &worktrees_path,
            &fusion_worktrees_path,
            &debate_worktrees_path,
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
    } else if role == "Prince" {
        Some(AgentRole::Prince)
    } else if role.starts_with("Planner(") {
        let index = role
            .trim_start_matches("Planner(")
            .trim_end_matches(")")
            .parse::<u8>()
            .ok()?;
        Some(AgentRole::Planner { index })
    } else if role.starts_with("Worker(") {
        parse_indexed_role(role, "Worker(")
            .map(|(index, parent)| AgentRole::Worker { index, parent })
    } else if role.starts_with("QaWorker(") {
        parse_indexed_role(role, "QaWorker(")
            .map(|(index, parent)| AgentRole::QaWorker { index, parent })
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
            iteration: iteration
                .parse::<u8>()
                .ok()
                .filter(|iteration| *iteration > 0),
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
        "SpawningDebateRound" => SessionState::SpawningDebateRound(0),
        "WaitingForDebateRound" => SessionState::WaitingForDebateRound(0),
        "SpawningJudge" => SessionState::SpawningJudge,
        "Judging" => SessionState::Judging,
        "AwaitingVerdictSelection" => SessionState::AwaitingVerdictSelection,
        "MergingWinner" => SessionState::MergingWinner,
        "SpawningEvaluator" => SessionState::SpawningEvaluator,
        "QaInProgress" => SessionState::QaInProgress { iteration: None },
        "QaPassed" => SessionState::QaPassed,
        "QaFailed" => SessionState::QaFailed { iteration: 1 },
        "QaMaxRetriesExceeded" => SessionState::QaMaxRetriesExceeded,
        "PrinceRemediation" => SessionState::PrinceRemediation,
        "QaInconclusive" => SessionState::QaInconclusive,
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
        SessionState::SpawningDebateRound(_) => "SpawningDebateRound".to_string(),
        SessionState::WaitingForDebateRound(_) => "WaitingForDebateRound".to_string(),
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
        SessionState::PrinceRemediation => "PrinceRemediation".to_string(),
        SessionState::QaInconclusive => "QaInconclusive".to_string(),
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
            format!(
                "QaWorker({},{})",
                index,
                parent.as_deref().unwrap_or("None")
            )
        }
        AgentRole::Prince => "Prince".to_string(),
        AgentRole::ScratchShell => "ScratchShell".to_string(),
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
        AgentRole::Prince => "prince",
        AgentRole::ScratchShell => "scratch-shell",
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
        AgentRole::Prince => "Prince".to_string(),
        AgentRole::ScratchShell => "ScratchShell".to_string(),
    }
}

fn include_in_worker_roster(role: &AgentRole) -> bool {
    !matches!(
        serialize_agent_role(role),
        "queen" | "evaluator" | "qa-worker" | "prince" | "scratch-shell"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        extract_model_arg, parse_persisted_session_state, serialize_session_state, AgentConfig,
        AgentInfo, AuthStrategy, CompletionError, DebateDebaterMetadata, DebateSessionMetadata,
        FusionVariantMetadata, QaWorkerConfig, Session, SessionController, SessionError,
        SessionState, SessionType,
    };
    use super::{heartbeat_cadence_label, CliBehavior, CliRegistry, ACTIVATION_POLL_INTERVAL};
    use crate::coordination::queue_manager::{
        HEARTBEAT_MAX_INTERVAL_SECS, HEARTBEAT_MIN_INTERVAL_SECS,
    };
    use crate::domain::{ArtifactBundle, HiveExecutionPolicy, WorkspaceStrategy};
    use crate::pty::{AgentRole, AgentStatus, PtyManager, WorkerRole};
    use crate::workspace::git::current_head;
    use chrono::{Duration, Utc};
    use parking_lot::RwLock;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn extract_model_arg_reads_short_long_and_equals_forms() {
        assert_eq!(
            extract_model_arg(&["--dangerously-skip-permissions", "-m", "gpt-5.5"]).as_deref(),
            Some("gpt-5.5")
        );
        assert_eq!(
            extract_model_arg(&["--model", "opus"]).as_deref(),
            Some("opus")
        );
        assert_eq!(
            extract_model_arg(&["--model=gpt-5.6-terra"]).as_deref(),
            Some("gpt-5.6-terra")
        );
        assert_eq!(extract_model_arg(&["--model="]), None);
        assert_eq!(extract_model_arg(&["-m"]), None);
    }

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
    fn stall_sweep_excludes_completed_heartbeats() {
        let controller = test_controller();
        controller
            .update_heartbeat("session-stall", "session-stall-worker-1", "working", None)
            .expect("record working heartbeat");
        controller
            .update_heartbeat(
                "session-stall",
                "session-stall-worker-2",
                "completed",
                Some("Queen verified completion"),
            )
            .expect("record completed heartbeat");

        let stale_at = Utc::now() - Duration::minutes(5);
        let mut heartbeats = controller.agent_heartbeats.write();
        for heartbeat in heartbeats
            .get_mut("session-stall")
            .expect("session heartbeat map")
            .values_mut()
        {
            heartbeat.last_activity = stale_at;
        }
        drop(heartbeats);

        let stalled = controller.get_stalled_agents(
            "session-stall",
            std::time::Duration::from_secs(30),
        );
        assert_eq!(stalled.len(), 1);
        assert_eq!(stalled[0].0, "session-stall-worker-1");
    }

    #[test]
    fn only_hive_and_legacy_swarm_accept_dynamic_managed_principals() {
        assert!(SessionController::session_type_supports_dynamic_principals(
            &SessionType::Hive { worker_count: 1 }
        ));
        assert!(SessionController::session_type_supports_dynamic_principals(
            &SessionType::Swarm { planner_count: 1 }
        ));
        assert!(
            !SessionController::session_type_supports_dynamic_principals(&SessionType::Solo {
                cli: "codex".to_string(),
                model: Some("gpt-5.6-sol".to_string()),
            })
        );
        assert!(
            !SessionController::session_type_supports_dynamic_principals(&SessionType::Fusion {
                variants: vec![]
            })
        );
        assert!(
            !SessionController::session_type_supports_dynamic_principals(&SessionType::Debate {
                variants: vec![]
            })
        );
    }

    #[test]
    fn solo_allows_only_prince_owned_fixers_during_remediation() {
        let mut session = waiting_worker_session("solo-fixer", Path::new("/repo"), 1);
        session.session_type = SessionType::Solo {
            cli: "codex".to_string(),
            model: Some("gpt-5.6-sol".to_string()),
        };
        let fixer = WorkerRole::new("prince-fixer", "Prince Fixer", "codex");
        let prince_id = "solo-fixer-prince";

        session.state = SessionState::Running;
        assert!(!SessionController::session_allows_dynamic_principal(
            &session,
            &fixer,
            Some(prince_id),
        ));

        session.state = SessionState::PrinceRemediation;
        assert!(SessionController::session_allows_dynamic_principal(
            &session,
            &fixer,
            Some(prince_id),
        ));
        assert!(!SessionController::session_allows_dynamic_principal(
            &session,
            &fixer,
            Some("another-session-prince"),
        ));
    }

    #[test]
    fn fusion_and_debate_planning_continuations_reach_type_dispatch() {
        let temp = tempfile::tempdir().expect("temp project");
        let controller = test_controller();
        let cases = [
            (
                "planning-fusion",
                SessionType::Fusion {
                    variants: vec!["Variant A".to_string()],
                },
                "Failed to read pending fusion config",
            ),
            (
                "planning-debate",
                SessionType::Debate {
                    variants: vec!["Debater A".to_string()],
                },
                "Failed to read pending debate config",
            ),
        ];

        for (session_id, session_type, expected_dispatch_error) in cases {
            let mut session = waiting_worker_session(session_id, temp.path(), 1);
            session.session_type = session_type;
            session.state = SessionState::Planning;
            controller.insert_test_session(session);

            let error = controller
                .continue_after_planning(session_id)
                .expect_err("missing pending config should stop after type dispatch");
            assert!(
                error.contains(expected_dispatch_error),
                "unexpected continuation error: {error}"
            );
        }
    }

    #[test]
    fn session_state_serialization() {
        let state = SessionState::SpawningWorker(3);
        let json = serde_json::to_string(&state).expect("serialize SessionState");
        assert!(json.contains("SpawningWorker"));
    }

    // ---- #125 run journal: resume skip + ledger recovery ----

    fn controller_with_journal() -> (SessionController, crate::storage::RunJournalStore) {
        let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
        let mut controller = SessionController::new(pty_manager);
        let db = Arc::new(crate::storage::ApplicationStateDb::open_in_memory().unwrap());
        let store = crate::storage::RunJournalStore::new(db);
        store.ensure_schema().unwrap();
        controller.set_run_journal(store.clone());
        (controller, store)
    }

    #[test]
    fn test_resume_skips_completed_write_step() {
        use crate::domain::run_journal::{StepKind, StepStatus};
        let (controller, store) = controller_with_journal();
        let run_id = "resume-skip";

        // A worker-spawn write-step that completed in a prior run.
        let step_id = store
            .record_step_started(run_id, StepKind::WorkerSpawn, 1, None)
            .unwrap();
        store
            .record_step_finished(run_id, &step_id, StepStatus::Completed)
            .unwrap();

        // The resume guard sees it as completed (so the destructive spawn is NOT re-run).
        assert!(controller.is_write_step_completed(run_id, StepKind::WorkerSpawn, 1));

        // Building the resume report marks it Skipped and surfaces it.
        let report = controller.build_resume_report(run_id, Path::new("."));
        assert_eq!(
            report.skipped.len(),
            1,
            "completed write-step is reported skipped"
        );
        assert_eq!(report.skipped[0].step_id, step_id);
        assert_eq!(report.skipped[0].status, StepStatus::Skipped);

        // The journal row is now persisted as Skipped.
        let journal = store.read_journal(run_id).unwrap();
        assert_eq!(journal[0].status, StepStatus::Skipped);
    }

    #[test]
    fn test_resume_classifies_interrupted_step() {
        use crate::domain::run_journal::{StepKind, StepStatus};
        let (controller, store) = controller_with_journal();
        let run_id = "resume-interrupted";

        // A worker spawn that started but never finished (no ledger): Interrupted.
        store
            .record_step_started(run_id, StepKind::WorkerSpawn, 1, None)
            .unwrap();

        let report = controller.build_resume_report(run_id, Path::new("."));
        assert_eq!(report.interrupted.len(), 1);
        assert_eq!(report.interrupted[0].kind, StepKind::WorkerSpawn);
        assert!(report.skipped.is_empty());
        // Not completed, so the guard would allow a re-spawn.
        assert!(!controller.is_write_step_completed(run_id, StepKind::WorkerSpawn, 1));
        let _ = StepStatus::Interrupted;
    }

    #[test]
    fn test_resume_recovers_unconfirmed_commit_in_temp_repo() {
        use crate::domain::run_journal::{Confidence, StepKind};
        let (controller, store) = controller_with_journal();
        let run_id = "resume-recover";

        // Build a real temp git repo with one commit so the SHA verification succeeds.
        let repo = TempDir::new().unwrap();
        let repo_path = repo.path().to_path_buf();
        SessionController::run_git_in_dir(&repo_path, &["init", "-q"]).unwrap();
        SessionController::run_git_in_dir(&repo_path, &["config", "user.email", "t@t.dev"])
            .unwrap();
        SessionController::run_git_in_dir(&repo_path, &["config", "user.name", "tester"]).unwrap();
        std::fs::write(repo_path.join("a.txt"), "hi").unwrap();
        SessionController::run_git_in_dir(&repo_path, &["add", "."]).unwrap();
        SessionController::run_git_in_dir(&repo_path, &["commit", "-q", "-m", "init"]).unwrap();
        let sha = current_head(&repo_path).unwrap();

        // Simulate a crash between commit and confirmation: Started step + unconfirmed
        // ledger row carrying the real SHA.
        let step_id = store
            .record_step_started(run_id, StepKind::GitCommit, 1, None)
            .unwrap();
        store
            .record_ledger(
                run_id,
                &step_id,
                "git_commit",
                Some(&sha),
                Confidence::Uncertain,
            )
            .unwrap();

        // Resume verifies the SHA exists -> ledger confirmed with High confidence.
        let report = controller.build_resume_report(run_id, &repo_path);
        assert!(
            report.uncertain.is_empty(),
            "verified commit is not uncertain"
        );
        let ledger = store
            .read_ledger_for_step(run_id, &step_id)
            .unwrap()
            .unwrap();
        assert!(ledger.confirmed);
        assert_eq!(ledger.confidence, Confidence::High);
    }

    #[test]
    fn test_resume_report_no_journal_store_is_empty() {
        // Controller without a journal store: build_resume_report is a cheap empty no-op.
        let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
        let controller = SessionController::new(pty_manager);
        let report = controller.build_resume_report("x", Path::new("."));
        assert!(report.is_empty());
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
            "/repo/execution",
        );

        assert!(prompt.contains("Accessibility Tester"));
        assert!(prompt.contains("axe-core"));
        assert!(prompt.contains("/repo/execution"));
        assert!(!prompt.contains("UI Tester"));
        assert!(prompt.contains("## Completion Protocol (MANDATORY)"));
        assert!(
            prompt.contains(".hive-manager/session-123/tasks/qa-worker-1-task.md")
        );
        assert!(prompt.contains(r#""agent_id":"session-123-qa-worker-1""#));
        assert!(prompt.contains(r#""status":"completed""#));
        assert!(!prompt.contains("{{qa_worker_completed_heartbeat}}"));
    }

    #[test]
    fn every_qa_worker_prompt_has_a_ready_completed_heartbeat() {
        for specialization in ["ui", "api", "a11y", "adversarial"] {
            let prompt = SessionController::build_qa_worker_prompt(
                "session-qa",
                3,
                specialization,
                &AgentConfig::default(),
                &AuthStrategy::default(),
                "/repo/execution",
            );

            let completion = extract_markdown_section(
                &prompt,
                "## Completion Protocol (MANDATORY)",
            );
            assert!(
                completion.contains(r#""agent_id":"session-qa-qa-worker-3""#),
                "missing exact agent ID for {specialization}"
            );
            assert!(
                completion.contains(r#""status":"completed""#),
                "missing completed status for {specialization}"
            );
            assert!(completion.contains("curl -fsS -X POST"));
        }
    }

    #[test]
    fn evaluator_enabled_solo_prompt_emits_and_waits_for_verification_handoffs() {
        let prompt = SessionController::build_solo_evaluator_prompt(
            "solo-123",
            Path::new("/repo"),
            "/repo/.hive-manager/worktrees/solo-123/worker-1",
            Some("Fix the bounded bug"),
        );

        assert!(prompt.contains("Fix the bounded bug"));
        assert!(prompt.contains("milestone-ready.json"));
        assert!(prompt.contains("qa-verdict.json"));
        assert!(prompt.contains("prince-verdict.json"));
        assert!(prompt.contains("commit the completed Solo implementation"));
        assert!(prompt.contains("solo-123-worker-1"));
        assert!(prompt.contains(SessionController::qa_blocked_verdict_grep_pattern()));
        let blocked_guard = prompt.find("QA is BLOCKED").expect("BLOCKED guard");
        let prince_wait = prompt
            .find("while [ ! -f \"/repo/.hive-manager/solo-123/peer/prince-verdict.json\" ]")
            .expect("Prince wait loop");
        assert!(blocked_guard < prince_wait);
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
    fn session_worker_task_path_follows_shared_isolated_and_research_workspaces() {
        let project = Path::new("/repo");
        let mut session = waiting_worker_session("session-123", project, 1);

        let isolated = SessionController::task_file_path_for_session_worker(&session, 2)
            .expect("isolated task path");
        assert_eq!(
            isolated,
            PathBuf::from(
                "/repo/.hive-manager/worktrees/session-123/worker-2/.hive-manager/tasks/worker-2-task.md"
            )
        );

        session.execution_policy.workspace_strategy = WorkspaceStrategy::SharedCell;
        session.worktree_path =
            Some("/repo/.hive-manager/worktrees/session-123/primary".to_string());
        let shared = SessionController::task_file_path_for_session_worker(&session, 2)
            .expect("shared task path");
        assert_eq!(
            shared,
            PathBuf::from(
                "/repo/.hive-manager/worktrees/session-123/primary/.hive-manager/tasks/worker-2-task.md"
            )
        );

        session.no_git = true;
        session.worktree_path = None;
        let research = SessionController::task_file_path_for_session_worker(&session, 2)
            .expect("research task path");
        assert_eq!(
            research,
            PathBuf::from("/repo/.hive-manager/session-123/tasks/worker-2-task.md")
        );
    }

    #[test]
    fn legacy_no_git_restore_normalizes_research_workspace_and_prompt_path() {
        let temp = tempfile::tempdir().expect("temp project");
        let controller = test_controller();
        let mut source = waiting_worker_session("legacy-research", temp.path(), 1);
        source.no_git = true;
        source.execution_policy = HiveExecutionPolicy::default();

        let persisted = SessionController::session_to_persisted_snapshot(&source);
        let mut legacy_json = serde_json::to_value(persisted).expect("persisted session JSON");
        legacy_json
            .as_object_mut()
            .expect("persisted object")
            .remove("execution_policy");
        let legacy: crate::storage::PersistedSession =
            serde_json::from_value(legacy_json).expect("legacy session");
        assert_eq!(
            legacy.execution_policy.workspace_strategy,
            WorkspaceStrategy::IsolatedCell
        );

        let restored = controller
            .session_from_persisted(&legacy)
            .expect("restore legacy Research session");
        assert_eq!(
            restored.execution_policy.workspace_strategy,
            WorkspaceStrategy::None
        );

        let task_path = SessionController::task_file_path_for_session_worker(&restored, 2)
            .expect("Research task path");
        let prompt = SessionController::build_worker_prompt(
            2,
            &AgentConfig {
                role: Some(WorkerRole::new("researcher", "Researcher", "claude")),
                ..AgentConfig::default()
            },
            "legacy-research-queen",
            &restored.id,
            &restored.project_path,
            &restored.project_path,
            &restored.execution_policy,
        );
        assert!(prompt.contains(&SessionController::prompt_path(&task_path)));
    }

    /// Every `CliBehavior` must name a CLI here. Adding a variant breaks this match at
    /// compile time, which forces the new behavior into the coverage test below instead of
    /// letting it silently ship without a heartbeat instruction (#141 defect A).
    fn representative_cli_for(behavior: &CliBehavior) -> &'static str {
        match behavior {
            CliBehavior::ActionProne => "claude",
            CliBehavior::InstructionFollowing => "qwen",
            CliBehavior::ExplicitPolling => "codex",
            CliBehavior::Interactive => "droid",
        }
    }

    /// Enumerate every `CliBehavior` EXHAUSTIVELY BY CONSTRUCTION. A hand-written `vec![]`
    /// would compile unchanged when a variant is added, so the new behavior would never be
    /// rendered by the coverage test below and its polling arm would ship uncovered — the
    /// exhaustive match in `representative_cli_for` forces a variant to NAME a CLI, not to
    /// enter the iterated collection. The successor chain below closes that gap: its match
    /// is compiler-checked, so a fifth variant is a build error here (E0004) rather than a
    /// silent hole. A `const ALL: [CliBehavior; N]` would NOT work — array arity is not tied
    /// to variant count.
    fn all_cli_behaviors() -> Vec<CliBehavior> {
        let mut all = Vec::new();
        let mut next = Some(CliBehavior::ActionProne);
        while let Some(behavior) = next {
            next = match behavior {
                CliBehavior::ActionProne => Some(CliBehavior::InstructionFollowing),
                CliBehavior::InstructionFollowing => Some(CliBehavior::ExplicitPolling),
                CliBehavior::ExplicitPolling => Some(CliBehavior::Interactive),
                CliBehavior::Interactive => None,
            };
            all.push(behavior);
        }
        all
    }

    fn worker_prompt_for_cli(cli: &str) -> String {
        let temp = tempfile::tempdir().expect("temp project");
        SessionController::build_worker_prompt(
            1,
            &AgentConfig {
                cli: cli.to_string(),
                role: Some(WorkerRole::new("backend", "Backend", cli)),
                ..AgentConfig::default()
            },
            "session-141-queen",
            "session-141",
            temp.path(),
            temp.path(),
            &HiveExecutionPolicy::default(),
        )
    }

    /// #141 defect A: `heartbeat_line` was interpolated only into the `ExplicitPolling` arm,
    /// so workers on the other three behaviors were handed no heartbeat instruction at all
    /// and went silent straight into `reclaim_stuck`. Asserted against the RENDERED prompt,
    /// not a template constant.
    #[test]
    fn worker_prompt_instructs_heartbeat_cadence_for_every_cli_behavior() {
        let cadence = heartbeat_cadence_label();

        for behavior in all_cli_behaviors() {
            let cli = representative_cli_for(&behavior);
            assert_eq!(
                CliRegistry::get_behavior(cli),
                behavior,
                "representative CLI {cli} no longer maps to the behavior it stands in for"
            );

            let prompt = worker_prompt_for_cli(cli);
            assert!(
                prompt.contains(&cadence),
                "{behavior:?} ({cli}) prompt carries no heartbeat cadence instruction"
            );
            assert!(
                prompt.contains(r#""status":"idle""#),
                "{behavior:?} ({cli}) prompt drops the activation-wait heartbeat command"
            );
            assert!(
                prompt.contains(r#""status":"working""#),
                "{behavior:?} ({cli}) prompt drops the active-work heartbeat command"
            );
            assert!(
                prompt.contains("/api/sessions/session-141/heartbeat"),
                "{behavior:?} ({cli}) prompt has no heartbeat endpoint to call"
            );

            // The active-work heartbeat is the one that keeps a `running` queue row fresh,
            // so its own instruction must state the cadence rather than leaving the worker
            // to infer it from the activation-wait section.
            let active_line = prompt
                .lines()
                .find(|line| line.starts_with("Heartbeat while active"))
                .unwrap_or_else(|| {
                    panic!("{behavior:?} ({cli}) prompt has no active-work heartbeat instruction")
                });
            assert!(
                active_line.contains(&cadence),
                "{behavior:?} ({cli}) active-work heartbeat states no cadence: {active_line}"
            );

            // #155/#156 on the status axis: the completed heartbeat is built outside the
            // CliBehavior match, so it already reaches every behavior. Guard that — gating it
            // the way `heartbeat_line` was gated would resurrect the same defect.
            assert!(
                prompt.contains("Completion Protocol (MANDATORY)")
                    && prompt.contains(r#""status":"completed""#),
                "{behavior:?} ({cli}) prompt drops the completed heartbeat instruction"
            );

            // The polling section must stand on its own: a worker parked in STANDBY never
            // reaches the active-work section.
            let polling = super::get_polling_instructions(
                cli,
                "worker-1-task.md",
                Some("backend"),
                Some("HEARTBEAT_COMMAND"),
            );
            assert!(
                polling.contains(&cadence) && polling.contains("HEARTBEAT_COMMAND"),
                "{behavior:?} ({cli}) polling section omits the heartbeat instruction: {polling}"
            );
        }
    }

    /// The behavior enum is only reachable in production through real CLI names, so cover
    /// the allowlist too: a CLI whose behavior mapping changes must not lose its cadence.
    #[test]
    fn every_allowlisted_cli_receives_the_heartbeat_cadence() {
        let cadence = heartbeat_cadence_label();

        for cli in crate::adapters::VALID_CLIS {
            let prompt = worker_prompt_for_cli(cli);
            assert!(
                prompt.contains(&cadence),
                "cli {cli} prompt carries no heartbeat cadence instruction"
            );
            assert!(
                prompt.contains(r#""status":"idle""#),
                "cli {cli} prompt drops the activation-wait heartbeat command"
            );

            // Assert the POLLING SECTION on its own. The whole-prompt check above is ALSO
            // satisfied by the unconditional "Heartbeat while active" line, which lives
            // outside the `CliBehavior` match — so on its own it can never see a behavior arm
            // that drops the cadence. This is also the assertion that covers the
            // `get_behavior` catch-all: a CLI added to VALID_CLIS with no mapping arm
            // silently inherits ActionProne, and only a VALID_CLIS-driven loop notices.
            let polling = super::get_polling_instructions(
                cli,
                "worker-1-task.md",
                Some("backend"),
                Some("HEARTBEAT_COMMAND"),
            );
            assert!(
                polling.contains(&cadence) && polling.contains("HEARTBEAT_COMMAND"),
                "cli {cli} polling section omits the heartbeat instruction: {polling}"
            );
        }
    }

    /// The `ExplicitPolling` loop is one of several cadences enforced by code rather than by
    /// the model obeying prose — the evaluator and prince poll loops in `templates/` are the
    /// others, and they are guarded by `heartbeat_loops_sleep_within_the_instructed_cadence`.
    /// Wherever a sleep sits between two heartbeats, it must satisfy the instruction it
    /// ships with.
    #[test]
    fn activation_poll_loop_obeys_the_instructed_cadence() {
        assert!(
            ACTIVATION_POLL_INTERVAL.as_secs() <= HEARTBEAT_MAX_INTERVAL_SECS,
            "polling loop sleeps {}s, longer than the instructed {HEARTBEAT_MAX_INTERVAL_SECS}s maximum",
            ACTIVATION_POLL_INTERVAL.as_secs()
        );
        assert!(
            ACTIVATION_POLL_INTERVAL.as_secs() >= HEARTBEAT_MIN_INTERVAL_SECS,
            "polling loop sleeps {}s, shorter than the instructed {HEARTBEAT_MIN_INTERVAL_SECS}s minimum",
            ACTIVATION_POLL_INTERVAL.as_secs()
        );
    }

    #[test]
    fn coding_task_file_defers_to_workspace_contract_for_commit_authority() {
        let temp = tempfile::tempdir().expect("temp dir");
        let task = SessionController::write_task_file(temp.path(), 1, None, false)
            .expect("write task file");
        let body = std::fs::read_to_string(task).expect("read task file");

        assert!(body.contains("Follow the launch prompt's Workspace Contract"));
        assert!(!body.contains("Do NOT push or commit"));
    }

    #[test]
    fn worker_prompt_file_uses_worktree_local_hive_manager_dir() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let worktree_path = temp_dir.path().join("worker-2");
        std::fs::create_dir_all(&worktree_path).expect("create worktree");

        let path = SessionController::write_worker_prompt_file(
            &worktree_path,
            2,
            "worker-2-prompt.md",
            "Prompt body",
        )
        .expect("write worker prompt");

        assert_eq!(
            path,
            worktree_path
                .join(".hive-manager")
                .join("prompts")
                .join("worker-2-prompt.md")
        );
        assert_eq!(
            std::fs::read_to_string(path).expect("read worker prompt"),
            "Prompt body"
        );
    }

    #[test]
    fn add_prompt_to_args_preserves_worktree_scoped_absolute_prompt_path() {
        let prompt_path = r"D:\repo\.hive-manager\worktrees\session-123\worker-2\.hive-manager\prompts\worker-2-prompt.md";
        let expected_prompt = format!("Read {} and execute.", prompt_path);
        let expected_cursor_prompt =
            "Read /mnt/d/repo/.hive-manager/worktrees/session-123/worker-2/.hive-manager/prompts/worker-2-prompt.md and execute."
                .to_string();

        for cli in ["claude", "codex", "droid"] {
            let mut args = Vec::new();
            SessionController::add_prompt_to_args(cli, &mut args, prompt_path);
            assert_eq!(args, vec![expected_prompt.clone()], "cli {cli}");
        }

        let mut args = Vec::new();
        SessionController::add_prompt_to_args("cursor", &mut args, prompt_path);
        assert_eq!(args, vec![expected_cursor_prompt.clone()], "cli cursor");

        let mut args = Vec::new();
        SessionController::add_prompt_to_args("wsl", &mut args, prompt_path);
        assert_eq!(args, vec![expected_cursor_prompt], "cli wsl");

        let mut args = Vec::new();
        SessionController::add_prompt_to_args("qwen", &mut args, prompt_path);
        assert_eq!(
            args,
            vec!["-i".to_string(), expected_prompt.clone()],
            "cli qwen"
        );
    }

    #[test]
    fn to_wsl_path_converts_windows_drive_paths() {
        assert_eq!(
            SessionController::to_wsl_path(r"D:\foo\bar"),
            "/mnt/d/foo/bar"
        );
        assert_eq!(
            SessionController::to_wsl_path("D:/foo/bar"),
            "/mnt/d/foo/bar"
        );
        assert_eq!(
            SessionController::to_wsl_path(r"C:\Users\x"),
            "/mnt/c/Users/x"
        );
        assert_eq!(SessionController::to_wsl_path("/tmp/x"), "/tmp/x");
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

        let worker_tool_path = temp_dir
            .path()
            .join(".hive-manager")
            .join("session-123")
            .join("tools")
            .join("spawn-worker.md");
        let worker_content =
            std::fs::read_to_string(worker_tool_path).expect("read worker tool doc");
        assert!(worker_content.contains("gpt-5.6-sol for Codex or fable/opus for Claude"));
        assert!(worker_content.contains("Omit to inherit the session principal CLI (`claude`)"));
        assert!(worker_content.contains("| flags | string[] | No |"));
        assert!(worker_content.contains("Omit to inherit principal flags; send `[]` to clear them"));
        assert!(!worker_content.contains(r#"{\"role_type\": \"backend\", \"cli\""#));
        assert!(worker_content.contains("absolute `task_file` returned by the API"));
        assert!(worker_content.contains("Shared-cell Hive"));
        assert!(worker_content.contains("Isolated-cell Hive"));
        assert!(worker_content.contains("Research/no-worktree Hive"));
        assert!(!worker_content.contains("inside that worker's worktree"));

        let status_tool_path = temp_dir
            .path()
            .join(".hive-manager")
            .join("session-123")
            .join("tools")
            .join("mark-worker-status.md");
        let status_content =
            std::fs::read_to_string(status_tool_path).expect("read status tool doc");
        assert!(status_content.contains("Queen MUST use this tool"));
        assert!(status_content.contains(r#""agent_id":"<exact-agent-id>""#));
        assert!(status_content.contains(r#""status":"completed""#));
        assert!(status_content.contains("shortened ID such as `worker-N`"));
        assert!(status_content.contains("placeholder fails validation"));
    }

    fn shared_meta_harness_policy() -> HiveExecutionPolicy {
        HiveExecutionPolicy {
            launch_kind: crate::domain::HiveLaunchKind::Hive,
            workspace_strategy: crate::domain::WorkspaceStrategy::SharedCell,
            queen_delegation: crate::domain::DelegationPolicy {
                mode: crate::domain::NativeDelegationMode::Auto,
                max_children: Some(3),
                max_depth: Some(2),
            },
            principal_delegation: crate::domain::DelegationPolicy {
                mode: crate::domain::NativeDelegationMode::Encouraged,
                max_children: Some(4),
                max_depth: Some(2),
            },
        }
    }

    fn codex_principal() -> AgentConfig {
        AgentConfig {
            cli: "codex".to_string(),
            model: Some("gpt-5.6-sol".to_string()),
            flags: vec![
                "--config".to_string(),
                "model_reasoning_effort=\"high\"".to_string(),
            ],
            role: Some(WorkerRole::new("backend", "Backend Principal", "codex")),
            ..AgentConfig::default()
        }
    }

    #[test]
    fn live_master_planner_uses_capability_policy_and_coherent_workstreams() {
        let policy = shared_meta_harness_policy();
        let planner = AgentConfig {
            cli: "claude".to_string(),
            model: Some("fable".to_string()),
            ..AgentConfig::default()
        };
        let prompt = SessionController::build_master_planner_prompt(
            "session-modern",
            "Implement the operator objective",
            &planner,
            &[codex_principal()],
            &policy,
            Path::new("/repo"),
            Path::new("/repo/.hive-manager/worktrees/session-modern/primary"),
        );

        assert!(prompt.contains("Harness: `claude`"));
        assert!(prompt.contains("Model: `fable`"));
        assert!(
            prompt.contains("Runtime CWD: `/repo/.hive-manager/worktrees/session-modern/primary`")
        );
        assert!(prompt.contains("`codex` | `gpt-5.6-sol`"));
        assert!(prompt.contains(r#"["--config","model_reasoning_effort=\"high\""]"#));
        assert!(prompt.contains("not a required task count"));
        assert!(prompt.contains(
            "native read-only scouts only when the Capability Card says delegation is authorized"
        ));
        assert!(!prompt.contains("ALL 3"));
        assert!(!prompt.contains("GPT-5.5"));
        assert!(!prompt.contains("codex exec"));
        assert!(!prompt.contains("one per worker"));
    }

    #[test]
    fn live_shared_queen_prompt_reports_actual_roster_workspace_and_authority() {
        let policy = shared_meta_harness_policy();
        let queen = AgentConfig {
            cli: "claude".to_string(),
            model: Some("opus".to_string()),
            ..AgentConfig::default()
        };
        let prompt = SessionController::build_queen_master_prompt(
            &queen,
            Path::new("/repo"),
            Path::new("/repo/.hive-manager/worktrees/session-modern/primary"),
            "session-modern",
            &[codex_principal()],
            Some("Implement the operator objective"),
            true,
            false,
            &policy,
        );

        assert!(prompt.contains("Harness: `claude`"));
        assert!(prompt.contains("Model: `opus`"));
        assert!(
            prompt.contains("Runtime CWD: /repo/.hive-manager/worktrees/session-modern/primary")
        );
        assert!(prompt.contains("codex | gpt-5.6-sol"));
        assert!(prompt.contains(r#"["--config","model_reasoning_effort=\"high\""]"#));
        assert!(prompt.contains("supported; encouraged (authorized)"));
        assert!(prompt.contains("do not drop effort or reasoning settings"));
        assert!(prompt.contains("Shared Cell Integration"));
        assert!(prompt.contains("Principals do not commit"));
        assert!(prompt.contains("backend-created hive/session-modern/primary branch"));
        assert!(prompt.contains("Managed principals are visible Hive agents"));
        assert!(prompt.contains("mark-worker-status.md"));
        assert!(prompt.contains("UI completion checkoff and stall monitor depend on it"));
        assert!(!prompt.contains("full Claude Code capabilities"));
        assert!(!prompt.contains("Claude Code Tools"));
        assert!(!prompt.contains("git checkout -b"));
    }

    #[test]
    fn live_worker_prompt_uses_actual_codex_capabilities_and_topology_git_contract() {
        let shared_policy = shared_meta_harness_policy();
        let principal = codex_principal();
        let shared_prompt = SessionController::build_worker_prompt(
            1,
            &principal,
            "session-modern-queen",
            "session-modern",
            Path::new("/repo"),
            Path::new("/repo/.hive-manager/worktrees/session-modern/primary"),
            &shared_policy,
        );

        assert!(shared_prompt.contains("Harness: `codex`"));
        assert!(shared_prompt.contains("Model: `gpt-5.6-sol`"));
        assert!(
            shared_prompt.contains(r#"Flags: `["--config","model_reasoning_effort=\"high\""]`"#)
        );
        assert!(shared_prompt.contains("Native delegation authorized: yes"));
        assert!(shared_prompt
            .contains("Runtime CWD: /repo/.hive-manager/worktrees/session-modern/primary"));
        assert!(shared_prompt.contains("leave the reviewed changes uncommitted for the Queen"));
        assert!(shared_prompt.contains("Learnings Protocol (MANDATORY)"));
        assert!(shared_prompt.contains("Completion Protocol (MANDATORY)"));
        assert!(shared_prompt.contains(r#""agent_id":"session-modern-worker-1""#));
        assert!(shared_prompt.contains(r#""status":"completed""#));
        assert!(shared_prompt.contains("Begin only when Status is ACTIVE"));
        assert!(shared_prompt.contains("Polling Protocol (MANDATORY)"));
        assert!(shared_prompt.contains("while true; do"));
        assert!(!shared_prompt.contains("full access to Claude Code tools"));

        let isolated_policy = HiveExecutionPolicy {
            launch_kind: crate::domain::HiveLaunchKind::Hive,
            workspace_strategy: crate::domain::WorkspaceStrategy::IsolatedCell,
            ..shared_policy.clone()
        };
        let isolated_prompt = SessionController::build_worker_prompt(
            1,
            &principal,
            "session-modern-queen",
            "session-modern",
            Path::new("/repo"),
            Path::new("/repo/.hive-manager/worktrees/session-modern/worker-1"),
            &isolated_policy,
        );
        assert!(isolated_prompt.contains("Commit the completed assignment"));
        assert!(isolated_prompt
            .contains("commit SHA when applicable plus focused validation evidence"));
        assert!(isolated_prompt.contains("Do not create or switch branches"));

        let no_workspace_policy = HiveExecutionPolicy {
            workspace_strategy: WorkspaceStrategy::None,
            ..shared_policy
        };
        let no_workspace_prompt = SessionController::build_worker_prompt(
            1,
            &principal,
            "session-modern-queen",
            "session-modern",
            Path::new("/repo"),
            Path::new("/repo"),
            &no_workspace_policy,
        );
        assert!(no_workspace_prompt
            .contains("/repo/.hive-manager/session-modern/tasks/worker-1-task.md"));
        assert!(no_workspace_prompt.contains("Do not mutate git without explicit operator"));
        assert!(no_workspace_prompt.contains("Completion Protocol (MANDATORY)"));
        assert!(no_workspace_prompt.contains(r#""status":"completed""#));
    }

    #[test]
    fn evaluator_prompt_uses_session_default_cli_and_model() {
        let prompt = SessionController::build_evaluator_prompt(
            "session-123",
            &AgentConfig {
                cli: "codex".to_string(),
                model: Some("gpt-5.5".to_string()),
                ..AgentConfig::default()
            },
            &[],
            0,
            "/repo/execution",
            false,
        );

        let required_protocol = extract_markdown_section(&prompt, "## Required Protocol");
        assert!(
            required_protocol.starts_with(&SessionController::evaluator_required_protocol(
                "session-123"
            ))
        );
        assert!(prompt.contains(".hive-manager/session-123/peer/qa-verdict.json"));
        assert!(prompt.contains("This session uses CLI: codex, Model: gpt-5.5."));
        assert!(prompt.contains(r#""specialization":"api""#));
        assert!(prompt.contains(r#""cli":"codex""#));
        assert!(prompt.contains(r#""model":"gpt-5.5""#));
        assert!(prompt.contains("/repo/execution"));
        assert!(!prompt.contains(r#""cli": "claude""#));
    }

    #[test]
    fn evaluator_prompt_uses_configured_qa_workers() {
        let prompt = SessionController::build_evaluator_prompt(
            "session-123",
            &AgentConfig {
                cli: "claude".to_string(),
                model: Some("opus".to_string()),
                ..AgentConfig::default()
            },
            &[QaWorkerConfig {
                specialization: "ui".to_string(),
                cli: "droid".to_string(),
                model: Some("glm-5.1".to_string()),
                label: Some("Visual QA".to_string()),
                flags: None,
            }],
            0,
            "/repo/execution",
            false,
        );

        let required_protocol = extract_markdown_section(&prompt, "## Required Protocol");
        assert!(
            required_protocol.starts_with(&SessionController::evaluator_required_protocol(
                "session-123"
            ))
        );
        assert!(
            prompt.contains("configured QA workers below (1 total) before you grade any criterion")
        );
        assert!(prompt.contains(r#""specialization":"ui""#));
        assert!(prompt.contains(r#""cli":"droid""#));
        assert!(prompt.contains(r#""model":"glm-5.1""#));
        assert!(
            prompt.contains("You MUST spawn all 1 QA workers one at a time in this exact order:")
        );
        assert!(!prompt.contains(r#""specialization": "api", "cli": "claude""#));
    }

    #[test]
    fn research_worker_surfaces_are_read_only() {
        let session_id = "session-research-readonly";
        let temp = tempfile::tempdir().expect("temp project");
        let worktree_path = temp.path().join("worker-1");
        std::fs::create_dir_all(&worktree_path).expect("create worker directory");

        let cfg = AgentConfig {
            role: Some(WorkerRole::new("researcher", "Researcher", "claude")),
            ..AgentConfig::default()
        };

        // Worker prompt: read-only framing, no EXECUTOR, no learnings POST.
        let research_policy = HiveExecutionPolicy {
            workspace_strategy: crate::domain::WorkspaceStrategy::None,
            ..HiveExecutionPolicy::default()
        };
        let prompt = SessionController::build_worker_prompt(
            1,
            &cfg,
            "queen",
            session_id,
            temp.path(),
            &worktree_path,
            &research_policy,
        );
        assert!(prompt.contains("RESEARCHER"));
        assert!(prompt.contains("Read-Only"));
        let expected_task_path = SessionController::prompt_path(
            &SessionController::session_task_file_path(temp.path(), session_id, 1),
        );
        assert!(prompt.contains(&expected_task_path));
        assert!(!prompt.contains("## Your Role: EXECUTOR"));
        assert!(!prompt.contains("Learnings Protocol (MANDATORY)"));
        assert!(prompt.contains("Completion Protocol (MANDATORY)"));
        assert!(prompt.contains(r#""agent_id":"session-research-readonly-worker-1""#));
        assert!(prompt.contains(r#""status":"completed""#));
        assert!(prompt.contains("repository and git state remain unchanged"));
        assert!(prompt.contains("only permitted filesystem write"));
        assert!(prompt.contains("exact Hive control-plane task file"));

        // Task file (read_only=true): read-only role constraints, no EXECUTOR.
        let task_path = SessionController::write_task_file_with_status(
            &worktree_path,
            1,
            Some("investigate the sub-question"),
            Some("ACTIVE"),
            true,
        )
        .expect("write research task file");
        let task = std::fs::read_to_string(&task_path).unwrap();
        assert!(task.contains("RESEARCHER (READ-ONLY)"));
        assert!(!task.contains("EXECUTOR"));
        assert!(task.contains("only permitted filesystem write"));
    }

    #[test]
    fn scope_block_is_identical_across_worker_and_task_surfaces() {
        let session_id = "session-scope-equality";
        let temp = tempfile::tempdir().expect("temp project");
        let worktree_path = temp.path().join("worker-1");
        std::fs::create_dir_all(&worktree_path).expect("create worktree");

        let fusion_prompt = SessionController::build_fusion_worker_prompt(
            session_id,
            1,
            "Variant 1",
            "feat/test",
            worktree_path.to_str().expect("utf8 worktree path"),
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
            Path::new("."),
            &worktree_path,
            &HiveExecutionPolicy::default(),
        );
        let task_file_path = SessionController::write_task_file_with_status(
            &worktree_path,
            1,
            Some("Test task"),
            Some("ACTIVE"),
            false,
        )
        .expect("write task file");
        let task_file = std::fs::read_to_string(&task_file_path).expect("read task file");

        let expected = SessionController::scope_block(".");
        assert_eq!(
            extract_markdown_section(&worker_prompt, "## Scope"),
            expected
        );
        assert_eq!(
            extract_markdown_section(&fusion_prompt, "## Scope"),
            expected
        );
        assert_eq!(extract_markdown_section(&task_file, "## Scope"), expected);
        let fusion_completion =
            extract_markdown_section(&fusion_prompt, "## Completion Protocol (MANDATORY)");
        assert!(fusion_completion.contains(r#""agent_id":"session-scope-equality-fusion-1""#));
        assert!(fusion_completion.contains(r#""status":"completed""#));
        assert!(fusion_completion.contains("curl -fsS -X POST"));
    }

    #[test]
    fn required_protocol_block_is_identical_across_queens() {
        let session_root = SessionController::session_root_path(Path::new("/repo"), "session-123");
        let queen_master_prompt = SessionController::build_queen_master_prompt(
            &AgentConfig {
                cli: "claude".to_string(),
                model: Some("opus".to_string()),
                ..AgentConfig::default()
            },
            Path::new("/repo"),
            Path::new("/repo/.hive-manager/worktrees/session-123/queen"),
            "session-123",
            &[],
            None,
            false,
            true,
            &HiveExecutionPolicy::default(),
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

        assert!(
            extract_markdown_section(&queen_master_prompt, "## Required Protocol")
                .starts_with(&expected)
        );
        assert!(
            extract_markdown_section(&fusion_queen_prompt, "## Required Protocol")
                .starts_with(&expected)
        );
        assert!(
            extract_markdown_section(&swarm_queen_prompt, "## Required Protocol")
                .starts_with(&expected)
        );
        assert!(expected.contains("mark-worker-status.md"));
        assert!(expected.contains("UI completion checkoff and stall monitor depend on it"));
    }

    #[test]
    fn fusion_queen_roster_exposes_exact_agent_ids() {
        let variants = vec![FusionVariantMetadata {
            index: 1,
            name: "Safe Variant".to_string(),
            slug: "safe-variant".to_string(),
            branch: "fusion/session-123/variant-1".to_string(),
            worktree_path: "/repo/.hive-manager/worktrees/session-123/fusion-1".to_string(),
            task_file: "/repo/.hive-manager/session-123/tasks/fusion-1.md".to_string(),
            agent_id: "session-123-fusion-1".to_string(),
        }];
        let prompt = SessionController::build_fusion_queen_prompt(
            "claude",
            Path::new("/repo"),
            "session-123",
            &variants,
            "Test task",
            false,
        );

        assert!(prompt.contains("| # | Name | Agent ID | Branch | Worktree |"));
        assert!(prompt.contains("| 1 | Safe Variant | `session-123-fusion-1` |"));
    }

    #[test]
    fn evaluator_required_protocol_omits_queen_only_handoff_and_wait_text() {
        let evaluator_prompt = SessionController::build_evaluator_prompt(
            "session-123",
            &AgentConfig {
                cli: "claude".to_string(),
                model: Some("opus".to_string()),
                ..AgentConfig::default()
            },
            &[],
            0,
            "/repo/execution",
            false,
        );
        let required_protocol = extract_markdown_section(&evaluator_prompt, "## Required Protocol");

        assert!(
            required_protocol.starts_with(&SessionController::evaluator_required_protocol(
                "session-123"
            ))
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
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy: HiveExecutionPolicy::default(),
            qa_workers: Vec::new(),
            max_qa_iterations: 3,
            qa_timeout_secs: 300,
            auth_strategy: AuthStrategy::default(),
            worktree_path: None,
            worktree_branch: None,
            no_git: false,
            resume_report: None,
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
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy: HiveExecutionPolicy::default(),
            qa_workers: Vec::new(),
            max_qa_iterations: 3,
            qa_timeout_secs: 300,
            auth_strategy: AuthStrategy::default(),
            worktree_path: None,
            worktree_branch: None,
            no_git: false,
            resume_report: None,
        }
    }

    #[test]
    fn session_stop_and_close_drain_owned_scratch_ptys_without_agent_entries() {
        for (session_id, close) in [("scratch-stop", false), ("scratch-close", true)] {
            let temp_dir = tempfile::tempdir().expect("temp project dir");
            let controller = test_controller();
            let mut session = test_completion_session(
                session_id,
                SessionState::Running,
                Utc::now(),
                false,
            );
            session.project_path = temp_dir.path().to_path_buf();
            controller.insert_test_session(session);

            let pty_id = format!("scratch:{session_id}:test");
            controller
                .register_scratch_pty(session_id, pty_id.clone())
                .expect("scratch PTY should be owned by its session");
            assert!(
                controller
                    .scratch_ptys
                    .read()
                    .get(session_id)
                    .is_some_and(|ids| ids.contains(&pty_id))
            );

            if close {
                controller
                    .close_session(session_id)
                    .expect("closing should clean scratch PTYs");
            } else {
                controller
                    .stop_session(session_id)
                    .expect("stopping should clean scratch PTYs");
            }

            assert!(
                !controller.scratch_ptys.read().contains_key(session_id),
                "scratch ownership should be drained for {session_id}"
            );
        }
    }

    #[test]
    fn scratch_registration_rejects_terminal_and_cleanup_sessions() {
        for (session_id, state) in [
            ("scratch-completed", SessionState::Completed),
            ("scratch-closing", SessionState::Closing),
        ] {
            let controller = test_controller();
            controller.insert_test_session(test_completion_session(
                session_id,
                state,
                Utc::now(),
                false,
            ));

            let error = controller
                .register_scratch_pty(session_id, format!("scratch:{session_id}:test"))
                .expect_err("terminal sessions must reject new scratch PTYs");
            assert!(error.contains("not running"), "unexpected error: {error}");
        }

        let controller = test_controller();
        let session_id = "scratch-cleanup";
        controller.insert_test_session(test_completion_session(
            session_id,
            SessionState::Running,
            Utc::now(),
            false,
        ));
        let duplicate_id = format!("scratch:{session_id}:duplicate");
        controller
            .register_scratch_pty(session_id, duplicate_id.clone())
            .expect("first scratch registration should succeed");
        let duplicate_error = controller
            .register_scratch_pty(session_id, duplicate_id.clone())
            .expect_err("duplicate scratch registration must be rejected");
        assert!(
            duplicate_error.contains("already registered"),
            "unexpected error: {duplicate_error}"
        );
        controller.unregister_scratch_pty(&duplicate_id);
        controller
            .scratch_pty_cleanup_sessions
            .write()
            .insert(session_id.to_string());

        let error = controller
            .register_scratch_pty(session_id, format!("scratch:{session_id}:test"))
            .expect_err("cleanup barrier must reject a concurrent scratch registration");
        assert!(error.contains("is stopping"), "unexpected error: {error}");
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

    fn qa_session_with(
        id: &str,
        state: SessionState,
        project_path: PathBuf,
        with_prince: bool,
    ) -> Session {
        let mut agents = vec![AgentInfo {
            id: format!("{id}-evaluator"),
            role: AgentRole::Evaluator,
            status: AgentStatus::Running,
            config: AgentConfig::default(),
            parent_id: None,
            commit_sha: None,
            base_commit_sha: None,
        }];
        if with_prince {
            agents.push(AgentInfo {
                id: format!("{id}-prince"),
                role: AgentRole::Prince,
                status: AgentStatus::Running,
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
            session_type: SessionType::Hive { worker_count: 2 },
            project_path,
            state,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            agents,
            default_cli: "claude".to_string(),
            default_model: None,
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy: HiveExecutionPolicy::default(),
            qa_workers: Vec::new(),
            max_qa_iterations: 3,
            qa_timeout_secs: 300,
            auth_strategy: AuthStrategy::default(),
            worktree_path: None,
            worktree_branch: None,
            no_git: false,
            resume_report: None,
        }
    }

    #[test]
    fn qa_verdict_routes_to_prince_when_prince_present() {
        let controller = test_controller();
        controller.insert_test_session(qa_session_with(
            "prince-route",
            SessionState::QaInProgress { iteration: None },
            PathBuf::from("."),
            true,
        ));
        let new_state = controller
            .record_http_qa_verdict("prince-route", "prince-route-evaluator", "PASS", None)
            .expect("verdict recorded");
        // With a Prince present, even a PASS hands off to remediation before push.
        assert_eq!(new_state, SessionState::PrinceRemediation);
    }

    #[test]
    fn qa_verdict_passes_directly_without_prince() {
        let controller = test_controller();
        controller.insert_test_session(qa_session_with(
            "no-prince",
            SessionState::QaInProgress { iteration: None },
            PathBuf::from("."),
            false,
        ));
        let new_state = controller
            .record_http_qa_verdict("no-prince", "no-prince-evaluator", "PASS", None)
            .expect("verdict recorded");
        assert_eq!(new_state, SessionState::QaPassed);
    }

    #[test]
    fn prince_verdict_clears_remediation_to_qa_passed() {
        let controller = test_controller();
        controller.insert_test_session(qa_session_with(
            "prince-clear",
            SessionState::PrinceRemediation,
            PathBuf::from("."),
            true,
        ));
        let new_state = controller
            .record_prince_verdict("prince-clear", "PASS")
            .expect("prince verdict recorded");
        assert_eq!(new_state, SessionState::QaPassed);
    }

    #[test]
    fn prince_verdict_blocked_marks_inconclusive() {
        let controller = test_controller();
        controller.insert_test_session(qa_session_with(
            "prince-blocked",
            SessionState::PrinceRemediation,
            PathBuf::from("."),
            true,
        ));
        let new_state = controller
            .record_prince_verdict("prince-blocked", "BLOCKED")
            .expect("prince verdict recorded");
        assert_eq!(new_state, SessionState::QaInconclusive);
    }

    #[test]
    fn prince_verdict_rejected_outside_remediation() {
        let controller = test_controller();
        controller.insert_test_session(qa_session_with(
            "prince-bad-state",
            SessionState::QaInProgress { iteration: None },
            PathBuf::from("."),
            true,
        ));
        assert!(controller
            .record_prince_verdict("prince-bad-state", "PASS")
            .is_err());
    }

    #[test]
    fn qa_timeout_marks_inconclusive_blocks_completion_and_writes_verdict_file() {
        let temp = tempfile::tempdir().expect("temp");
        let controller = test_controller();
        controller.insert_test_session(qa_session_with(
            "inconclusive",
            SessionState::QaInProgress { iteration: None },
            temp.path().to_path_buf(),
            true,
        ));
        let new_state = controller
            .mark_qa_inconclusive("inconclusive", "timed out")
            .expect("marked inconclusive");
        assert_eq!(new_state, SessionState::QaInconclusive);

        // Inconclusive sessions must never auto-complete — push/complete stays blocked.
        let session = controller.get_session("inconclusive").expect("session");
        assert!(!SessionController::state_allows_completion(&session));

        // A BLOCKED verdict file is written so the Queen's poll loop terminates instead
        // of hanging forever.
        let verdict_path = temp
            .path()
            .join(".hive-manager")
            .join("inconclusive")
            .join("peer")
            .join("qa-verdict.json");
        assert!(verdict_path.exists());
        let body = std::fs::read_to_string(&verdict_path).expect("verdict body");
        assert!(body.contains("BLOCKED"));
        let blocked_pattern =
            regex::Regex::new(SessionController::qa_blocked_verdict_grep_pattern())
                .expect("valid grep-compatible BLOCKED pattern");
        assert!(
            blocked_pattern.is_match(&body),
            "Solo's shell guard must match the actual PeerMessageRecord envelope"
        );
    }

    #[test]
    fn adversarial_worker_count_is_ceil_half_of_workers() {
        assert_eq!(SessionController::adversarial_worker_count(0), 0);
        assert_eq!(SessionController::adversarial_worker_count(1), 1);
        assert_eq!(SessionController::adversarial_worker_count(2), 1);
        assert_eq!(SessionController::adversarial_worker_count(3), 2);
        assert_eq!(SessionController::adversarial_worker_count(4), 2);
        assert_eq!(SessionController::adversarial_worker_count(5), 3);
    }

    #[test]
    fn prince_remediation_and_inconclusive_block_completion() {
        for state in [
            SessionState::PrinceRemediation,
            SessionState::QaInconclusive,
        ] {
            let session = qa_session_with("gate", state.clone(), PathBuf::from("."), true);
            assert!(
                !SessionController::state_allows_completion(&session),
                "state {:?} must block completion",
                state
            );
        }
    }

    #[test]
    fn adversarial_workers_fill_configured_qa_to_target() {
        // A manually configured adversarial lane counts toward the target but must
        // not suppress the remaining automatic coverage.
        let prompt = SessionController::build_evaluator_prompt(
            "adv-config",
            &AgentConfig {
                cli: "claude".to_string(),
                model: Some("opus".to_string()),
                ..AgentConfig::default()
            },
            &[
                QaWorkerConfig {
                    specialization: "ui".to_string(),
                    cli: "claude".to_string(),
                    model: Some("opus".to_string()),
                    label: Some("UI QA".to_string()),
                    flags: None,
                },
                QaWorkerConfig {
                    specialization: "adversarial".to_string(),
                    cli: "claude".to_string(),
                    model: Some("opus".to_string()),
                    label: Some("Manual Adversarial QA".to_string()),
                    flags: None,
                },
            ],
            4, // ceil(4/2) = 2 adversarial agents expected
            "/repo/execution",
            false,
        );
        assert_eq!(
            prompt.matches(r#""specialization":"adversarial""#).count(),
            2,
            "one configured plus one automatic lane should satisfy the target"
        );
    }

    #[test]
    fn milestone_ready_does_not_regress_gated_states() {
        // Regression: a duplicate milestone-ready must not drag PrinceRemediation /
        // QaInconclusive back to QaInProgress (which would re-arm the QA timeout).
        for state in [
            SessionState::PrinceRemediation,
            SessionState::QaInconclusive,
        ] {
            let controller = test_controller();
            controller.insert_test_session(qa_session_with(
                "gated-milestone",
                state.clone(),
                PathBuf::from("."),
                true,
            ));
            controller
                .on_milestone_ready("gated-milestone")
                .expect("milestone-ready handled");
            let session = controller.get_session("gated-milestone").expect("session");
            assert_eq!(
                session.state, state,
                "milestone-ready must not regress {:?}",
                state
            );
        }
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
        let session = waiting_worker_session(session_id, temp_dir.path(), 1);
        std::fs::write(worktree_path.join("worker.txt"), "worker change\n")
            .expect("write worker change");
        run_git(&worktree_path, &["add", "worker.txt"]);
        run_git(&worktree_path, &["commit", "-m", "worker change"]);

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
    async fn on_worker_completed_skips_individual_commit_gate_for_shared_cell() {
        let _env_guard = ENV_MUTEX.lock().unwrap();
        let session_id = "worker-gate-shared";
        let (temp_dir, _) = init_repo_with_worker_worktree(session_id, 1);
        let mut session = waiting_worker_session(session_id, temp_dir.path(), 1);
        session.execution_policy.workspace_strategy = crate::domain::WorkspaceStrategy::SharedCell;
        session.worktree_path = Some(temp_dir.path().to_string_lossy().to_string());

        let controller = test_controller();
        controller.insert_test_session(session);

        unsafe {
            std::env::set_var("REQUIRE_COMMIT_SHA", "true");
        }
        let result = controller.on_worker_completed(session_id, 1).await;
        unsafe {
            std::env::remove_var("REQUIRE_COMMIT_SHA");
        }

        result.expect("shared-cell completion must not require an individual commit");
        let refreshed = controller.get_session(session_id).unwrap();
        assert_eq!(refreshed.agents[0].commit_sha, None);
    }

    #[tokio::test]
    async fn on_worker_completed_records_commit_sha_before_progression() {
        let session_id = "worker-gate-record";
        let (temp_dir, worktree_path) = init_repo_with_worker_worktree(session_id, 1);
        let session = waiting_worker_session(session_id, temp_dir.path(), 1);
        std::fs::write(worktree_path.join("worker.txt"), "worker change\n")
            .expect("write worker change");
        run_git(&worktree_path, &["add", "worker.txt"]);
        run_git(&worktree_path, &["commit", "-m", "worker change"]);
        let expected_head = current_head(&worktree_path).expect("worker HEAD");

        let controller = test_controller();
        controller.insert_test_session(session);

        controller
            .on_worker_completed(session_id, 1)
            .await
            .expect("missing pending config should not block commit capture");

        let refreshed = controller.get_session(session_id).unwrap();
        assert_eq!(refreshed.state, SessionState::WaitingForWorker(1));
        assert_eq!(
            refreshed.agents[0].commit_sha.as_deref(),
            Some(expected_head.as_str())
        );
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
        for args in [
            ["add", "README.md"].as_slice(),
            ["commit", "-m", "initial commit"].as_slice(),
        ] {
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
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy: HiveExecutionPolicy::default(),
            qa_workers: Vec::new(),
            max_qa_iterations: 3,
            qa_timeout_secs: 300,
            auth_strategy: AuthStrategy::default(),
            worktree_path: None, // Key: no session worktree for planning/swarm
            worktree_branch: None,
            no_git: false,
            resume_report: None,
        };

        assert!(session.worktree_path.is_none());
        let base_ref = SessionController::resolve_worker_base_ref(&session, "spawn_next_worker", 2);

        assert_eq!(base_ref, expected_head);
    }

    #[test]
    fn shared_cell_agents_resolve_artifacts_from_primary_worktree() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let primary = temp_dir.path().join("primary");
        let mut session = waiting_worker_session("shared-artifacts", temp_dir.path(), 1);
        session.execution_policy.workspace_strategy = crate::domain::WorkspaceStrategy::SharedCell;
        session.worktree_path = Some(primary.to_string_lossy().to_string());

        let worker = session.agents.first().unwrap();
        assert_eq!(
            SessionController::agent_git_worktree_path_for_artifacts(&session, worker),
            Some(primary.clone())
        );

        let mut queen = worker.clone();
        queen.role = AgentRole::Queen;
        assert_eq!(
            SessionController::agent_git_worktree_path_for_artifacts(&session, &queen),
            Some(primary)
        );
    }

    #[test]
    fn shared_worker_rollback_removes_files_but_preserves_primary_workspace() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let primary = temp_dir.path().join("primary");
        std::fs::create_dir_all(&primary).unwrap();
        let task = primary.join("worker-1-task.md");
        let prompt = primary.join("worker-1-prompt.md");
        std::fs::write(&task, "task").unwrap();
        std::fs::write(&prompt, "prompt").unwrap();

        SessionController::rollback_worker_launch_artifacts(
            temp_dir.path(),
            "shared-rollback",
            "worker-1",
            &task,
            Some(&prompt),
            false,
        );

        assert!(primary.exists());
        assert!(!task.exists());
        assert!(!prompt.exists());
    }

    #[test]
    fn primary_cell_artifact_merge_is_cumulative_and_idempotent() {
        let bundle = |name: &str| ArtifactBundle {
            summary: Some(format!("summary {name}")),
            changed_files: vec![format!("src/{name}.rs")],
            commits: vec![format!("{name}123 {name}")],
            branch: format!("hive/session/{name}"),
            test_results: None,
            diff_summary: Some(format!("diff {name}")),
            unresolved_issues: vec![format!("issue {name}")],
            confidence: Some(0.8),
            recommended_next_step: None,
        };

        let merged =
            SessionController::merge_primary_cell_artifact_bundles(bundle("a"), bundle("b"));
        let merged = SessionController::merge_primary_cell_artifact_bundles(merged, bundle("b"));

        assert_eq!(merged.changed_files, vec!["src/a.rs", "src/b.rs"]);
        assert_eq!(merged.commits, vec!["a123 a", "b123 b"]);
        assert_eq!(merged.summary.as_deref(), Some("summary a · summary b"));
        assert_eq!(merged.diff_summary.as_deref(), Some("diff a\n---\ndiff b"));
        assert_eq!(merged.unresolved_issues, vec!["issue a", "issue b"]);
        assert_eq!(merged.branch, "hive/session/a | hive/session/b");
    }

    #[test]
    fn command_builders_use_canonical_sol_and_preserve_custom_models() {
        let (_, explicit_args) = SessionController::build_command(&AgentConfig {
            cli: "codex".to_string(),
            model: Some("operator-selected-model".to_string()),
            ..AgentConfig::default()
        });
        assert!(explicit_args
            .windows(2)
            .any(|pair| { pair == ["-m".to_string(), "operator-selected-model".to_string()] }));
        assert!(!explicit_args.iter().any(|arg| arg == "gpt-5.6-sol"));

        let (_, default_args) = SessionController::build_command(&AgentConfig {
            cli: "codex".to_string(),
            model: None,
            ..AgentConfig::default()
        });
        assert!(default_args
            .windows(2)
            .any(|pair| pair == ["-m".to_string(), "gpt-5.6-sol".to_string()]));

        let legacy_config = AgentConfig {
            cli: "codex".to_string(),
            model: Some("gpt-5.6".to_string()),
            ..AgentConfig::default()
        };
        for (_, args) in [
            SessionController::build_command(&legacy_config),
            SessionController::build_solo_command(&legacy_config, Some("Do the task")),
        ] {
            assert!(args
                .windows(2)
                .any(|pair| pair == ["-m".to_string(), "gpt-5.6-sol".to_string()]));
            assert!(!args.iter().any(|arg| arg == "gpt-5.6"));
        }

        let legacy_flag_config = AgentConfig {
            cli: "codex".to_string(),
            model: None,
            flags: vec![
                "--full-auto".to_string(),
                "--model".to_string(),
                "gpt-5.6".to_string(),
            ],
            ..AgentConfig::default()
        };
        for (_, args) in [
            SessionController::build_command(&legacy_flag_config),
            SessionController::build_solo_command(&legacy_flag_config, None),
        ] {
            assert_eq!(args.iter().filter(|arg| *arg == "-m").count(), 1);
            assert!(args
                .windows(2)
                .any(|pair| pair == ["-m".to_string(), "gpt-5.6-sol".to_string()]));
            assert!(args.iter().any(|arg| arg == "--full-auto"));
            assert!(!args.iter().any(|arg| arg == "--model"));
        }
    }

    #[test]
    fn prince_uses_principal_defaults_and_topology_specific_integration() {
        let prince = AgentConfig {
            cli: "claude".to_string(),
            model: Some("opus".to_string()),
            ..AgentConfig::default()
        };
        let principal = codex_principal();
        let workspace = "/repo/.hive-manager/worktrees/session/primary";

        let shared = SessionController::build_prince_prompt(
            "session",
            &prince,
            &principal,
            workspace,
            WorkspaceStrategy::SharedCell,
            false,
        );
        assert!(shared.contains(r#""cli":"codex""#));
        assert!(shared.contains(r#""parent_id":"session-prince""#));
        assert!(shared.contains(r#""model": "gpt-5.6-sol""#));
        assert!(shared.contains("model_reasoning_effort"));
        assert!(shared.contains("do not merge or cherry-pick fixer branches"));
        assert!(shared.contains(workspace));

        let isolated = SessionController::build_prince_prompt(
            "session",
            &prince,
            &principal,
            workspace,
            WorkspaceStrategy::IsolatedCell,
            false,
        );
        assert!(isolated.contains("git -C"));
        assert!(isolated.contains("cherry-pick <sha>"));
        assert!(isolated.contains("hive/session/worker-N"));
    }

    // ---------------------------------------------------------------------
    // Debate mode: "Prior Wiki Context" load phase (issue #120)
    //
    // The acceptance criterion is prompt text, so these assert on the string
    // the builders actually return -- never on the template constant, which
    // would prove nothing about what a debater receives.
    // ---------------------------------------------------------------------

    const DEBATE_TEST_WIKI_PATH: &str = "/home/tester/.ai-docs/wiki";

    fn debate_test_debater_with_cli(cli: &str) -> DebateDebaterMetadata {
        DebateDebaterMetadata {
            index: 1,
            name: "Debater 1".to_string(),
            stance: Some("Monolith first".to_string()),
            slug: "debater-1".to_string(),
            branch: "debate/session-wiki-debater-1".to_string(),
            worktree_path: "/projects/app/debater-1".to_string(),
            config: AgentConfig {
                cli: cli.to_string(),
                ..AgentConfig::default()
            },
        }
    }

    fn debate_test_debater() -> DebateDebaterMetadata {
        debate_test_debater_with_cli("claude")
    }

    fn debate_test_metadata() -> DebateSessionMetadata {
        DebateSessionMetadata {
            base_branch: "main".to_string(),
            debaters: vec![debate_test_debater()],
            judge_config: AgentConfig::default(),
            topic: "Monolith versus microservices".to_string(),
            rounds: 2,
            verdict_file: ".hive-manager/session-wiki/debate/verdict.md".to_string(),
        }
    }

    fn render_debate_test_debater_prompt_for_cli(global_wiki_path: &str, cli: &str) -> String {
        SessionController::build_debate_debater_prompt(
            "session-wiki",
            &debate_test_debater_with_cli(cli),
            "Monolith versus microservices",
            1,
            2,
            Path::new("/projects/app/debater-1/argument.md"),
            None,
            "- Debater 2: `/projects/app/debater-2/argument.md`",
            Path::new("/projects/app/debater-1/task.md"),
            global_wiki_path,
        )
    }

    fn render_debate_test_debater_prompt(global_wiki_path: &str) -> String {
        render_debate_test_debater_prompt_for_cli(global_wiki_path, "claude")
    }

    /// Any leftover mustache control token means the render silently produced a
    /// prompt with raw template syntax in it -- the exact failure mode an
    /// unset `{{#if}}` gate flag causes.
    fn assert_no_unrendered_template_syntax(prompt: &str, label: &str) {
        for token in ["{{#if", "{{/if}}", "{{global_wiki_path}}"] {
            assert!(
                !prompt.contains(token),
                "{} prompt leaked unrendered template syntax {}:\n{}",
                label,
                token,
                prompt
            );
        }
    }

    /// A prompt with no wiki configured must contain no read of an empty path.
    /// `cat "/index.md"` (from a blank `{{global_wiki_path}}`) is the dangling
    /// read this guards against.
    fn assert_no_dangling_wiki_read(prompt: &str, label: &str) {
        assert!(
            !prompt.contains("/index.md"),
            "{} prompt still instructs a wiki index read with no wiki configured:\n{}",
            label,
            prompt
        );
        assert!(
            !prompt.contains("cat \""),
            "{} prompt still contains a dangling cat of an empty path:\n{}",
            label,
            prompt
        );
    }

    #[test]
    fn debater_prompt_loads_prior_wiki_context_when_path_configured() {
        let prompt = render_debate_test_debater_prompt(DEBATE_TEST_WIKI_PATH);

        assert!(
            prompt.contains("## Prior Wiki Context"),
            "debater prompt is missing the wiki load phase:\n{}",
            prompt
        );
        assert!(
            prompt.contains(DEBATE_TEST_WIKI_PATH),
            "debater prompt never names the configured wiki path:\n{}",
            prompt
        );
        assert!(
            prompt.contains(&format!("cat \"{}/index.md\"", DEBATE_TEST_WIKI_PATH)),
            "debater prompt is missing the concrete index read:\n{}",
            prompt
        );
        assert!(
            !prompt.contains("No global wiki path is configured"),
            "debater prompt rendered the skip notice despite a configured path:\n{}",
            prompt
        );
        assert_no_unrendered_template_syntax(&prompt, "debater");
    }

    #[test]
    fn debater_prompt_skips_wiki_load_gracefully_when_path_unset() {
        for unset in ["", "   "] {
            let prompt = render_debate_test_debater_prompt(unset);

            assert_no_dangling_wiki_read(&prompt, "debater");
            assert_no_unrendered_template_syntax(&prompt, "debater");
            assert!(
                prompt.contains("No global wiki path is configured"),
                "debater prompt is missing the explicit skip notice:\n{}",
                prompt
            );

            // ...and the prompt is still a usable debate brief.
            assert!(prompt.contains("Monolith versus microservices"));
            assert!(prompt.contains("Monolith first"));
            assert!(prompt.contains("Round 1 of 2"));
            assert!(prompt.contains("## Deliverable"));
            assert!(prompt.contains("/projects/app/debater-1/argument.md"));
        }
    }

    /// The sibling the debate templates had and `queen-research` did not.
    ///
    /// Its Phase 1 was gated by PROSE alone ("If the path is non-empty... / If it is
    /// empty..."), so an unset wiki rendered a literal `cat "/index.md"` — and the
    /// "verify the read succeeded, this is a defect the user needs to see" instruction
    /// then fired for a wiki that was never configured, turning a supported flow into a
    /// reported failure. The `{{#if}}` gate the debater and judge already used is what
    /// makes the empty case a real skip rather than a promise in prose.
    #[test]
    fn research_queen_prompt_skips_wiki_load_gracefully_when_path_unset() {
        for unset in ["", "   "] {
            let prompt = render_research_queen_prompt(unset, "claude");

            assert_no_dangling_wiki_read(&prompt, "queen-research");
            assert_no_unrendered_template_syntax(&prompt, "queen-research");
            assert!(
                prompt.contains("No global wiki path is configured"),
                "queen-research prompt is missing the explicit skip notice:\n{}",
                prompt
            );
            // The contradictory half: an unconfigured wiki must NOT be reported as an
            // unreadable one.
            assert!(
                !prompt.contains("WIKI INDEX UNREADABLE"),
                "queen-research tells the queen to report an unreadable wiki when none \
                 is configured at all:\n{}",
                prompt
            );
            // ...and it is still a usable research brief.
            assert!(prompt.contains("Investigate prompt path handling"));
            assert!(prompt.contains("Phase 2"));
        }
    }

    #[test]
    fn debate_judge_prompt_loads_prior_wiki_context_when_path_configured() {
        let prompt = SessionController::build_debate_judge_prompt(
            "session-wiki",
            &debate_test_metadata(),
            DEBATE_TEST_WIKI_PATH,
            "claude",
        );

        assert!(
            prompt.contains("## Prior Wiki Context"),
            "judge prompt is missing the wiki load phase:\n{}",
            prompt
        );
        assert!(
            prompt.contains(&format!("cat \"{}/index.md\"", DEBATE_TEST_WIKI_PATH)),
            "judge prompt is missing the concrete index read:\n{}",
            prompt
        );
        assert!(
            !prompt.contains("No global wiki path is configured"),
            "judge prompt rendered the skip notice despite a configured path:\n{}",
            prompt
        );
        // The pre-existing capture half must survive alongside the new load half.
        assert!(
            prompt.contains("## Wiki Capture"),
            "judge prompt lost its wiki capture phase:\n{}",
            prompt
        );
        assert_no_unrendered_template_syntax(&prompt, "judge");
    }

    #[test]
    fn debate_judge_prompt_skips_wiki_load_gracefully_when_path_unset() {
        for unset in ["", "   "] {
            let prompt = SessionController::build_debate_judge_prompt(
                "session-wiki",
                &debate_test_metadata(),
                unset,
                "claude",
            );

            assert_no_dangling_wiki_read(&prompt, "judge");
            assert_no_unrendered_template_syntax(&prompt, "judge");
            assert!(
                prompt.contains("No global wiki path is configured"),
                "judge prompt is missing the explicit skip notice:\n{}",
                prompt
            );

            // ...and the prompt is still a usable judging brief.
            assert!(prompt.contains("Monolith versus microservices"));
            assert!(prompt.contains("## Verdict Format"));
            assert!(prompt.contains(".hive-manager/session-wiki/debate/verdict.md"));
        }
    }

    // ---------------------------------------------------------------------
    // `global_wiki_path` separator / WSL normalization (issue #168)
    //
    // `expand_tilde` resolves `~` from `USERPROFILE` on Windows, so what reaches a
    // prompt is MIXED-separator. Every assertion below is on the string a builder
    // actually returns: a template-constant assertion would prove nothing about
    // what the agent receives, which is the whole point of the issue.
    // ---------------------------------------------------------------------

    /// Exactly what `expand_tilde("~/.ai-docs/wiki")` yields on Windows: `USERPROFILE`
    /// contributes backslashes, the configured remainder keeps its forward slashes.
    const MIXED_SEPARATOR_WIKI_PATH: &str = r"C:\Users\RDuff/.ai-docs/wiki";
    /// The Git-Bash/MSYS-safe spelling every non-WSL CLI must receive.
    const FORWARD_SLASH_WIKI_PATH: &str = "C:/Users/RDuff/.ai-docs/wiki";
    /// The only spelling that resolves under WSL. `C:/Users/...` does NOT.
    const WSL_WIKI_PATH: &str = "/mnt/c/Users/RDuff/.ai-docs/wiki";

    fn render_research_queen_prompt(global_wiki_path: &str, queen_cli: &str) -> String {
        let extra_vars =
            SessionController::research_queen_extra_vars(global_wiki_path, queen_cli, false);
        SessionController::build_templated_queen_prompt(
            "queen-research",
            "session-wiki",
            &[AgentConfig::default()],
            Some("Investigate prompt path handling"),
            extra_vars,
        )
    }

    /// A backslash surviving into the prompt is the defect. Asserting on the drive
    /// prefix catches it wherever in the prompt it appears -- the load block, the
    /// prose line naming the path, or the Phase 4 / Wiki Capture `cd`.
    fn assert_no_windows_separators(prompt: &str, label: &str) {
        assert!(
            !prompt.contains(r"C:\"),
            "{} prompt still carries a backslash-separated Windows wiki path:\n{}",
            label,
            prompt
        );
    }

    #[test]
    fn research_queen_prompt_renders_a_forward_slash_wiki_path() {
        let prompt = render_research_queen_prompt(MIXED_SEPARATOR_WIKI_PATH, "claude");

        assert!(
            prompt.contains(&format!("cat \"{}/index.md\"", FORWARD_SLASH_WIKI_PATH)),
            "queen-research prompt is missing the normalized index read:\n{}",
            prompt
        );
        // Phase 4's capture block quotes the same variable and must agree.
        assert!(
            prompt.contains(&format!("cd \"{}\"", FORWARD_SLASH_WIKI_PATH)),
            "queen-research wiki capture `cd` was not normalized:\n{}",
            prompt
        );
        assert_no_windows_separators(&prompt, "queen-research");
        assert_no_unrendered_template_syntax(&prompt, "queen-research");
    }

    /// A separator swap alone would leave `C:/Users/...` here, which is just as
    /// unresolvable under WSL as `C:\Users\...` -- solved-looking but not solved.
    #[test]
    fn research_queen_prompt_translates_the_wiki_path_for_a_wsl_backed_queen() {
        let prompt = render_research_queen_prompt(MIXED_SEPARATOR_WIKI_PATH, "cursor");

        assert!(
            prompt.contains(&format!("cat \"{}/index.md\"", WSL_WIKI_PATH)),
            "cursor-backed queen-research prompt did not get a /mnt-translated path:\n{}",
            prompt
        );
        assert!(
            prompt.contains(&format!("cd \"{}\"", WSL_WIKI_PATH)),
            "cursor-backed queen-research capture `cd` was not translated:\n{}",
            prompt
        );
        assert!(
            !prompt.contains("C:"),
            "cursor-backed queen-research prompt still names a drive letter WSL cannot \
             resolve:\n{}",
            prompt
        );
        assert_no_unrendered_template_syntax(&prompt, "queen-research");
    }

    #[test]
    fn debate_prompts_render_a_forward_slash_wiki_path() {
        let debater =
            render_debate_test_debater_prompt_for_cli(MIXED_SEPARATOR_WIKI_PATH, "claude");
        assert!(
            debater.contains(&format!("cat \"{}/index.md\"", FORWARD_SLASH_WIKI_PATH)),
            "debater prompt is missing the normalized index read:\n{}",
            debater
        );
        assert_no_windows_separators(&debater, "debater");
        assert_no_unrendered_template_syntax(&debater, "debater");

        let judge = SessionController::build_debate_judge_prompt(
            "session-wiki",
            &debate_test_metadata(),
            MIXED_SEPARATOR_WIKI_PATH,
            "claude",
        );
        assert!(
            judge.contains(&format!("cat \"{}/index.md\"", FORWARD_SLASH_WIKI_PATH)),
            "judge prompt is missing the normalized index read:\n{}",
            judge
        );
        assert!(
            judge.contains(&format!("cd \"{}\"", FORWARD_SLASH_WIKI_PATH)),
            "judge wiki capture `cd` was not normalized:\n{}",
            judge
        );
        assert_no_windows_separators(&judge, "judge");
        assert_no_unrendered_template_syntax(&judge, "judge");
    }

    #[test]
    fn debate_prompts_translate_the_wiki_path_for_wsl_backed_clis() {
        let debater =
            render_debate_test_debater_prompt_for_cli(MIXED_SEPARATOR_WIKI_PATH, "cursor");
        assert!(
            debater.contains(&format!("cat \"{}/index.md\"", WSL_WIKI_PATH)),
            "cursor-backed debater prompt did not get a /mnt-translated path:\n{}",
            debater
        );
        assert!(
            !debater.contains("C:"),
            "cursor-backed debater prompt still names a drive letter WSL cannot resolve:\n{}",
            debater
        );

        let judge = SessionController::build_debate_judge_prompt(
            "session-wiki",
            &debate_test_metadata(),
            MIXED_SEPARATOR_WIKI_PATH,
            "cursor",
        );
        assert!(
            judge.contains(&format!("cat \"{}/index.md\"", WSL_WIKI_PATH)),
            "cursor-backed judge prompt did not get a /mnt-translated path:\n{}",
            judge
        );
        assert!(
            judge.contains(&format!("cd \"{}\"", WSL_WIKI_PATH)),
            "cursor-backed judge capture `cd` was not translated:\n{}",
            judge
        );
        assert!(
            !judge.contains("C:"),
            "cursor-backed judge prompt still names a drive letter WSL cannot resolve:\n{}",
            judge
        );
    }

    /// A failed `cat` currently degrades to "no prior context" with no signal to
    /// anyone. Every prompt that instructs the read must also instruct the report.
    #[test]
    fn wiki_loading_prompts_require_reporting_a_failed_read() {
        let prompts = [
            (
                "queen-research",
                render_research_queen_prompt(MIXED_SEPARATOR_WIKI_PATH, "claude"),
            ),
            (
                "debater",
                render_debate_test_debater_prompt(DEBATE_TEST_WIKI_PATH),
            ),
            (
                "judge",
                SessionController::build_debate_judge_prompt(
                    "session-wiki",
                    &debate_test_metadata(),
                    DEBATE_TEST_WIKI_PATH,
                    "claude",
                ),
            ),
        ];

        for (label, prompt) in prompts {
            assert!(
                prompt.contains("WIKI INDEX UNREADABLE"),
                "{} prompt does not make a failed wiki read observable:\n{}",
                label,
                prompt
            );
            assert!(
                prompt.contains("Verify the read actually succeeded"),
                "{} prompt does not instruct the agent to check the read:\n{}",
                label,
                prompt
            );
        }
    }
}
