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

/// Maximum allowed paste size (5MB) - prevents DoS via oversized pastes
const MAX_PASTE_SIZE: usize = 5 * 1024 * 1024;

#[tauri::command]
pub async fn write_to_pty(
    state: State<'_, PtyManagerState>,
    id: String,
    data: String,
) -> Result<(), String> {
    // Check size limit for DoS prevention
    if data.len() > MAX_PASTE_SIZE {
        return Err(format!(
            "Paste size {} bytes exceeds maximum allowed {} bytes",
            data.len(),
            MAX_PASTE_SIZE
        ));
    }

    let pty_manager = state.0.read();

    // Wrap in bracketed paste mode for proper terminal handling
    pty_manager.write_bracketed(&id, data.as_bytes()).map_err(|e| e.to_string())
}

/// Write a string message to a PTY and optionally send Enter
#[tauri::command]
pub async fn inject_to_pty(
    state: State<'_, PtyManagerState>,
    id: String,
    message: String,
    send_enter: bool,
) -> Result<(), String> {
    // Check size limit for DoS prevention
    if message.len() > MAX_PASTE_SIZE {
        return Err(format!(
            "Message size {} bytes exceeds maximum allowed {} bytes",
            message.len(),
            MAX_PASTE_SIZE
        ));
    }

    let pty_manager = state.0.read();

    tracing::info!("inject_to_pty: id={}, message={:?}, send_enter={}", id, message, send_enter);

    if send_enter {
        // For messages with Enter, we send message with bracketed paste + CR
        let message_with_enter = format!("{}\r", message);
        pty_manager.write_bracketed(&id, message_with_enter.as_bytes()).map_err(|e| e.to_string())?;
    } else {
        // Use bracketed paste for consistency
        pty_manager.write_bracketed(&id, message.as_bytes()).map_err(|e| e.to_string())?;
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
