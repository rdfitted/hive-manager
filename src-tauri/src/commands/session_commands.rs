use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::RwLock;
use tauri::State;

use crate::session::{Session, SessionController, HiveLaunchConfig, SwarmLaunchConfig, FusionLaunchConfig};

pub struct SessionControllerState(pub Arc<RwLock<SessionController>>);

// SessionControllerState is Send + Sync because Arc<RwLock<T>> is Send + Sync when T is Send
unsafe impl Send for SessionControllerState {}
unsafe impl Sync for SessionControllerState {}

#[tauri::command]
pub async fn launch_hive(
    state: State<'_, SessionControllerState>,
    project_path: String,
    worker_count: u8,
    command: String,
    prompt: Option<String>,
) -> Result<Session, String> {
    let controller = state.0.read();
    controller.launch_hive(PathBuf::from(project_path), worker_count, &command, prompt)
}

#[tauri::command]
pub async fn get_session(
    state: State<'_, SessionControllerState>,
    id: String,
) -> Result<Option<Session>, String> {
    let controller = state.0.read();
    Ok(controller.get_session(&id))
}

#[tauri::command]
pub async fn list_sessions(
    state: State<'_, SessionControllerState>,
) -> Result<Vec<Session>, String> {
    let controller = state.0.read();
    Ok(controller.list_sessions())
}

#[tauri::command]
pub async fn stop_session(
    state: State<'_, SessionControllerState>,
    id: String,
) -> Result<(), String> {
    let controller = state.0.read();
    controller.stop_session(&id)
}

#[tauri::command]
pub async fn stop_agent(
    state: State<'_, SessionControllerState>,
    session_id: String,
    agent_id: String,
) -> Result<(), String> {
    let controller = state.0.read();
    controller.stop_agent(&session_id, &agent_id)
}

#[tauri::command]
pub async fn launch_hive_v2(
    state: State<'_, SessionControllerState>,
    config: HiveLaunchConfig,
) -> Result<Session, String> {
    let controller = state.0.read();
    controller.launch_hive_v2(config)
}

#[tauri::command]
pub async fn launch_swarm(
    state: State<'_, SessionControllerState>,
    config: SwarmLaunchConfig,
) -> Result<Session, String> {
    let controller = state.0.read();
    controller.launch_swarm(config)
}

#[tauri::command]
pub async fn launch_fusion(
    state: State<'_, SessionControllerState>,
    config: FusionLaunchConfig,
) -> Result<Session, String> {
    let controller = state.0.read();
    controller.launch_fusion(config)
}

#[tauri::command]
pub async fn continue_after_planning(
    state: State<'_, SessionControllerState>,
    session_id: String,
) -> Result<Session, String> {
    let controller = state.0.read();
    controller.continue_after_planning(&session_id)
}

#[tauri::command]
pub async fn mark_plan_ready(
    state: State<'_, SessionControllerState>,
    session_id: String,
) -> Result<(), String> {
    let controller = state.0.read();
    controller.mark_plan_ready(&session_id)
}

#[tauri::command]
pub async fn resume_session(
    state: State<'_, SessionControllerState>,
    session_id: String,
) -> Result<Session, String> {
    let controller = state.0.read();
    controller.resume_session(&session_id)
}
