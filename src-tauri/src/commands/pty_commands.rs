use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::Arc;
use tauri::State;

use crate::actions::{ActionContext, ActionRegistry, Caller};
use crate::http::state::AppState;
use crate::pty::{AgentRole, AgentStatus, PtyManager};

#[allow(dead_code)]
pub struct PtyManagerState(pub Arc<RwLock<PtyManager>>);

// PtyManagerState is Send + Sync because Arc<RwLock<T>> is Send + Sync when T is Send
unsafe impl Send for PtyManagerState {}
unsafe impl Sync for PtyManagerState {}

async fn dispatch_pty<T: DeserializeOwned>(
    registry: &ActionRegistry,
    state: Arc<AppState>,
    name: &str,
    input: serde_json::Value,
) -> Result<T, String> {
    let ctx = ActionContext::new(Caller::Frontend, state);
    let value = registry
        .dispatch(name, &ctx, input)
        .await
        .map_err(|e| e.to_message())?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_pty(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    id: String,
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    dispatch_pty(
        &registry,
        Arc::clone(&app_state),
        "pty.create",
        json!({
            "id": id,
            "command": command,
            "args": args,
            "cwd": cwd,
            "cols": cols,
            "rows": rows,
        }),
    )
    .await
}

#[tauri::command]
pub async fn write_to_pty(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    id: String,
    data: String,
) -> Result<(), String> {
    dispatch_pty(
        &registry,
        Arc::clone(&app_state),
        "pty.write",
        json!({ "id": id, "data": data }),
    )
    .await
}

#[tauri::command]
pub async fn paste_to_pty(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    id: String,
    data: String,
) -> Result<(), String> {
    dispatch_pty(
        &registry,
        Arc::clone(&app_state),
        "pty.paste",
        json!({ "id": id, "data": data }),
    )
    .await
}

/// Write a string message to a PTY and optionally send Enter
#[tauri::command]
pub async fn inject_to_pty(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    id: String,
    message: String,
    send_enter: bool,
) -> Result<(), String> {
    dispatch_pty(
        &registry,
        Arc::clone(&app_state),
        "pty.inject",
        json!({ "id": id, "message": message, "send_enter": send_enter }),
    )
    .await
}

#[tauri::command]
pub async fn resize_pty(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    dispatch_pty(
        &registry,
        Arc::clone(&app_state),
        "pty.resize",
        json!({ "id": id, "cols": cols, "rows": rows }),
    )
    .await
}

#[tauri::command]
pub async fn kill_pty(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), String> {
    dispatch_pty(
        &registry,
        Arc::clone(&app_state),
        "pty.kill",
        json!({ "id": id }),
    )
    .await
}

#[tauri::command]
pub async fn get_pty_status(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<Option<AgentStatus>, String> {
    dispatch_pty(
        &registry,
        Arc::clone(&app_state),
        "pty.status",
        json!({ "id": id }),
    )
    .await
}

#[tauri::command]
pub async fn list_ptys(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
) -> Result<Vec<(String, AgentRole, AgentStatus)>, String> {
    dispatch_pty(&registry, Arc::clone(&app_state), "pty.list", json!({})).await
}
