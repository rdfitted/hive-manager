use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::RwLock;
use serde_json::json;
use tauri::State;

use crate::actions::{ActionContext, ActionRegistry, Caller};
use crate::http::handlers::{validate_cli, validate_project_path};
use crate::http::state::AppState;
use crate::pty::AgentConfig;
use crate::session::{Session, SessionController, HiveLaunchConfig, ResearchLaunchConfig, SwarmLaunchConfig, FusionLaunchConfig};

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

const SESSION_COLOR_ALLOWLIST: &[&str] = &[
    "#7aa2f7",
    "#bb9af7",
    "#9ece6a",
    "#e0af68",
    "#7dcfff",
    "#f7768e",
    "#ff9e64",
    "#f7b1d1",
];

// SessionControllerState is Send + Sync because Arc<RwLock<T>> is Send + Sync when T is Send
unsafe impl Send for SessionControllerState {}
unsafe impl Sync for SessionControllerState {}

fn validate_session_name(name: Option<&str>) -> Result<(), String> {
    let Some(name) = name else {
        return Ok(());
    };

    if name.trim().is_empty() {
        return Err("Invalid session name: must not be empty or whitespace".to_string());
    }

    if name.chars().count() > 64 {
        return Err("Invalid session name: must be 64 characters or fewer".to_string());
    }

    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err("Invalid session name: must not contain '..', '/', or '\\'".to_string());
    }

    Ok(())
}

fn validate_session_color(color: Option<&str>) -> Result<(), String> {
    let Some(color) = color else {
        return Ok(());
    };

    if !SESSION_COLOR_ALLOWLIST.contains(&color) && !is_valid_hex_session_color(color) {
        return Err(format!(
            "Invalid session color '{}'. Valid options: {} or any #RRGGBB hex color",
            color,
            SESSION_COLOR_ALLOWLIST.join(", ")
        ));
    }

    Ok(())
}

fn is_valid_hex_session_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color.chars().skip(1).all(|c| c.is_ascii_hexdigit())
}

fn validate_research_launch_config(config: &ResearchLaunchConfig) -> Result<(), String> {
    validate_project_path(&config.project_path).map_err(|e| e.message.clone())?;
    validate_session_name(config.name.as_deref())?;
    validate_session_color(config.color.as_deref())?;
    validate_cli(&config.queen_config.cli).map_err(|e| e.message.clone())?;

    // Research requires a roster of 1..=6 researchers. These are NOT pre-spawned:
    // the Queen spawns them on demand, so the roster must list at least one entry for
    // the Queen to draw from (and is capped like the launch dialog does at 6).
    if !(1..=6).contains(&config.workers.len()) {
        return Err(format!(
            "Research sessions require 1 to 6 researchers (got {}).",
            config.workers.len()
        ));
    }

    for worker in &config.workers {
        validate_cli(&worker.cli).map_err(|e| e.message.clone())?;
    }

    Ok(())
}

fn validate_swarm_launch_config(config: &SwarmLaunchConfig) -> Result<(), String> {
    validate_project_path(&config.project_path).map_err(|e| e.message.clone())?;
    validate_session_name(config.name.as_deref())?;
    validate_session_color(config.color.as_deref())?;
    validate_cli(&config.queen_config.cli).map_err(|e| e.message.clone())?;
    validate_cli(&config.planner_config.cli).map_err(|e| e.message.clone())?;

    for worker in &config.workers_per_planner {
        validate_cli(&worker.cli).map_err(|e| e.message.clone())?;
    }

    for planner in &config.planners {
        validate_cli(&planner.config.cli).map_err(|e| e.message.clone())?;
        for worker in &planner.workers {
            validate_cli(&worker.cli).map_err(|e| e.message.clone())?;
        }
    }

    if let Some(evaluator_config) = &config.evaluator_config {
        if evaluator_config.cli.trim().is_empty() {
            // Empty nested CLI means "inherit session default"; only validate explicit overrides.
        } else {
        validate_cli(&evaluator_config.cli).map_err(|e| e.message.clone())?;
        }
    }

    if let Some(qa_workers) = &config.qa_workers {
        for qa_worker in qa_workers {
            validate_cli(&qa_worker.cli).map_err(|e| e.message.clone())?;
            match qa_worker.specialization.as_str() {
                "ui" | "api" | "a11y" => {}
                other => {
                    return Err(format!(
                        "Invalid QA specialization '{}'. Valid options: ui, api, a11y",
                        other
                    ));
                }
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn launch_hive(
    state: State<'_, SessionControllerState>,
    project_path: String,
    worker_count: u8,
    command: String,
    prompt: Option<String>,
) -> Result<Session, String> {
    let controller = state.0.read();
    controller.launch_hive(
        PathBuf::from(project_path),
        worker_count,
        &command,
        prompt,
        None,
        None,
    )
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
    dispatch_frontend(
        &registry,
        Arc::clone(&app_state),
        "session.list",
        json!({}),
    )
    .await
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
    state: State<'_, SessionControllerState>,
    config: ResearchLaunchConfig,
) -> Result<Session, String> {
    validate_research_launch_config(&config)?;
    let controller = state.0.read();
    controller.launch_research(config)
}

#[tauri::command]
pub async fn launch_swarm(
    state: State<'_, SessionControllerState>,
    config: SwarmLaunchConfig,
) -> Result<Session, String> {
    validate_swarm_launch_config(&config)?;
    let controller = state.0.read();
    controller.launch_swarm(config)
}

#[tauri::command]
pub async fn launch_solo(
    state: State<'_, SessionControllerState>,
    project_path: String,
    task_description: Option<String>,
    cli: String,
    model: Option<String>,
    flags: Option<Vec<String>>,
    evaluator_cli: Option<String>,
    evaluator_model: Option<String>,
) -> Result<Session, String> {
    validate_project_path(&project_path).map_err(|e| e.message.clone())?;
    validate_cli(&cli).map_err(|e| e.message.clone())?;

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
        validate_cli(eval_cli).map_err(|e| e.message.clone())?;
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

    let controller = state.0.read();
    controller.launch_solo(config)
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
    if session_id.contains("..") || session_id.contains('/') || session_id.contains('\\') {
        return Err("Invalid session ID format".to_string());
    }
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
