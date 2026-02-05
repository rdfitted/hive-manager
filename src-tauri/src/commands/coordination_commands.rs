use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::coordination::{
    CoordinationMessage, InjectionManager, StateManager, WorkerStateInfo,
};
use crate::pty::{AgentConfig, AgentRole, WorkerRole};
use crate::session::AgentInfo;
use crate::storage::SessionStorage;

/// State wrapper for coordination
pub struct CoordinationState(pub Arc<RwLock<InjectionManager>>);

/// State wrapper for storage
pub struct StorageState(pub Arc<SessionStorage>);

/// Request to inject a message from Queen to a worker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueenInjectRequest {
    pub session_id: String,
    pub queen_id: String,
    pub target_worker_id: String,
    pub message: String,
}

/// Request to add a worker to a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddWorkerRequest {
    pub session_id: String,
    pub config: AgentConfig,
    pub role: WorkerRole,
    pub parent_id: Option<String>,
}

/// Queen injects a message to a worker
#[tauri::command]
pub async fn queen_inject(
    state: State<'_, CoordinationState>,
    request: QueenInjectRequest,
) -> Result<(), String> {
    let manager = state.0.read();
    manager
        .queen_inject(
            &request.session_id,
            &request.queen_id,
            &request.target_worker_id,
            &request.message,
        )
        .map_err(|e: crate::coordination::InjectionError| e.to_string())
}

/// Queen initiates a branch switch for all workers
#[tauri::command]
pub async fn queen_switch_branch(
    session_id: String,
    queen_id: String,
    branch: String,
    state: State<'_, CoordinationState>,
    session_state: State<'_, super::SessionControllerState>,
) -> Result<Vec<(String, bool)>, String> {
    let worker_ids = {
        let controller = session_state.0.read();
        controller
            .get_session(&session_id)
            .map(|s| {
                s.agents
                    .iter()
                    .filter(|a| matches!(a.role, AgentRole::Worker { .. }))
                    .map(|a| a.id.clone())
                    .collect::<Vec<_>>()
            })
            .ok_or("Session not found")?
    };

    let manager = state.0.read();
    let results = manager
        .queen_switch_branch(&session_id, &queen_id, &worker_ids, &branch)
        .map_err(|e| e.to_string())?;

    Ok(results
        .into_iter()
        .map(|(id, r)| (id, r.is_ok()))
        .collect())
}

/// Request for operator injection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInjectRequest {
    pub session_id: String,
    pub target_agent_id: String,
    pub message: String,
}

/// Request for worker status notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerStatusRequest {
    pub session_id: String,
    pub queen_id: String,
    pub worker_id: String,
    pub status: String,
}

/// Operator injects a message to any agent (including Queen)
#[tauri::command]
pub async fn operator_inject(
    state: State<'_, CoordinationState>,
    request: OperatorInjectRequest,
) -> Result<(), String> {
    let manager = state.0.read();
    manager
        .operator_inject(
            &request.session_id,
            &request.target_agent_id,
            &request.message,
        )
        .map_err(|e: crate::coordination::InjectionError| e.to_string())
}

/// Report worker status change to Queen
#[tauri::command]
pub async fn report_worker_status(
    state: State<'_, CoordinationState>,
    request: WorkerStatusRequest,
) -> Result<(), String> {
    let manager = state.0.read();
    manager
        .notify_queen_worker_status(
            &request.session_id,
            &request.queen_id,
            &request.worker_id,
            &request.status,
        )
        .map_err(|e: crate::coordination::InjectionError| e.to_string())
}

/// Add a worker to an existing session
#[tauri::command]
pub async fn add_worker_to_session(
    session_state: State<'_, super::SessionControllerState>,
    coord_state: State<'_, CoordinationState>,
    storage_state: State<'_, StorageState>,
    request: AddWorkerRequest,
) -> Result<AgentInfo, String> {
    let controller = session_state.0.write();

    // Add worker through session controller
    let agent_info = controller
        .add_worker(
            &request.session_id,
            request.config,
            request.role.clone(),
            request.parent_id,
        )
        .map_err(|e| e.to_string())?;

    // Notify Queen about new worker
    let coord_manager = coord_state.0.read();

    // Find Queen ID
    let queen_id = format!("{}-queen", request.session_id);

    // Create worker state info for notification
    let worker_state = WorkerStateInfo {
        id: agent_info.id.clone(),
        role: request.role,
        cli: agent_info.config.cli.clone(),
        status: "Running".to_string(),
        current_task: None,
        last_update: chrono::Utc::now(),
    };

    // Notify Queen
    let _ = coord_manager.notify_queen_worker_added(&request.session_id, &queen_id, &worker_state);

    // Update workers.md
    let session_path = storage_state.0.session_dir(&request.session_id);
    let state_manager = StateManager::new(session_path);

    // Get all current workers and update the file
    if let Some(session) = controller.get_session(&request.session_id) {
        let workers: Vec<WorkerStateInfo> = session
            .agents
            .iter()
            .filter(|a| !matches!(a.role, crate::pty::AgentRole::Queen))
            .map(|a| WorkerStateInfo {
                id: a.id.clone(),
                role: a.config.role.clone().unwrap_or_default(),
                cli: a.config.cli.clone(),
                status: format!("{:?}", a.status),
                current_task: None,
                last_update: chrono::Utc::now(),
            })
            .collect();

        let _ = state_manager.update_workers_file(&workers);
    }

    Ok(agent_info)
}

