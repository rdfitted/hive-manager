use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::RwLock;
use tauri::State;

use crate::http::handlers::{validate_cli, validate_project_path};
use crate::pty::AgentConfig;
use crate::session::{Session, SessionController, HiveLaunchConfig, SwarmLaunchConfig, FusionLaunchConfig};

pub struct SessionControllerState(pub Arc<RwLock<SessionController>>);

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

fn validate_hive_launch_config(config: &HiveLaunchConfig) -> Result<(), String> {
    validate_project_path(&config.project_path).map_err(|e| e.message.clone())?;
    validate_session_name(config.name.as_deref())?;
    validate_session_color(config.color.as_deref())?;
    validate_cli(&config.queen_config.cli).map_err(|e| e.message.clone())?;

    for worker in &config.workers {
        validate_cli(&worker.cli).map_err(|e| e.message.clone())?;
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
pub async fn close_session(
    state: State<'_, SessionControllerState>,
    id: String,
) -> Result<(), String> {
    let controller = state.0.read();
    controller.close_session(&id)
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
    validate_hive_launch_config(&config)?;
    let controller = state.0.read();
    controller.launch_hive_v2(config)
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
    session_id: String,
) -> Result<Session, String> {
    let controller = state.0.read();
    controller.resume_session(&session_id)
}

#[tauri::command]
pub async fn update_session_metadata(
    state: State<'_, SessionControllerState>,
    id: String,
    name: Option<Option<String>>,
    color: Option<Option<String>>,
) -> Result<Session, String> {
    validate_session_name(name.as_ref().and_then(|value| value.as_deref()))?;
    validate_session_color(color.as_ref().and_then(|value| value.as_deref()))?;

    let controller = state.0.read();
    controller.update_session_metadata(&id, name, color)
}
