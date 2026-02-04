use tauri::State;
use std::sync::Arc;
use parking_lot::RwLock;

use crate::pty::{AgentRole, AgentStatus, PtyManager};

pub struct PtyManagerState(pub Arc<RwLock<PtyManager>>);

// PtyManagerState is Send + Sync because Arc<RwLock<T>> is Send + Sync when T is Send
unsafe impl Send for PtyManagerState {}
unsafe impl Sync for PtyManagerState {}

#[tauri::command]
pub async fn create_pty(
    state: State<'_, PtyManagerState>,
    id: String,
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let pty_manager = state.0.read();
    pty_manager
        .create_session(
            id,
            AgentRole::Worker { index: 0, parent: None },
            &command,
            &args_refs,
            cwd.as_deref(),
            cols,
            rows,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn write_to_pty(
    state: State<'_, PtyManagerState>,
    id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    let pty_manager = state.0.read();
    pty_manager.write(&id, &data).map_err(|e| e.to_string())
}

/// Write a string message to a PTY and optionally send Enter
#[tauri::command]
pub async fn inject_to_pty(
    state: State<'_, PtyManagerState>,
    id: String,
    message: String,
    send_enter: bool,
) -> Result<(), String> {
    let pty_manager = state.0.read();

    tracing::info!("inject_to_pty: id={}, message={:?}, send_enter={}", id, message, send_enter);

    if send_enter {
        // Send message + carriage return together
        // Use \r (0x0D) which is what xterm.js sends for Enter key
        let message_with_enter = format!("{}\r", message);
        tracing::info!("Sending message with CR (\\r) to {}: {:?}", id, message_with_enter.as_bytes());
        pty_manager.write(&id, message_with_enter.as_bytes()).map_err(|e| e.to_string())?;
    } else {
        // Write just the message
        pty_manager.write(&id, message.as_bytes()).map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub async fn resize_pty(
    state: State<'_, PtyManagerState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let pty_manager = state.0.read();
    pty_manager.resize(&id, cols, rows).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn kill_pty(
    state: State<'_, PtyManagerState>,
    id: String,
) -> Result<(), String> {
    let pty_manager = state.0.read();
    pty_manager.kill(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_pty_status(
    state: State<'_, PtyManagerState>,
    id: String,
) -> Result<Option<AgentStatus>, String> {
    let pty_manager = state.0.read();
    Ok(pty_manager.get_status(&id))
}

#[tauri::command]
pub async fn list_ptys(
    state: State<'_, PtyManagerState>,
) -> Result<Vec<(String, AgentRole, AgentStatus)>, String> {
    let pty_manager = state.0.read();
    Ok(pty_manager.list_sessions())
}