/// Get the coordination log for a session
#[tauri::command]
pub async fn get_coordination_log(
    state: State<'_, CoordinationState>,
    session_id: String,
    limit: Option<usize>,
) -> Result<Vec<CoordinationMessage>, String> {
    let manager = state.0.read();
    manager
        .get_coordination_log(&session_id, limit)
        .map_err(|e: crate::coordination::InjectionError| e.to_string())
}

/// Log a system message to coordination
#[tauri::command]
pub async fn log_coordination_message(
    state: State<'_, CoordinationState>,
    session_id: String,
    _from: String,
    to: String,
    content: String,
) -> Result<(), String> {
    let manager = state.0.read();
    manager
        .log_system_message(&session_id, &to, &content)
        .map_err(|e: crate::coordination::InjectionError| e.to_string())
}

/// Get workers state for a session
#[tauri::command]
pub async fn get_workers_state(
    storage_state: State<'_, StorageState>,
    session_id: String,
) -> Result<Vec<WorkerStateInfo>, String> {
    let session_path = storage_state.0.session_dir(&session_id);
    let state_manager = StateManager::new(session_path);
    state_manager
        .read_workers_file()
        .map_err(|e: crate::coordination::StateError| e.to_string())
}

/// Record a task assignment
#[tauri::command]
pub async fn assign_task(
    coord_state: State<'_, CoordinationState>,
    storage_state: State<'_, StorageState>,
    session_id: String,
    queen_id: String,
    worker_id: String,
    task: String,
    plan_task_id: Option<String>,
) -> Result<(), String> {
    // Log the injection
    let coord_manager = coord_state.0.read();
    coord_manager
        .queen_inject(&session_id, &queen_id, &worker_id, &task)
        .map_err(|e: crate::coordination::InjectionError| e.to_string())?;

    // Record the assignment
    let session_path = storage_state.0.session_dir(&session_id);
    let state_manager = StateManager::new(session_path);
    state_manager
        .record_assignment(&worker_id, &task, plan_task_id)
        .map_err(|e: crate::coordination::StateError| e.to_string())
}

/// Get session storage path
#[tauri::command]
pub async fn get_session_storage_path(
    storage_state: State<'_, StorageState>,
    session_id: String,
) -> Result<String, String> {
    let path = storage_state.0.session_dir(&session_id);
    Ok(path.to_string_lossy().to_string())
}

/// Get current working directory
#[tauri::command]
pub async fn get_current_directory() -> Result<String, String> {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

/// List stored sessions, optionally filtered by project path
#[tauri::command]
pub async fn list_stored_sessions(
    storage_state: State<'_, StorageState>,
    project_path: Option<String>,
) -> Result<Vec<crate::storage::SessionSummary>, String> {
    let sessions = storage_state.0.list_sessions().map_err(|e| e.to_string())?;

    match project_path {
        Some(path) => {
            // Normalize paths for comparison (handle trailing slashes, case on Windows)
            let normalize = |p: &str| -> String {
                let p = p.trim_end_matches(['/', '\\']);
                #[cfg(windows)]
                { p.to_lowercase() }
                #[cfg(not(windows))]
                { p.to_string() }
            };

            let target = normalize(&path);
            Ok(sessions.into_iter()
                .filter(|s| normalize(&s.project_path) == target)
                .collect())
        }
        None => Ok(sessions),
    }
}

/// Get app config
#[tauri::command]
pub async fn get_app_config(
    storage_state: State<'_, StorageState>,
) -> Result<crate::storage::AppConfig, String> {
    storage_state.0.load_config().map_err(|e| e.to_string())
}

/// Update app config
#[tauri::command]
pub async fn update_app_config(
    storage_state: State<'_, StorageState>,
    config: crate::storage::AppConfig,
) -> Result<(), String> {
    storage_state.0.save_config(&config).map_err(|e| e.to_string())
}

/// Plan task structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub assignee: Option<String>,
    pub priority: Option<String>,
}

/// Session plan structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPlan {
    pub title: String,
    pub summary: String,
    pub tasks: Vec<PlanTask>,
    pub generated_at: String,
    pub raw_content: String,  // Raw markdown content for display
}

