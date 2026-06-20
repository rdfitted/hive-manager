use std::sync::Arc;

use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use serde_json::json;
use tauri::State;

use crate::actions::{ActionContext, ActionRegistry, Caller};
use crate::coordination::{CoordinationMessage, InjectionManager, WorkerStateInfo};
use crate::http::state::AppState;
use crate::session::AgentInfo;
use crate::storage::SessionStorage;

#[allow(unused_imports)]
pub use crate::actions::coordination::{
    AddWorkerRequest, OperatorInjectRequest, PlanTask, QueenInjectRequest, SessionPlan,
    WorkerStatusRequest,
};

/// State wrapper for coordination.
#[allow(dead_code)]
pub struct CoordinationState(pub Arc<RwLock<InjectionManager>>);

/// State wrapper for storage.
#[allow(dead_code)]
pub struct StorageState(pub Arc<SessionStorage>);

async fn dispatch_coordination<T: DeserializeOwned>(
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
pub async fn queen_inject(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    request: QueenInjectRequest,
) -> Result<(), String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.queen_inject",
        json!(request),
    )
    .await
}

#[tauri::command]
pub async fn queen_switch_branch(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    session_id: String,
    queen_id: String,
    branch: String,
) -> Result<Vec<(String, bool)>, String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.queen_switch_branch",
        json!({
            "session_id": session_id,
            "queen_id": queen_id,
            "branch": branch,
        }),
    )
    .await
}

#[tauri::command]
pub async fn operator_inject(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    request: OperatorInjectRequest,
) -> Result<(), String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.operator_inject",
        json!(request),
    )
    .await
}

#[allow(dead_code)]
#[tauri::command]
pub async fn report_worker_status(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    request: WorkerStatusRequest,
) -> Result<(), String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.report_worker_status",
        json!(request),
    )
    .await
}

#[tauri::command]
pub async fn add_worker_to_session(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    request: AddWorkerRequest,
) -> Result<AgentInfo, String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.add_worker",
        json!(request),
    )
    .await
}

#[tauri::command]
pub async fn get_coordination_log(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    session_id: String,
    limit: Option<usize>,
) -> Result<Vec<CoordinationMessage>, String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.get_log",
        json!({ "session_id": session_id, "limit": limit }),
    )
    .await
}

#[tauri::command]
pub async fn log_coordination_message(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    session_id: String,
    from: String,
    to: String,
    content: String,
) -> Result<(), String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.log_message",
        json!({
            "session_id": session_id,
            "from": from,
            "to": to,
            "content": content,
        }),
    )
    .await
}

#[tauri::command]
pub async fn get_workers_state(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<WorkerStateInfo>, String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.get_workers_state",
        json!({ "session_id": session_id }),
    )
    .await
}

#[tauri::command]
pub async fn assign_task(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    session_id: String,
    queen_id: String,
    worker_id: String,
    task: String,
    plan_task_id: Option<String>,
) -> Result<(), String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.assign_task",
        json!({
            "session_id": session_id,
            "queen_id": queen_id,
            "worker_id": worker_id,
            "task": task,
            "plan_task_id": plan_task_id,
        }),
    )
    .await
}

#[tauri::command]
pub async fn get_session_storage_path(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<String, String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.get_session_storage_path",
        json!({ "session_id": session_id }),
    )
    .await
}

#[tauri::command]
pub async fn get_current_directory(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.get_current_directory",
        json!({}),
    )
    .await
}

#[tauri::command]
pub async fn list_stored_sessions(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: Option<String>,
) -> Result<Vec<crate::storage::SessionSummary>, String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.list_stored_sessions",
        json!({ "project_path": project_path }),
    )
    .await
}

#[tauri::command]
pub async fn get_app_config(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
) -> Result<crate::storage::AppConfig, String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.get_app_config",
        json!({}),
    )
    .await
}

#[tauri::command]
pub async fn update_app_config(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    config: crate::storage::AppConfig,
) -> Result<(), String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.update_app_config",
        json!({ "config": config }),
    )
    .await
}

#[tauri::command]
pub async fn get_session_plan(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Option<SessionPlan>, String> {
    dispatch_coordination(
        &registry,
        Arc::clone(&app_state),
        "coordination.get_session_plan",
        json!({ "session_id": session_id }),
    )
    .await
}
