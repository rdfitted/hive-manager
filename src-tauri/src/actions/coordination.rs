//! Coordination and session-state actions behind the unified action registry.

use async_trait::async_trait;
use schemars::schema::RootSchema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use crate::coordination::{
    CoordinationMessage, InjectionError, MessageType, StateManager, WorkerStateInfo,
};
use crate::orchestrator::work_graph::schema::TaskTier;
use crate::orchestrator::work_graph::TaskId;
use crate::pty::{AgentConfig, AgentRole, WorkerRole};
use crate::tauri_shim::Emitter;

use super::error::ActionError;
use super::registry::{Action, ActionRegistry};
use super::{ActionContext, Caller};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QueenInjectRequest {
    pub session_id: String,
    pub queen_id: String,
    pub target_worker_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AddWorkerRequest {
    pub session_id: String,
    pub config: AgentConfig,
    pub role: WorkerRole,
    pub name: Option<String>,
    pub description: Option<String>,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OperatorInjectRequest {
    pub session_id: String,
    pub target_agent_id: String,
    pub message: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkerStatusRequest {
    pub session_id: String,
    pub queen_id: String,
    pub worker_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_label: Option<String>,
    pub priority: Option<String>,
    #[serde(default)]
    pub tier: TaskTier,
    #[serde(default)]
    pub depends_on: Vec<TaskId>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
    /// Internal parser provenance. It is intentionally absent from the stored/UI
    /// plan shape so adding graph syntax does not change legacy serialization.
    #[serde(skip)]
    pub(crate) explicit_id: bool,
    #[serde(skip)]
    pub(crate) checkbox_source: bool,
    /// Whether the assignee was normalized from a supported principal token.
    /// Unknown values remain schedulable and are surfaced as PlanReady warnings.
    #[serde(skip, default = "default_binding_recognized")]
    pub(crate) assignee_recognized: bool,
    /// Whether an explicit tier token used the supported vocabulary. Missing
    /// tokens are recognized and default to medium for legacy plans.
    #[serde(skip, default = "default_tier_recognized")]
    pub tier_recognized: bool,
    /// Raw explicit value retained only so graph omissions can name it.
    #[serde(skip)]
    pub(crate) tier_source: Option<String>,
}

fn default_binding_recognized() -> bool {
    true
}

fn default_tier_recognized() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionPlan {
    pub title: String,
    pub summary: String,
    pub tasks: Vec<PlanTask>,
    pub generated_at: String,
    pub raw_content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EmptyInput {}

#[derive(Debug, Deserialize, JsonSchema)]
struct QueenSwitchBranchInput {
    session_id: String,
    queen_id: String,
    branch: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CoordinationLogInput {
    session_id: String,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LogCoordinationMessageInput {
    session_id: String,
    from: String,
    to: String,
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SessionIdInput {
    session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AssignTaskInput {
    session_id: String,
    queen_id: String,
    worker_id: String,
    task: String,
    plan_task_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListStoredSessionsInput {
    project_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UpdateAppConfigInput {
    config: Value,
}

fn deserialize_input<T: for<'de> Deserialize<'de>>(input: Value) -> Result<T, ActionError> {
    serde_json::from_value(input)
        .map_err(|e| ActionError::bad_request(format!("Invalid input: {}", e)))
}

fn serialize_output<T: Serialize>(value: T, label: &str) -> Result<Value, ActionError> {
    serde_json::to_value(value)
        .map_err(|e| ActionError::internal(format!("Failed to serialize {}: {}", label, e)))
}

fn require_frontend(ctx: &ActionContext) -> Result<(), ActionError> {
    if matches!(ctx.caller, Caller::Frontend) {
        Ok(())
    } else {
        Err(ActionError::bad_request(
            "Coordination actions are only available through Tauri commands",
        ))
    }
}

async fn run_blocking_injection<T, F>(operation: F) -> Result<T, ActionError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, InjectionError> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| ActionError::internal(format!("Injection task failed: {error}")))?
        .map_err(|error| ActionError::internal(error.to_string()))
}

struct QueenInject;

#[async_trait]
impl Action for QueenInject {
    fn name(&self) -> &'static str {
        "coordination.queen_inject"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(QueenInjectRequest)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let request: QueenInjectRequest = deserialize_input(input)?;
        let manager = Arc::clone(&ctx.state.injection_manager);
        run_blocking_injection(move || {
            manager.read().queen_inject(
                &request.session_id,
                &request.queen_id,
                &request.target_worker_id,
                &request.message,
                true,
            )
        })
        .await?;
        Ok(Value::Null)
    }
}

struct QueenSwitchBranch;

#[async_trait]
impl Action for QueenSwitchBranch {
    fn name(&self) -> &'static str {
        "coordination.queen_switch_branch"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(QueenSwitchBranchInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: QueenSwitchBranchInput = deserialize_input(input)?;
        let worker_ids = {
            let controller = ctx.state.session_controller.read();
            controller
                .get_session(&parsed.session_id)
                .map(|s| {
                    s.agents
                        .iter()
                        .filter(|a| matches!(a.role, AgentRole::Worker { .. }))
                        .map(|a| a.id.clone())
                        .collect::<Vec<_>>()
                })
                .ok_or_else(|| ActionError::not_found("Session not found"))?
        };

        let manager = Arc::clone(&ctx.state.injection_manager);
        let results = run_blocking_injection(move || {
            manager.read().queen_switch_branch(
                &parsed.session_id,
                &parsed.queen_id,
                &worker_ids,
                &parsed.branch,
            )
        })
        .await?;

        serialize_output(
            results
                .into_iter()
                .map(|(id, result)| (id, result.is_ok()))
                .collect::<Vec<(String, bool)>>(),
            "branch switch results",
        )
    }
}

struct OperatorInject;

#[async_trait]
impl Action for OperatorInject {
    fn name(&self) -> &'static str {
        "coordination.operator_inject"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(OperatorInjectRequest)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let request: OperatorInjectRequest = deserialize_input(input)?;
        let manager = Arc::clone(&ctx.state.injection_manager);
        run_blocking_injection(move || {
            manager.read().operator_inject(
                &request.session_id,
                &request.target_agent_id,
                &request.message,
                true,
            )
        })
        .await?;
        Ok(Value::Null)
    }
}

struct ReportWorkerStatus;

#[async_trait]
impl Action for ReportWorkerStatus {
    fn name(&self) -> &'static str {
        "coordination.report_worker_status"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(WorkerStatusRequest)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let request: WorkerStatusRequest = deserialize_input(input)?;
        let manager = ctx.state.injection_manager.read();
        manager
            .notify_queen_worker_status(
                &request.session_id,
                &request.queen_id,
                &request.worker_id,
                &request.status,
            )
            .map_err(|e| ActionError::internal(e.to_string()))?;
        Ok(Value::Null)
    }
}

struct AddWorker;

#[async_trait]
impl Action for AddWorker {
    fn name(&self) -> &'static str {
        "coordination.add_worker"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(AddWorkerRequest)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let request: AddWorkerRequest = deserialize_input(input)?;
        let controller = ctx.state.session_controller.write();

        let mut config = request.config;
        let normalize_opt_str = |value: Option<String>| {
            value.and_then(|v| {
                let trimmed = v.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            })
        };
        config.name = normalize_opt_str(request.name).or_else(|| normalize_opt_str(config.name));
        config.description = normalize_opt_str(request.description)
            .or_else(|| normalize_opt_str(config.description));

        let agent_info = controller
            .add_worker(
                &request.session_id,
                config,
                request.role.clone(),
                request.parent_id,
                // This surface does not reserve an index up front (it also does
                // not enqueue a queue row), so there is nothing to race against.
                None,
            )
            .map_err(|e| ActionError::internal(e.to_string()))?;

        let coord_manager = ctx.state.injection_manager.read();
        let queen_id = format!("{}-queen", request.session_id);
        let worker_state = WorkerStateInfo {
            id: agent_info.id.clone(),
            role: request.role,
            cli: agent_info.config.cli.clone(),
            status: "Running".to_string(),
            current_task: None,
            last_update: chrono::Utc::now(),
            last_heartbeat: None,
        };
        let _ =
            coord_manager.notify_queen_worker_added(&request.session_id, &queen_id, &worker_state);

        let session_path = ctx.state.storage.session_dir(&request.session_id);
        let state_manager = StateManager::new(session_path);

        if let Some(session) = controller.get_session(&request.session_id) {
            let workers: Vec<WorkerStateInfo> = session
                .agents
                .iter()
                .filter(|a| {
                    !matches!(
                        a.role,
                        AgentRole::Queen | AgentRole::Evaluator | AgentRole::QaWorker { .. }
                    )
                })
                .map(|a| WorkerStateInfo {
                    id: a.id.clone(),
                    role: a.config.role.clone().unwrap_or_default(),
                    cli: a.config.cli.clone(),
                    status: format!("{:?}", a.status),
                    current_task: None,
                    last_update: chrono::Utc::now(),
                    last_heartbeat: None,
                })
                .collect();

            state_manager
                .update_workers_file(&workers)
                .map_err(|e| ActionError::internal(e.to_string()))?;
        }

        serialize_output(agent_info, "agent info")
    }
}

struct GetCoordinationLog;

#[async_trait]
impl Action for GetCoordinationLog {
    fn name(&self) -> &'static str {
        "coordination.get_log"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(CoordinationLogInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: CoordinationLogInput = deserialize_input(input)?;
        let manager = ctx.state.injection_manager.read();
        let log = manager
            .get_coordination_log(&parsed.session_id, parsed.limit)
            .map_err(|e| ActionError::internal(e.to_string()))?;
        serialize_output(log, "coordination log")
    }
}

struct LogCoordinationMessage;

#[async_trait]
impl Action for LogCoordinationMessage {
    fn name(&self) -> &'static str {
        "coordination.log_message"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(LogCoordinationMessageInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: LogCoordinationMessageInput = deserialize_input(input)?;
        let coord_message = CoordinationMessage::new(
            &parsed.from,
            &parsed.to,
            &parsed.content,
            MessageType::System,
        );
        ctx.state
            .storage
            .append_coordination_log(&parsed.session_id, &coord_message)
            .map_err(|e| ActionError::internal(e.to_string()))?;
        if let Some(app_handle) = ctx.state.app_handle.as_ref() {
            app_handle
                .emit("coordination-message", &coord_message)
                .map_err(|e| ActionError::internal(e.to_string()))?;
        }
        Ok(Value::Null)
    }
}

struct GetWorkersState;

#[async_trait]
impl Action for GetWorkersState {
    fn name(&self) -> &'static str {
        "coordination.get_workers_state"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(SessionIdInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: SessionIdInput = deserialize_input(input)?;
        let session_path = ctx.state.storage.session_dir(&parsed.session_id);
        let state_manager = StateManager::new(session_path);
        let workers = state_manager
            .read_workers_file()
            .map_err(|e| ActionError::internal(e.to_string()))?;
        serialize_output(workers, "workers state")
    }
}

struct AssignTask;

#[async_trait]
impl Action for AssignTask {
    fn name(&self) -> &'static str {
        "coordination.assign_task"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(AssignTaskInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: AssignTaskInput = deserialize_input(input)?;
        let manager = Arc::clone(&ctx.state.injection_manager);
        let injection_session_id = parsed.session_id.clone();
        let queen_id = parsed.queen_id.clone();
        let worker_id = parsed.worker_id.clone();
        let task = parsed.task.clone();
        run_blocking_injection(move || {
            manager
                .read()
                .queen_inject(&injection_session_id, &queen_id, &worker_id, &task, true)
        })
        .await?;

        let session_path = ctx.state.storage.session_dir(&parsed.session_id);
        let state_manager = StateManager::new(session_path);
        state_manager
            .record_assignment(&parsed.worker_id, &parsed.task, parsed.plan_task_id)
            .map_err(|e| ActionError::internal(e.to_string()))?;
        Ok(Value::Null)
    }
}

struct GetSessionStoragePath;

#[async_trait]
impl Action for GetSessionStoragePath {
    fn name(&self) -> &'static str {
        "coordination.get_session_storage_path"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(SessionIdInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: SessionIdInput = deserialize_input(input)?;
        let path = ctx.state.storage.session_dir(&parsed.session_id);
        Ok(Value::String(path.to_string_lossy().to_string()))
    }
}

struct GetCurrentDirectory;

#[async_trait]
impl Action for GetCurrentDirectory {
    fn name(&self) -> &'static str {
        "coordination.get_current_directory"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(EmptyInput)
    }

    async fn run(&self, _ctx: &ActionContext, _input: Value) -> Result<Value, ActionError> {
        require_frontend(_ctx)?;
        std::env::current_dir()
            .map(|p| Value::String(p.to_string_lossy().to_string()))
            .map_err(|e| ActionError::internal(e.to_string()))
    }
}

struct ListStoredSessions;

#[async_trait]
impl Action for ListStoredSessions {
    fn name(&self) -> &'static str {
        "coordination.list_stored_sessions"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(ListStoredSessionsInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: ListStoredSessionsInput = deserialize_input(input)?;
        let sessions = ctx
            .state
            .storage
            .list_sessions()
            .map_err(|e| ActionError::internal(e.to_string()))?;

        let sessions = match parsed.project_path {
            Some(path) => {
                let normalize = |p: &str| -> String {
                    let p = p.trim_end_matches(['/', '\\']);
                    #[cfg(windows)]
                    {
                        p.to_lowercase()
                    }
                    #[cfg(not(windows))]
                    {
                        p.to_string()
                    }
                };

                let target = normalize(&path);
                sessions
                    .into_iter()
                    .filter(|s| normalize(&s.project_path) == target)
                    .collect()
            }
            None => sessions,
        };

        serialize_output(sessions, "stored sessions")
    }
}

struct GetAppConfig;

#[async_trait]
impl Action for GetAppConfig {
    fn name(&self) -> &'static str {
        "coordination.get_app_config"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(EmptyInput)
    }

    async fn run(&self, ctx: &ActionContext, _input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let config = ctx
            .state
            .storage
            .load_config()
            .map_err(|e| ActionError::internal(e.to_string()))?;
        serialize_output(config, "app config")
    }
}

struct UpdateAppConfig;

#[async_trait]
impl Action for UpdateAppConfig {
    fn name(&self) -> &'static str {
        "coordination.update_app_config"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(UpdateAppConfigInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: UpdateAppConfigInput = deserialize_input(input)?;
        let config = serde_json::from_value(parsed.config)
            .map_err(|e| ActionError::bad_request(format!("Invalid app config: {}", e)))?;
        ctx.state
            .storage
            .save_config(&config)
            .map_err(|e| ActionError::internal(e.to_string()))?;
        Ok(Value::Null)
    }
}

struct GetSessionPlan;

#[async_trait]
impl Action for GetSessionPlan {
    fn name(&self) -> &'static str {
        "coordination.get_session_plan"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(SessionIdInput)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        require_frontend(ctx)?;
        let parsed: SessionIdInput = deserialize_input(input)?;
        let project_plan_path = {
            let controller = ctx.state.session_controller.read();
            controller.get_session(&parsed.session_id).map(|session| {
                session
                    .project_path
                    .join(".hive-manager")
                    .join(&parsed.session_id)
                    .join("plan.md")
            })
        };

        let plan_path = if let Some(ref path) = project_plan_path {
            if path.exists() {
                path.clone()
            } else {
                ctx.state
                    .storage
                    .session_dir(&parsed.session_id)
                    .join("plan.md")
            }
        } else {
            ctx.state
                .storage
                .session_dir(&parsed.session_id)
                .join("plan.md")
        };

        if !plan_path.exists() {
            return Ok(Value::Null);
        }

        let content = std::fs::read_to_string(&plan_path)
            .map_err(|e| ActionError::internal(format!("Failed to read plan.md: {}", e)))?;
        serialize_output(Some(parse_plan_markdown(&content)), "session plan")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanMarkdownError {
    pub messages: Vec<String>,
}

impl std::fmt::Display for PlanMarkdownError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.messages.join("; "))
    }
}

