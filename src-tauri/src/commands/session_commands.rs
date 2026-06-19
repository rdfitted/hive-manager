use parking_lot::RwLock;
use serde_json::json;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::State;

use crate::actions::{ActionContext, ActionRegistry, Caller};
use crate::http::state::AppState;
use crate::pty::AgentConfig;
use crate::session::{
    DebateLaunchConfig, FusionLaunchConfig, HiveLaunchConfig, ResearchLaunchConfig, Session,
    SessionController, SwarmLaunchConfig,
};

pub struct SessionControllerState(pub Arc<RwLock<SessionController>>);

/// Dispatch an action through the shared registry with `caller = Frontend`,
/// returning the raw JSON value or the action's message string (the exact text
/// the frontend `invoke()` already expects on error).
async fn dispatch_frontend(
    registry: &ActionRegistry,
    state: Arc<AppState>,
    name: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let ctx = ActionContext::new(Caller::Frontend, state);
    registry
        .dispatch(name, &ctx, input)
        .await
        .map_err(|e| e.to_message())
}

// SessionControllerState is Send + Sync because Arc<RwLock<T>> is Send + Sync when T is Send
unsafe impl Send for SessionControllerState {}
unsafe impl Sync for SessionControllerState {}

const SESSION_FILE_RESULT_CAP: usize = 100;
const SESSION_FILE_VISIT_CAP: usize = 5_000;

fn validate_session_id_for_command(session_id: &str) -> Result<(), String> {
    if session_id.contains("..") || session_id.contains('/') || session_id.contains('\\') {
        return Err("Invalid session ID format".to_string());
    }
    Ok(())
}

fn is_ignored_file_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        name,
        ".git" | ".hive-manager" | ".svelte-kit" | "node_modules" | "target" | "dist" | "build"
    )
}

fn path_within_any_root(path: &Path, canonical_roots: &[PathBuf]) -> bool {
    canonical_roots.iter().any(|root| path.starts_with(root))
}

fn dedupe_canonical_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut canonical_roots = Vec::new();

    for root in roots {
        let Ok(canonical) = fs::canonicalize(root) else {
            continue;
        };
        let key = canonical.to_string_lossy().to_lowercase();
        if seen.insert(key) {
            canonical_roots.push(canonical);
        }
    }

    canonical_roots
}

fn file_matches_query(path: &Path, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    let path_text = path.to_string_lossy().to_lowercase();
    let name_text = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_lowercase();
    path_text.contains(query) || name_text.contains(query)
}

fn list_files_under_roots(roots: Vec<PathBuf>, query: String) -> Result<Vec<String>, String> {
    let canonical_roots = dedupe_canonical_roots(roots);
    let query = query.trim().to_lowercase();
    let mut pending: VecDeque<PathBuf> = canonical_roots.iter().cloned().collect();
    let mut results = Vec::new();
    let mut visited = 0usize;

    while let Some(dir) = pending.pop_front() {
        if visited >= SESSION_FILE_VISIT_CAP || results.len() >= SESSION_FILE_RESULT_CAP {
            break;
        }
        visited += 1;

        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            if results.len() >= SESSION_FILE_RESULT_CAP || visited >= SESSION_FILE_VISIT_CAP {
                break;
            }

            let path = entry.path();
            let Ok(canonical_path) = fs::canonicalize(&path) else {
                continue;
            };
            if !path_within_any_root(&canonical_path, &canonical_roots) {
                continue;
            }

            let Ok(file_type) = entry.file_type() else {
                continue;
            };

            if file_type.is_dir() {
                if !is_ignored_file_dir(&path) {
                    pending.push_back(path);
                }
                continue;
            }

            if file_type.is_file() && file_matches_query(&path, &query) {
                results.push(path.to_string_lossy().to_string());
            }
        }
    }

    Ok(results)
}

