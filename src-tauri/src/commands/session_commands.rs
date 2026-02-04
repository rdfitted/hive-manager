use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::RwLock;
use tauri::State;

use crate::session::{Session, SessionController};

pub struct SessionControllerState(pub Arc<RwLock<SessionController>>);

// SessionControllerState is Send + Sync because Arc<RwLock<T>> is Send + Sync when T is Send
unsafe impl Send for SessionControllerState {}
unsafe impl Sync for SessionControllerState {}

#[tauri::command]
pub async fn launch_hive(
    state: State<'_, SessionControllerState>,
    project_path: String,
    worker_count: u8,
    prompt: Option<String>,
) -> Result<Session, String> {
    let controller = state.0.read();
    controller.launch_hive(PathBuf::from(project_path), worker_count, prompt)
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