impl std::error::Error for PlanMarkdownError {}

pub(crate) fn parse_plan_markdown(content: &str) -> SessionPlan {
    parse_plan_markdown_with_diagnostics(content).0
}

pub(crate) fn parse_plan_markdown_checked(content: &str) -> Result<SessionPlan, PlanMarkdownError> {
    let (plan, messages) = parse_plan_markdown_with_diagnostics(content);
    if messages.is_empty() {
        Ok(plan)
    } else {
        Err(PlanMarkdownError { messages })
    }
}

pub(crate) fn parse_plan_markdown_with_diagnostics(content: &str) -> (SessionPlan, Vec<String>) {
    let mut title = String::new();
    let mut summary = String::new();
    let mut tasks: Vec<PlanTask> = Vec::new();
    let mut diagnostics = Vec::new();
    let mut current_section = "";
    let mut task_counter = 0;

    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        if trimmed.starts_with("# ") && title.is_empty() {
            title = trimmed[2..].trim().to_string();
            continue;
        }

        if let Some(section) = trimmed.strip_prefix("## ") {
            let section_name = section.trim().to_lowercase();
            if section_name.contains("summary") || section_name.contains("overview") {
                current_section = "summary";
            } else if section_name.contains("task") || section_name.contains("plan") {
                current_section = "tasks";
            } else {
                current_section = "";
            }
            continue;
        }

        if current_section == "summary" && !trimmed.is_empty() && !trimmed.starts_with('#') {
            if !summary.is_empty() {
                summary.push(' ');
            }
            summary.push_str(trimmed);
            continue;
        }

        if current_section == "tasks" {
            let (task, error) = parse_task_line_with_diagnostics(trimmed, &mut task_counter);
            if let Some(message) = error {
                diagnostics.push(format!("line {}: {}", line_index + 1, message));
            }
            if let Some(task) = task {
                tasks.push(task);
            }
        }
    }

    if title.is_empty() {
        title = "Plan in Progress...".to_string();
    }
    if tasks.iter().any(|task| task.explicit_id) {
        for task in tasks
            .iter()
            .filter(|task| task.checkbox_source && !task.explicit_id)
        {
            diagnostics.push(format!(
                "schedulable checkbox task is missing a stable T<number>: id: {}",
                task.title
            ));
        }
    }

    (
        SessionPlan {
            title,
            summary,
            tasks,
            generated_at: chrono::Utc::now().to_rfc3339(),
            raw_content: content.to_string(),
        },
        diagnostics,
    )
}

