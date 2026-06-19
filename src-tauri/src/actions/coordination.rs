//! Coordination and session-state actions behind the unified action registry.

use async_trait::async_trait;
use schemars::schema::RootSchema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::coordination::{CoordinationMessage, MessageType, StateManager, WorkerStateInfo};
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
    pub priority: Option<String>,
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
        let manager = ctx.state.injection_manager.read();
        manager
            .queen_inject(
                &request.session_id,
                &request.queen_id,
                &request.target_worker_id,
                &request.message,
            )
            .map_err(|e| ActionError::internal(e.to_string()))?;
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

        let manager = ctx.state.injection_manager.read();
        let results = manager
            .queen_switch_branch(
                &parsed.session_id,
                &parsed.queen_id,
                &worker_ids,
                &parsed.branch,
            )
            .map_err(|e| ActionError::internal(e.to_string()))?;

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
        let manager = ctx.state.injection_manager.read();
        manager
            .operator_inject(
                &request.session_id,
                &request.target_agent_id,
                &request.message,
            )
            .map_err(|e| ActionError::internal(e.to_string()))?;
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
        let coord_manager = ctx.state.injection_manager.read();
        coord_manager
            .queen_inject(
                &parsed.session_id,
                &parsed.queen_id,
                &parsed.worker_id,
                &parsed.task,
            )
            .map_err(|e| ActionError::internal(e.to_string()))?;

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

fn parse_plan_markdown(content: &str) -> SessionPlan {
    let mut title = String::new();
    let mut summary = String::new();
    let mut tasks: Vec<PlanTask> = Vec::new();
    let mut current_section = "";
    let mut task_counter = 0;

    for line in content.lines() {
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
            if let Some(task) = parse_task_line(trimmed, &mut task_counter) {
                tasks.push(task);
            }
        }
    }

    if title.is_empty() {
        title = "Plan in Progress...".to_string();
    }

    SessionPlan {
        title,
        summary,
        tasks,
        generated_at: chrono::Utc::now().to_rfc3339(),
        raw_content: content.to_string(),
    }
}

fn parse_task_line(line: &str, counter: &mut i32) -> Option<PlanTask> {
    let trimmed = line.trim();

    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
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
            return None;
        }
    } else {
        return None;
    };

    if rest.is_empty() {
        return None;
    }

    *counter += 1;
    let (title, priority) = extract_priority(rest);
    let (title, assignee) = extract_assignee(&title);

    Some(PlanTask {
        id: format!("task-{}", counter),
        title: title.trim().to_string(),
        description: String::new(),
        status: status.to_string(),
        assignee,
        priority,
    })
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

fn extract_assignee(text: &str) -> (String, Option<String>) {
    for separator in ["->", "\u{2192}"] {
        if let Some((title, assignee)) = text.split_once(separator) {
            return (title.to_string(), Some(assignee.trim().to_string()));
        }
    }

    (text.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::{extract_assignee, extract_priority};

    #[test]
    fn extract_priority_strips_detected_token_case_insensitively() {
        let (title, priority) = extract_priority("[High] Fix launch regression");

        assert_eq!(title, "Fix launch regression");
        assert_eq!(priority.as_deref(), Some("high"));
    }

    #[test]
    fn extract_assignee_supports_ascii_and_unicode_arrows() {
        assert_eq!(
            extract_assignee("Fix launch -> worker-8"),
            ("Fix launch ".to_string(), Some("worker-8".to_string()))
        );
        assert_eq!(
            extract_assignee("Fix launch \u{2192} worker-9"),
            ("Fix launch ".to_string(), Some("worker-9".to_string()))
        );
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
