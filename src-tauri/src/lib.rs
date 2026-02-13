mod commands;
mod pty;
mod session;
mod storage;
mod coordination;
mod templates;
pub mod cli;
mod http;
mod watcher;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use parking_lot::RwLock;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use crate::http::state::AppState;

use commands::{
    create_pty, get_pty_status, kill_pty, list_ptys, resize_pty, write_to_pty, inject_to_pty,
    launch_hive, launch_hive_v2, launch_swarm, launch_solo, launch_fusion, get_session, list_sessions, stop_session, stop_agent,
    continue_after_planning, mark_plan_ready, resume_session,
    queen_inject, queen_switch_branch, operator_inject, add_worker_to_session, get_coordination_log, log_coordination_message,
    get_workers_state, assign_task, get_session_storage_path, list_stored_sessions, get_current_directory,
    get_app_config, update_app_config, get_session_plan,
    list_branches, get_current_branch, switch_branch, git_pull, git_push, git_fetch,
    git_worktree_add, git_worktree_list, git_worktree_remove, git_worktree_prune,
    PtyManagerState, SessionControllerState, CoordinationState, StorageState,
};
use pty::PtyManager;
use session::SessionController;
use storage::SessionStorage;
use coordination::InjectionManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Initialize session storage
    let storage = Arc::new(SessionStorage::new().expect("Failed to initialize session storage"));
    let config = storage.load_config().expect("Failed to load config");
    let shared_config = Arc::new(tokio::sync::RwLock::new(config));

    // Create shared state
    let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
    let session_controller = Arc::new(RwLock::new(SessionController::new(Arc::clone(&pty_manager))));
    let injection_manager = Arc::new(RwLock::new(InjectionManager::new(
        Arc::clone(&pty_manager),
        SessionStorage::new().expect("Failed to initialize injection manager storage"),
    )));

    // Set storage on session controller
    {
        let mut controller = session_controller.write();
        controller.set_storage(Arc::clone(&storage));
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(PtyManagerState(Arc::clone(&pty_manager)))
        .manage(SessionControllerState(Arc::clone(&session_controller)))
        .manage(CoordinationState(Arc::clone(&injection_manager)))
        .manage(StorageState(Arc::clone(&storage)))
        .setup(move |app| {
            // Set app handle for event emission
            {
                let mut controller = session_controller.write();
                controller.set_app_handle(app.handle().clone());
            }
            {
                let mut injection = injection_manager.write();
                injection.set_app_handle(app.handle().clone());
            }

            // Stall detection background task - runs every 60s, emits agent-stalled/agent-recovered
            let stall_controller = session_controller.clone();
            let stall_app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let stall_threshold = Duration::from_secs(180); // 3 minutes
                let mut known_stalled: HashSet<(String, String)> = HashSet::new();
                let mut interval = tokio::time::interval(Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    let controller = stall_controller.read();
                    let sessions = controller.list_sessions();
                    let running_session_ids: Vec<String> = sessions
                        .iter()
                        .filter(|s| s.state == crate::session::SessionState::Running)
                        .map(|s| s.id.clone())
                        .collect();
                    drop(sessions);

                    let mut currently_stalled: HashSet<(String, String)> = HashSet::new();
                    for session_id in &running_session_ids {
                        let stalled = controller.get_stalled_agents(session_id, stall_threshold);
                        for (agent_id, last_activity) in stalled {
                            currently_stalled.insert((session_id.clone(), agent_id.clone()));
                        }
                    }
                    drop(controller);

                    // Emit agent-stalled for newly stalled
                    for (sid, aid) in &currently_stalled {
                        if !known_stalled.contains(&(sid.clone(), aid.clone())) {
                            let _ = stall_app_handle.emit("agent-stalled", serde_json::json!({
                                "session_id": sid,
                                "agent_id": aid,
                            }));
                        }
                    }
                    // Emit agent-recovered for no longer stalled
                    for (sid, aid) in known_stalled.iter() {
                        if !currently_stalled.contains(&(sid.clone(), aid.clone())) {
                            let _ = stall_app_handle.emit("agent-recovered", serde_json::json!({
                                "session_id": sid,
                                "agent_id": aid,
                            }));
                        }
                    }
                    known_stalled = currently_stalled;
                }
            });

            // Start HTTP server if enabled
            let http_config = shared_config.clone();
            let http_session_controller = session_controller.clone();
            tauri::async_runtime::spawn(async move {
                let (enabled, port) = {
                    let cfg = http_config.read().await;
                    (cfg.api.enabled, cfg.api.port)
                };

                if enabled {
                    tracing::info!("Starting HTTP API on port {}", port);
                    let state = Arc::new(AppState::new(
                        http_config,
                        pty_manager.clone(),
                        http_session_controller.clone(),
                        injection_manager.clone(),
                        storage.clone(),
                    ));
                    if let Err(e) = http::serve(state, port).await {
                        tracing::error!("HTTP server error: {}", e);
                    }
                }
            });

            // Set up worker-completed event listener for sequential spawning
            let session_controller_clone = session_controller.clone();
            use tauri::Listener;
            app.listen("worker-completed", move |event: tauri::Event| {
                let payload = event.payload();

                // Parse the payload (session_id, worker_id, task_file)
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) {
                    let session_id = json.get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let worker_id = json.get("worker_id")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u8;

                    if session_id.is_empty() || worker_id == 0 {
                        tracing::warn!("Invalid worker-completed payload: {}", payload);
                        return;
                    }

                    tracing::info!("Worker {} completed, spawning next worker", worker_id);

                    // Spawn async task to handle worker completion
                    let controller = session_controller_clone.clone();
                    let session_id_clone = session_id.to_string();
                    tauri::async_runtime::spawn_blocking(move || {
                        let result = tauri::async_runtime::block_on(async {
                            let controller_read = controller.read();
                            controller_read.on_worker_completed(&session_id_clone, worker_id).await
                        });

                        if let Err(e) = result {
                            tracing::error!("Failed to handle worker completion: {}", e);
                        }
                    });
                } else {
                    tracing::warn!("Failed to parse worker-committed payload: {}", payload);
                }
            });

            // Set up fusion-variant-completed event listener for judge spawning
            let fusion_controller_clone = session_controller.clone();
            app.listen("fusion-variant-completed", move |event: tauri::Event| {
                let payload = event.payload();

                if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) {
                    let session_id = json.get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let variant_index = json.get("variant_index")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u8;

                    if session_id.is_empty() || variant_index == 0 {
                        tracing::warn!("Invalid fusion-variant-completed payload: {}", payload);
                        return;
                    }

                    tracing::info!(
                        "Fusion variant {} completed for session {}, checking judge trigger",
                        variant_index,
                        session_id
                    );

                    let controller = fusion_controller_clone.clone();
                    let session_id_clone = session_id.to_string();
                    tauri::async_runtime::spawn_blocking(move || {
                        let result = tauri::async_runtime::block_on(async {
                            let controller_read = controller.read();
                            controller_read
                                .on_fusion_variant_completed(&session_id_clone, variant_index)
                                .await
                        });

                        if let Err(e) = result {
                            tracing::error!("Failed to handle fusion variant completion: {}", e);
                        }
                    });
                } else {
                    tracing::warn!("Failed to parse fusion-variant-completed payload: {}", payload);
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // PTY commands
            create_pty,
            write_to_pty,
            inject_to_pty,
            resize_pty,
            kill_pty,
            get_pty_status,
            list_ptys,
            // Session commands
            launch_hive,
            launch_hive_v2,
            launch_swarm,
            launch_solo,
            launch_fusion,
            get_session,
            list_sessions,
            stop_session,
            stop_agent,
            // Coordination commands
            queen_inject,
            queen_switch_branch,
            operator_inject,
            add_worker_to_session,
            get_coordination_log,
            log_coordination_message,
            get_workers_state,
            assign_task,
            get_session_storage_path,
            list_stored_sessions,
            get_current_directory,
            get_app_config,
            update_app_config,
            get_session_plan,
            // Git commands
            list_branches,
            get_current_branch,
            switch_branch,
            git_pull,
            git_push,
            git_fetch,
            git_worktree_add,
            git_worktree_list,
            git_worktree_remove,
            git_worktree_prune,
            // Planning phase commands
            continue_after_planning,
            mark_plan_ready,
            resume_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