#[cfg(test)]
fn parse_task_line(line: &str, counter: &mut i32) -> Option<PlanTask> {
    parse_task_line_with_diagnostics(line, counter).0
}

#[derive(Debug)]
struct TaskMetadata {
    depends_on: Vec<TaskId>,
    inputs: Vec<String>,
    outputs: Vec<String>,
    acceptance: Vec<String>,
    tier: TaskTier,
    tier_recognized: bool,
    tier_source: Option<String>,
}

impl Default for TaskMetadata {
    fn default() -> Self {
        Self {
            depends_on: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            acceptance: Vec::new(),
            tier: TaskTier::default(),
            tier_recognized: true,
            tier_source: None,
        }
    }
}

/// Task-line extraction order is deliberately stable: checkbox/list marker ->
/// graph metadata (including tier) -> priority -> explicit `T<number>:` id ->
/// assignee. Priority still precedes assignee exactly as it did for legacy lines;
/// metadata is removed first so neither historical extractor can consume it.
fn parse_task_line_with_diagnostics(
    line: &str,
    counter: &mut i32,
) -> (Option<PlanTask>, Option<String>) {
    let trimmed = line.trim();
    let checkbox_source = ["- [ ]", "* [ ]", "- [x]", "* [x]", "- [X]", "* [X]"]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix));

    if trimmed.is_empty() || trimmed.starts_with('#') {
        return (None, None);
    }

    let (status, rest) = if trimmed.starts_with("- [ ]") || trimmed.starts_with("* [ ]") {
        ("pending", trimmed[5..].trim())
    } else if trimmed.starts_with("- [x]")
        || trimmed.starts_with("* [x]")
        || trimmed.starts_with("- [X]")
        || trimmed.starts_with("* [X]")
    {
        ("completed", trimmed[5..].trim())
    } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        ("pending", trimmed[2..].trim())
    } else if trimmed
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        if let Some(pos) = trimmed.find(". ") {
            ("pending", trimmed[pos + 2..].trim())
        } else {
            return (None, None);
        }
    } else {
        return (None, None);
    };

    if rest.is_empty() {
        return (None, None);
    }

    *counter += 1;
    let (rest, metadata, metadata_error) = extract_task_metadata(rest);
    let (title, priority) = extract_priority(&rest);
    let (title, explicit_id) = extract_explicit_task_id(&title);
    let has_explicit_id = explicit_id.is_some();
    let (title, assignee, assignee_label, assignee_recognized) = extract_assignee(&title);

    (
        Some(PlanTask {
            id: explicit_id.unwrap_or_else(|| format!("task-{}", counter)),
            title: title.trim().to_string(),
            description: String::new(),
            status: status.to_string(),
            assignee,
            assignee_label,
            priority,
            tier: metadata.tier,
            depends_on: metadata.depends_on,
            inputs: metadata.inputs,
            outputs: metadata.outputs,
            acceptance: metadata.acceptance,
            explicit_id: has_explicit_id,
            checkbox_source,
            assignee_recognized,
            tier_recognized: metadata.tier_recognized,
            tier_source: metadata.tier_source,
        }),
        metadata_error,
    )
}