#[tauri::command]
pub async fn launch_hive(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: String,
    worker_count: u8,
    command: String,
    prompt: Option<String>,
) -> Result<serde_json::Value, String> {
    dispatch_frontend(
        &registry,
        Arc::clone(&app_state),
        "session.launch_hive",
        json!({
            "project_path": project_path,
            "worker_count": worker_count,
            "command": command,
            "task_description": prompt,
        }),
    )
    .await
}

// NOTE on return types: the migrated session commands return `serde_json::Value`
// rather than the typed `Session` (which is `Serialize`-only, not `Deserialize`).
// The action layer already serialized the typed result; Tauri serializes this
// `Value` to byte-identical JSON, so the frontend `invoke()` wire contract is
// unchanged — only the Rust-side return type differs.
#[tauri::command]
pub async fn get_session(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<serde_json::Value, String> {
    dispatch_frontend(
        &registry,
        Arc::clone(&app_state),
        "session.get",
        json!({ "id": id }),
    )
    .await
}

#[tauri::command]
pub async fn list_sessions(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value, String> {
    dispatch_frontend(&registry, Arc::clone(&app_state), "session.list", json!({})).await
}

#[tauri::command]
pub async fn stop_session(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), String> {
    dispatch_frontend(
        &registry,
        Arc::clone(&app_state),
        "session.stop",
        json!({ "id": id }),
    )
    .await?;
    Ok(())
}

#[tauri::command]
pub async fn close_session(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), String> {
    dispatch_frontend(
        &registry,
        Arc::clone(&app_state),
        "session.close",
        json!({ "id": id }),
    )
    .await?;
    Ok(())
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
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    config: HiveLaunchConfig,
) -> Result<serde_json::Value, String> {
    let input = serde_json::to_value(config).map_err(|e| e.to_string())?;
    dispatch_frontend(
        &registry,
        Arc::clone(&app_state),
        "session.launch_hive_v2",
        input,
    )
    .await
}

#[tauri::command]
pub async fn launch_research(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    config: ResearchLaunchConfig,
) -> Result<serde_json::Value, String> {
    let input = serde_json::to_value(config).map_err(|e| e.to_string())?;
    dispatch_frontend(
        &registry,
        Arc::clone(&app_state),
        "session.launch_research",
        input,
    )
    .await
}

#[tauri::command]
pub async fn launch_swarm(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    config: SwarmLaunchConfig,
) -> Result<serde_json::Value, String> {
    let input = serde_json::to_value(config).map_err(|e| e.to_string())?;
    dispatch_frontend(
        &registry,
        Arc::clone(&app_state),
        "session.launch_swarm",
        input,
    )
    .await
}

#[tauri::command]
pub async fn launch_solo(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    project_path: String,
    task_description: Option<String>,
    cli: String,
    model: Option<String>,
    flags: Option<Vec<String>>,
    evaluator_cli: Option<String>,
    evaluator_model: Option<String>,
) -> Result<serde_json::Value, String> {
    let agent_config = AgentConfig {
        cli: cli.clone(),
        model,
        flags: flags.unwrap_or_default(),
        label: None,
        name: None,
        description: None,
        role: None,
        initial_prompt: None,
    };

    // Build evaluator_config: validate if provided, else fall back to cli silently
    let evaluator_config = if let Some(ref eval_cli) = evaluator_cli {
        Some(AgentConfig {
            cli: eval_cli.clone(),
            model: evaluator_model,
            flags: vec![],
            label: Some("Evaluator".to_string()),
            name: None,
            description: None,
            role: None,
            initial_prompt: None,
        })
    } else {
        None
    };
    let with_evaluator = evaluator_config.is_some();

    let config = HiveLaunchConfig {
        project_path,
        name: None,
        color: None,
        queen_config: agent_config,
        workers: vec![],
        prompt: task_description.filter(|t| !t.trim().is_empty()),
        with_planning: false,
        with_evaluator,
        evaluator_config,
        qa_workers: None,
        smoke_test: false,
    };

    let input = serde_json::to_value(config).map_err(|e| e.to_string())?;
    dispatch_frontend(
        &registry,
        Arc::clone(&app_state),
        "session.launch_solo",
        input,
    )
    .await
}

#[tauri::command]
pub async fn launch_fusion(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    config: FusionLaunchConfig,
) -> Result<serde_json::Value, String> {
    let input = serde_json::to_value(config).map_err(|e| e.to_string())?;
    dispatch_frontend(
        &registry,
        Arc::clone(&app_state),
        "session.launch_fusion",
        input,
    )
    .await
}

#[tauri::command]
pub async fn launch_debate(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    config: DebateLaunchConfig,
) -> Result<serde_json::Value, String> {
    let input = serde_json::to_value(config).map_err(|e| e.to_string())?;
    dispatch_frontend(
        &registry,
        Arc::clone(&app_state),
        "session.launch_debate",
        input,
    )
    .await
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
    app_state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Session, String> {
    let session = {
        let controller = state.0.read();
        controller.resume_session(&session_id)?
    };

    // #126: repair queue rows orphaned by the crash. Any `agent_run_queue` row still marked
    // `running` whose worker is NOT among the resumed session's live agents (its PTY did not
    // survive) is flipped back to `queued` so it becomes claimable again. The queue table
    // persisted across the restart on its own; reconcile only fixes orphaned `running` rows.
    let live_worker_ids: Vec<String> = session.agents.iter().map(|a| a.id.clone()).collect();
    if let Err(e) = app_state
        .queue_manager
        .reconcile(&session_id, &live_worker_ids)
        .await
    {
        tracing::warn!("Queue reconcile on resume failed for {session_id}: {e}");
    }

    Ok(session)
}

/// #125: read the run journal + side-effect ledger for a session, for the resume modal.
#[tauri::command]
pub async fn get_run_journal(
    app_state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<serde_json::Value, String> {
    validate_session_id_for_command(&session_id)?;
    let store = crate::storage::RunJournalStore::new(Arc::clone(&app_state.app_state_db));
    let journal = store
        .read_journal(&session_id)
        .map_err(|e| format!("Failed to read run journal: {e}"))?;
    let ledger = store
        .read_ledger(&session_id)
        .map_err(|e| format!("Failed to read run ledger: {e}"))?;
    Ok(json!({ "journal": journal, "ledger": ledger }))
}

#[tauri::command]
pub async fn list_session_files(
    state: State<'_, SessionControllerState>,
    session_id: String,
    query: String,
) -> Result<Vec<String>, String> {
    validate_session_id_for_command(&session_id)?;

    let roots = {
        let controller = state.0.read();
        let session = controller
            .get_session(&session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        let mut roots = vec![session.project_path];
        if let Some(worktree_path) = session.worktree_path {
            roots.push(PathBuf::from(worktree_path));
        }
        roots
    };

    tauri::async_runtime::spawn_blocking(move || list_files_under_roots(roots, query))
        .await
        .map_err(|e| format!("Failed to list session files: {e}"))?
}

#[tauri::command]
pub async fn update_session_metadata(
    registry: State<'_, Arc<ActionRegistry>>,
    app_state: State<'_, Arc<AppState>>,
    id: String,
    name: Option<Option<String>>,
    color: Option<Option<String>>,
) -> Result<serde_json::Value, String> {
    dispatch_frontend(
        &registry,
        Arc::clone(&app_state),
        "session.update_metadata",
        json!({ "id": id, "name": name, "color": color }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::path_within_any_root;
    use std::path::PathBuf;

    #[test]
    fn path_scope_rejects_sibling_paths() {
        let root = std::env::temp_dir().join("hm-session-root");
        let inside = root.join("src").join("main.rs");
        let outside = std::env::temp_dir()
            .join("hm-session-root-sibling")
            .join("main.rs");

        assert!(path_within_any_root(&inside, &[PathBuf::from(&root)]));
        assert!(!path_within_any_root(&outside, &[root]));
    }
}