/// Get the session plan (parsed from plan.md)
/// Looks in project-local .hive-manager/{session_id}/plan.md first,
/// then falls back to app storage
#[tauri::command]
pub async fn get_session_plan(
    session_state: State<'_, super::SessionControllerState>,
    storage_state: State<'_, StorageState>,
    session_id: String,
) -> Result<Option<SessionPlan>, String> {
    // First, try to get the project path from the active session
    let project_plan_path = {
        let controller = session_state.0.read();
        if let Some(session) = controller.get_session(&session_id) {
            let project_path = &session.project_path;
            Some(project_path.join(".hive-manager").join(&session_id).join("plan.md"))
        } else {
            None
        }
    };

    // Try project-local path first
    let plan_path = if let Some(ref path) = project_plan_path {
        if path.exists() {
            path.clone()
        } else {
            // Fall back to app storage
            let session_path = storage_state.0.session_dir(&session_id);
            session_path.join("plan.md")
        }
    } else {
        // No session found, try app storage
        let session_path = storage_state.0.session_dir(&session_id);
        session_path.join("plan.md")
    };

    if !plan_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&plan_path)
        .map_err(|e| format!("Failed to read plan.md: {}", e))?;

    // Parse the plan.md content, include raw content
    let plan = parse_plan_markdown(&content);
    Ok(Some(plan))
}

/// Parse plan.md markdown content into structured plan
/// Never fails - returns what it can parse, includes raw content
fn parse_plan_markdown(content: &str) -> SessionPlan {
    let mut title = String::new();
    let mut summary = String::new();
    let mut tasks: Vec<PlanTask> = Vec::new();
    let mut current_section = "";
    let mut task_counter = 0;

    for line in content.lines() {
        let trimmed = line.trim();

        // Parse title (first H1)
        if trimmed.starts_with("# ") && title.is_empty() {
            title = trimmed[2..].trim().to_string();
            continue;
        }

        // Detect sections
        if trimmed.starts_with("## ") {
            let section_name = trimmed[3..].trim().to_lowercase();
            if section_name.contains("summary") || section_name.contains("overview") {
                current_section = "summary";
            } else if section_name.contains("task") || section_name.contains("plan") {
                current_section = "tasks";
            } else {
                current_section = "";
            }
            continue;
        }

        // Parse summary
        if current_section == "summary" && !trimmed.is_empty() && !trimmed.starts_with("#") {
            if !summary.is_empty() {
                summary.push(' ');
            }
            summary.push_str(trimmed);
            continue;
        }

        // Parse tasks (look for list items or numbered items)
        if current_section == "tasks" {
            // Match patterns like: - [ ] Task, - [x] Task, 1. Task, - Task
            if let Some(task) = parse_task_line(trimmed, &mut task_counter) {
                tasks.push(task);
            }
        }
    }

    // If no title found, use "Plan in Progress"
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

/// Parse a single task line
fn parse_task_line(line: &str, counter: &mut i32) -> Option<PlanTask> {
    let trimmed = line.trim();

    // Skip empty lines and headers
    if trimmed.is_empty() || trimmed.starts_with("#") {
        return None;
    }

    // Check for checkbox format: - [ ] or - [x]
    let (status, rest) = if trimmed.starts_with("- [ ]") || trimmed.starts_with("* [ ]") {
        ("pending", trimmed[5..].trim())
    } else if trimmed.starts_with("- [x]") || trimmed.starts_with("* [x]")
           || trimmed.starts_with("- [X]") || trimmed.starts_with("* [X]") {
        ("completed", trimmed[5..].trim())
    } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        ("pending", trimmed[2..].trim())
    } else if trimmed.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        // Numbered list: 1. Task
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

    // Extract priority from brackets like [HIGH], [P1], etc.
    let (title, priority) = extract_priority(rest);

    // Extract assignee from arrow notation: -> Worker 1
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

/// Extract priority from task title
fn extract_priority(text: &str) -> (String, Option<String>) {
    let priorities = [
        ("[HIGH]", "high"), ("[P1]", "high"), ("[CRITICAL]", "high"),
        ("[MEDIUM]", "medium"), ("[P2]", "medium"), ("[MED]", "medium"),
        ("[LOW]", "low"), ("[P3]", "low"),
    ];

    for (marker, priority) in priorities {
        if text.to_uppercase().contains(marker) {
            let cleaned = text.replace(marker, "").replace(&marker.to_lowercase(), "");
            return (cleaned, Some(priority.to_string()));
        }
    }

    (text.to_string(), None)
}

/// Extract assignee from task title
fn extract_assignee(text: &str) -> (String, Option<String>) {
    // Look for patterns like "-> Worker 1" or "→ Queen"
    if let Some(pos) = text.find("->") {
        let (title, assignee) = text.split_at(pos);
        return (title.to_string(), Some(assignee[2..].trim().to_string()));
    }
    if let Some(pos) = text.find("→") {
        let (title, assignee) = text.split_at(pos);
        return (title.to_string(), Some(assignee[3..].trim().to_string())); // → is 3 bytes
    }

    (text.to_string(), None)
}