fn extract_explicit_task_id(text: &str) -> (String, Option<TaskId>) {
    let Some((candidate, remainder)) = text.split_once(':') else {
        return (text.to_string(), None);
    };
    let candidate = candidate.trim();
    let candidate = candidate
        .strip_prefix('[')
        .and_then(|candidate| candidate.split_once(']'))
        .map_or(candidate, |(_, candidate)| candidate.trim_start());
    let mut chars = candidate.chars();
    let is_explicit_id = matches!(chars.next(), Some('T' | 't'))
        && chars.clone().next().is_some()
        && chars.all(|character| character.is_ascii_digit());

    if is_explicit_id {
        (
            remainder.trim_start().to_string(),
            Some(candidate.to_string()),
        )
    } else {
        (text.to_string(), None)
    }
}

fn extract_task_metadata(text: &str) -> (String, TaskMetadata, Option<String>) {
    let mut cleaned = text.to_string();
    let mut metadata = TaskMetadata::default();

    for key in ["deps", "inputs", "outputs", "acceptance", "tier"] {
        let marker = format!("({key}:");
        while let Some(start) = if key == "tier" {
            cleaned.to_ascii_lowercase().find(&marker)
        } else {
            cleaned.find(&marker)
        } {
            let value_start = start + marker.len();
            let Some(close_offset) = cleaned[value_start..].find(')') else {
                return (
                    text.to_string(),
                    TaskMetadata::default(),
                    Some(format!("unterminated ({key}: ...) metadata")),
                );
            };
            let close = value_start + close_offset;
            let raw_value = cleaned[value_start..close].trim().to_string();
            let values = cleaned[value_start..close]
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();

            match key {
                "deps" => append_unique(&mut metadata.depends_on, values),
                "inputs" => append_unique(&mut metadata.inputs, values),
                "outputs" => append_unique(&mut metadata.outputs, values),
                "acceptance" => append_unique(&mut metadata.acceptance, values),
                "tier" => {
                    let (tier, recognized) = match raw_value.to_ascii_lowercase().as_str() {
                        "low" => (TaskTier::Low, true),
                        "medium" => (TaskTier::Medium, true),
                        "high" => (TaskTier::High, true),
                        "critical" => (TaskTier::Critical, true),
                        _ => (TaskTier::Medium, false),
                    };
                    if recognized {
                        if metadata.tier_recognized {
                            metadata.tier = tier;
                            metadata.tier_source = Some(raw_value);
                        }
                    } else {
                        if metadata.tier_recognized {
                            metadata.tier_source = Some(raw_value);
                        }
                        metadata.tier = TaskTier::Medium;
                        metadata.tier_recognized = false;
                    }
                }
                _ => unreachable!("metadata keys are fixed"),
            }
            cleaned.replace_range(start..=close, "");
        }
    }

    (cleaned, metadata, None)
}

