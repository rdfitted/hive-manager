//! Tauri `#[command]` wrappers for git operations.
//!
//! The real git logic (including the load-bearing Windows `CREATE_NO_WINDOW`
//! flag) now lives in `crate::actions::git`. These wrappers preserve the exact
//! command names + signatures the frontend `invoke()`s and dispatch the
//! corresponding action through the shared registry with `caller = Frontend`.

use std::sync::Arc;

use serde_json::json;
use tauri::State;

use crate::actions::{ActionContext, ActionRegistry, Caller};
use crate::http::state::AppState;

// Re-export the git value types from the action module so any existing importer
// of `commands::git_commands::{BranchInfo, WorktreeInfo}` keeps compiling.
pub use crate::actions::git::{BranchInfo, WorktreeInfo};

/// Dispatch a git action with `caller = Frontend`, surfacing the action's
/// message string on error (matching prior `Result<_, String>` behavior), and
/// deserializing the JSON output into the typed return.
async fn dispatch_git<T: serde::de::DeserializeOwned>(
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
pub async fn list_branches(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: String,
) -> Result<Vec<BranchInfo>, String> {
    dispatch_git(
        &registry,
        Arc::clone(&app_state),
        "git.list_branches",
        json!({ "project_path": project_path }),
    )
    .await
}

#[tauri::command]
pub async fn get_current_branch(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: String,
) -> Result<String, String> {
    dispatch_git(
        &registry,
        Arc::clone(&app_state),
        "git.current_branch",
        json!({ "project_path": project_path }),
    )
    .await
}

#[tauri::command]
pub async fn switch_branch(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: String,
    branch: String,
) -> Result<(), String> {
    dispatch_git(
        &registry,
        Arc::clone(&app_state),
        "git.switch_branch",
        json!({ "project_path": project_path, "branch": branch }),
    )
    .await
}

#[tauri::command]
pub async fn git_pull(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: String,
) -> Result<String, String> {
    dispatch_git(
        &registry,
        Arc::clone(&app_state),
        "git.pull",
        json!({ "project_path": project_path }),
    )
    .await
}

#[tauri::command]
pub async fn git_push(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: String,
) -> Result<String, String> {
    dispatch_git(
        &registry,
        Arc::clone(&app_state),
        "git.push",
        json!({ "project_path": project_path }),
    )
    .await
}

#[tauri::command]
pub async fn git_fetch(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: String,
) -> Result<String, String> {
    dispatch_git(
        &registry,
        Arc::clone(&app_state),
        "git.fetch",
        json!({ "project_path": project_path }),
    )
    .await
}

#[tauri::command]
pub async fn git_worktree_add(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: String,
    worktree_path: String,
    branch: String,
) -> Result<(), String> {
    dispatch_git(
        &registry,
        Arc::clone(&app_state),
        "git.worktree_add",
        json!({
            "project_path": project_path,
            "worktree_path": worktree_path,
            "branch": branch
        }),
    )
    .await
}

#[tauri::command]
pub async fn git_worktree_list(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: String,
) -> Result<Vec<WorktreeInfo>, String> {
    dispatch_git(
        &registry,
        Arc::clone(&app_state),
        "git.worktree_list",
        json!({ "project_path": project_path }),
    )
    .await
}

#[tauri::command]
pub async fn git_worktree_remove(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: String,
    worktree_path: String,
) -> Result<(), String> {
    dispatch_git(
        &registry,
        Arc::clone(&app_state),
        "git.worktree_remove",
        json!({ "project_path": project_path, "worktree_path": worktree_path }),
    )
    .await
}

#[tauri::command]
pub async fn git_worktree_prune(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: String,
) -> Result<(), String> {
    dispatch_git(
        &registry,
        Arc::clone(&app_state),
        "git.worktree_prune",
        json!({ "project_path": project_path }),
    )
    .await
}
