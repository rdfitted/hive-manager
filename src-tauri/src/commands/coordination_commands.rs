use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::coordination::{
    CoordinationMessage, InjectionManager, StateManager, WorkerStateInfo,
};
use crate::pty::{AgentConfig, WorkerRole};
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
        .record_assignment(&worker_id, &task)
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

/// List stored sessions
#[tauri::command]
pub async fn list_stored_sessions(
    storage_state: State<'_, StorageState>,
) -> Result<Vec<crate::storage::SessionSummary>, String> {
    storage_state.0.list_sessions().map_err(|e| e.to_string())
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