fn append_unique(target: &mut Vec<String>, values: Vec<String>) {
    for value in values {
        if !target.contains(&value) {
            target.push(value);
        }
    }
}

fn extract_priority(text: &str) -> (String, Option<String>) {
    let priorities = [
        ("[HIGH]", "high"),
        ("[P1]", "high"),
        ("[CRITICAL]", "high"),
        ("[MEDIUM]", "medium"),
        ("[P2]", "medium"),
        ("[MED]", "medium"),
        ("[LOW]", "low"),
        ("[P3]", "low"),
    ];

    for (marker, priority) in priorities {
        if text
            .split_whitespace()
            .any(|token| token.eq_ignore_ascii_case(marker))
        {
            let cleaned = text
                .split_whitespace()
                .filter(|token| !token.eq_ignore_ascii_case(marker))
                .collect::<Vec<_>>()
                .join(" ");
            return (cleaned, Some(priority.to_string()));
        }
    }

    (text.to_string(), None)
}

fn extract_assignee(text: &str) -> (String, Option<String>, Option<String>, bool) {
    for separator in ["->", "\u{2192}"] {
        if let Some((title, assignee)) = text.split_once(separator) {
            let assignee = assignee.trim();
            if assignee.is_empty() {
                return (title.to_string(), None, None, false);
            }
            let first_token = assignee.split_whitespace().next().unwrap_or(assignee);
            let normalized = normalize_principal_token(first_token);
            if let Some(principal) = normalized {
                let label = assignee[first_token.len()..].trim();
                return (
                    title.to_string(),
                    Some(principal),
                    (!label.is_empty()).then(|| label.to_string()),
                    true,
                );
            }
            return (title.to_string(), Some(assignee.to_string()), None, false);
        }
    }

    (text.to_string(), None, None, true)
}

fn normalize_principal_token(token: &str) -> Option<String> {
    let mut characters = token.chars();
    if matches!(characters.next(), Some('P' | 'p')) {
        let digits = characters.as_str();
        if !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit()) {
            return Some(format!("P{digits}"));
        }
    }
    if token.eq_ignore_ascii_case("queen") {
        return Some("Queen".to_string());
    }
    if token.eq_ignore_ascii_case("operator") {
        return Some("Operator".to_string());
    }
    if let Some(index) = token
        .to_ascii_lowercase()
        .strip_prefix("worker-")
        .filter(|index| {
            !index.is_empty() && index.chars().all(|character| character.is_ascii_digit())
        })
    {
        return Some(format!("worker-{index}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        extract_assignee, extract_priority, parse_plan_markdown_with_diagnostics, parse_task_line,
        run_blocking_injection,
    };
    use crate::coordination::InjectionError;
    use crate::orchestrator::work_graph::schema::TaskTier;
    use crate::orchestrator::work_graph::WorkGraphOmissionReason;

    #[tokio::test]
    async fn blocking_injection_preserves_operation_error_message() {
        let expected = InjectionError::NotAuthorized("denied".to_string());
        let expected_message = expected.to_string();

        let error = run_blocking_injection(move || Err::<(), _>(expected))
            .await
            .unwrap_err();

        assert_eq!(error.message, expected_message);
    }

    #[test]
    fn extract_priority_strips_detected_token_case_insensitively() {
        let (title, priority) = extract_priority("[High] Fix launch regression");

        assert_eq!(title, "Fix launch regression");
        assert_eq!(priority.as_deref(), Some("high"));
    }

    #[test]
    fn task_line_resolves_explicit_id_with_optional_bracket_label_and_spacing() {
        let cases = [
            ("P4", "- [ ] [P4] T4: title", "T4"),
            ("P18", "- [ ] [P18] T18: title", "T18"),
            ("Queen", "- [ ] [Queen] T5: title", "T5"),
            ("Operator", "- [ ] [Operator] T6: title", "T6"),
            ("bare-T", "- [ ] T7: title", "T7"),
            ("spaced-colon", "- [ ] T8 : title", "T8"),
        ];

        for (case, line, expected_id) in cases {
            let mut counter = 0;
            let task = parse_task_line(line, &mut counter)
                .unwrap_or_else(|| panic!("{case}: expected a parsed task"));

            assert_eq!(task.id, expected_id, "{case}");
            assert!(task.explicit_id, "{case}");
            assert_eq!(task.title, "title", "{case}");
        }
    }

    #[test]
    fn task_line_keeps_recognized_priority_tokens() {
        let cases = [
            ("HIGH", "high"),
            ("MEDIUM", "medium"),
            ("LOW", "low"),
            ("P1", "high"),
            ("P2", "medium"),
            ("P3", "low"),
        ];

        for (marker, expected_priority) in cases {
            let mut counter = 0;
            let line = format!("- [ ] [{marker}] T1: title");
            let task = parse_task_line(&line, &mut counter)
                .unwrap_or_else(|| panic!("{marker}: expected a parsed task"));

            assert_eq!(task.id, "T1", "{marker}");
            assert_eq!(task.title, "title", "{marker}");
            assert_eq!(
                task.priority.as_deref(),
                Some(expected_priority),
                "{marker}"
            );
        }
    }

    #[test]
    fn task_line_tier_is_copied_and_stripped_from_graph_title() {
        let mut counter = 0;
        let task = parse_task_line(
            "- [ ] [P1] T3: Cross a trust boundary (tier: high) -> P1",
            &mut counter,
        )
        .expect("tiered principal-bound task");

        assert_eq!(task.id, "T3");
        assert_eq!(task.priority.as_deref(), Some("high"));
        assert_eq!(task.tier, TaskTier::High);
        assert!(task.tier_recognized);

        let plan = super::SessionPlan {
            title: "Tiered plan".to_string(),
            summary: String::new(),
            tasks: vec![task],
            generated_at: String::new(),
            raw_content: String::new(),
        };
        let graph = crate::orchestrator::work_graph::plan_parse::task_graph_from_plan(&plan);

        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.nodes[0].tier, TaskTier::High);
        assert_eq!(graph.nodes[0].title, "Cross a trust boundary");
    }

    #[test]
    fn task_line_tier_token_is_case_insensitive() {
        let mut counter = 0;
        let task = parse_task_line(
            "- [ ] T3: Resolve architecture (TiEr: CrItIcAl)",
            &mut counter,
        )
        .expect("case-insensitive tier token");

        assert_eq!(task.tier, TaskTier::Critical);
        assert_eq!(task.title, "Resolve architecture");
        assert!(task.tier_recognized);
    }

    #[test]
    fn task_line_without_tier_defaults_medium_independently_of_priority() {
        let mut counter = 0;
        let ordinary =
            parse_task_line("- [ ] T1: Existing-test change", &mut counter).expect("ordinary task");
        let urgent = parse_task_line("- [ ] [P1] T2: Urgent existing-test change", &mut counter)
            .expect("urgent task");

        assert_eq!(ordinary.tier, TaskTier::Medium);
        assert_eq!(urgent.tier, TaskTier::Medium);
        assert_eq!(urgent.priority.as_deref(), Some("high"));
        assert!(ordinary.tier_recognized);
        assert!(urgent.tier_recognized);
    }

    #[test]
    fn unknown_tier_keeps_node_and_records_resolution_incomplete_omission() {
        let plan = super::parse_plan_markdown_checked(
            "# Plan\n\n## Tasks\n- [ ] T4: Keep this node (tier: huge) -> P1\n",
        )
        .expect("unknown tiers are omissions, not parse errors");

        assert_eq!(plan.tasks.len(), 1);
        assert_eq!(plan.tasks[0].tier, TaskTier::Medium);
        assert!(!plan.tasks[0].tier_recognized);
        assert_eq!(plan.tasks[0].title, "Keep this node");

        let graph = crate::orchestrator::work_graph::plan_parse::task_graph_from_plan(&plan);
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.nodes[0].id, "T4");
        assert_eq!(graph.nodes[0].tier, TaskTier::Medium);
        assert_eq!(graph.omissions.len(), 1);
        assert_eq!(
            graph.omissions[0].reason,
            WorkGraphOmissionReason::ResolutionIncomplete
        );
        assert_eq!(graph.omissions[0].count, 1);
        assert_eq!(
            graph.omissions[0].examples,
            vec!["task T4 preserved unrecognized tier huge as medium"]
        );
    }

    #[test]
    fn unknown_tier_remains_recorded_when_followed_by_valid_duplicate() {
        let plan = super::parse_plan_markdown_checked(
            "# Plan\n\n## Tasks\n- [ ] T5: Keep this node (tier: huge) (tier: high)\n",
        )
        .expect("duplicate tier tokens remain schedulable");

        assert_eq!(plan.tasks[0].tier, TaskTier::Medium);
        assert!(!plan.tasks[0].tier_recognized);
        assert_eq!(plan.tasks[0].title, "Keep this node");

        let graph = crate::orchestrator::work_graph::plan_parse::task_graph_from_plan(&plan);
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.nodes[0].tier, TaskTier::Medium);
        assert_eq!(
            graph.omissions[0].examples,
            vec!["task T5 preserved unrecognized tier huge as medium"]
        );
    }

    #[test]
    fn extract_assignee_supports_ascii_and_unicode_arrows() {
        assert_eq!(
            extract_assignee("Fix launch -> worker-8"),
            (
                "Fix launch ".to_string(),
                Some("worker-8".to_string()),
                None,
                true,
            )
        );
        assert_eq!(
            extract_assignee("Fix launch \u{2192} worker-9"),
            (
                "Fix launch ".to_string(),
                Some("worker-9".to_string()),
                None,
                true,
            )
        );
    }

    #[test]
    fn assignee_normalizes_principal_and_preserves_display_label() {
        let mut counter = 0;
        let task = parse_task_line(
            "- [ ] [P1] T1: Implement completion truth -> P1 WS-A #126",
            &mut counter,
        )
        .expect("principal-bound task");

        assert_eq!(task.assignee.as_deref(), Some("P1"));
        assert_eq!(task.assignee_label.as_deref(), Some("WS-A #126"));
        assert!(task.assignee_recognized);

        let plan = super::SessionPlan {
            title: "Principal plan".to_string(),
            summary: String::new(),
            tasks: vec![task],
            generated_at: String::new(),
            raw_content: String::new(),
        };
        let graph = crate::orchestrator::work_graph::plan_parse::task_graph_from_plan(&plan);
        assert_eq!(
            graph.nodes[0].binding,
            crate::orchestrator::work_graph::BindingRef::Role("P1".to_string())
        );
    }

    #[test]
    fn unrecognized_assignee_is_preserved_as_plan_ready_warning() {
        let plan = super::parse_plan_markdown_checked(
            "# Plan\n\n## Tasks\n- [ ] T1: Custom lane -> Planner 1\n",
        )
        .expect("unrecognized bindings are warnings, not parse errors");
        assert_eq!(plan.tasks[0].assignee.as_deref(), Some("Planner 1"));
        assert!(!plan.tasks[0].assignee_recognized);

        let graph = crate::orchestrator::work_graph::plan_parse::task_graph_from_plan(&plan);
        assert_eq!(
            graph.nodes[0].binding,
            crate::orchestrator::work_graph::BindingRef::Role("Planner 1".to_string())
        );
        assert_eq!(graph.omissions.len(), 1);
        assert_eq!(
            graph.omissions[0].reason,
            crate::orchestrator::work_graph::WorkGraphOmissionReason::ResolutionIncomplete
        );
    }

    #[test]
    fn legacy_task_pipeline_pins_priority_then_assignee_order() {
        let mut counter = 0;
        let task = parse_task_line(
            "- [ ] [P1]   Fix launch regression   -> worker-8",
            &mut counter,
        )
        .expect("legacy checkbox task");

        assert_eq!(task.id, "task-1");
        assert_eq!(task.title, "Fix launch regression");
        assert_eq!(task.priority.as_deref(), Some("high"));
        assert_eq!(task.assignee.as_deref(), Some("worker-8"));
        assert_eq!(task.assignee_label, None);
    }

    #[test]
    fn legacy_extractors_pin_spacing_and_first_arrow_behavior() {
        assert_eq!(
            extract_priority("  Keep   internal   spacing  "),
            ("  Keep   internal   spacing  ".to_string(), None)
        );
        assert_eq!(
            extract_priority("[P2]  Normalize   only when marked"),
            (
                "Normalize only when marked".to_string(),
                Some("medium".to_string())
            )
        );
        assert_eq!(
            extract_assignee("Title -> worker-1 -> trailing"),
            (
                "Title ".to_string(),
                Some("worker-1".to_string()),
                Some("-> trailing".to_string()),
                true,
            )
        );
    }

    #[test]
    fn legacy_checkbox_plan_has_no_missing_stable_id_diagnostics() {
        let (plan, diagnostics) = parse_plan_markdown_with_diagnostics(
            "# Legacy\n\n## Tasks\n- [ ] First positional task\n- [ ] Second positional task\n",
        );

        assert_eq!(plan.tasks.len(), 2);
        assert!(plan.tasks.iter().all(|task| !task.explicit_id));
        assert!(diagnostics.is_empty());
    }
}

pub fn register(registry: &mut ActionRegistry) {
    registry.register(Box::new(QueenInject));
    registry.register(Box::new(QueenSwitchBranch));
    registry.register(Box::new(OperatorInject));
    registry.register(Box::new(ReportWorkerStatus));
    registry.register(Box::new(AddWorker));
    registry.register(Box::new(GetCoordinationLog));
    registry.register(Box::new(LogCoordinationMessage));
    registry.register(Box::new(GetWorkersState));
    registry.register(Box::new(AssignTask));
    registry.register(Box::new(GetSessionStoragePath));
    registry.register(Box::new(GetCurrentDirectory));
    registry.register(Box::new(ListStoredSessions));
    registry.register(Box::new(GetAppConfig));
    registry.register(Box::new(UpdateAppConfig));
    registry.register(Box::new(GetSessionPlan));
}
