#[cfg(not(test))]
mod commands;
pub mod actions;
pub mod adapters;
pub mod artifacts;
pub mod cli;
mod coordination;
pub mod domain;
pub mod events;
mod http;
pub mod orchestrator;
mod pty;
pub mod runtime;
mod session;
mod storage;
mod tauri_shim;
mod templates;
mod watcher;
pub mod workspace;

#[cfg(not(test))]
use std::collections::HashSet;
#[cfg(not(test))]
use std::sync::Arc;
#[cfg(not(test))]
use std::time::Duration;
#[cfg(not(test))]
use parking_lot::RwLock;
#[cfg(not(test))]
use crate::actions::ActionRegistry;
#[cfg(not(test))]
use crate::domain::event::EventType;
#[cfg(not(test))]
use crate::http::handlers::cells::build_cells;
#[cfg(not(test))]
use crate::http::state::AppState;
#[cfg(not(test))]
use tauri::{Emitter, Manager};
#[cfg(not(test))]
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(not(test))]
use commands::{
    add_worker_to_session, assign_task, close_session, continue_after_planning, create_pty,
    get_app_config, get_coordination_log, get_current_branch, get_current_directory,
    get_pty_status, get_run_journal, get_session, get_session_plan, get_session_storage_path,
    get_workers_state, git_fetch, git_pull, git_push, git_worktree_add, git_worktree_list,
    git_worktree_prune, git_worktree_remove, inject_to_pty, kill_pty, launch_debate, launch_fusion,
    launch_hive, launch_hive_v2, launch_research, launch_solo, launch_swarm, list_branches,
    list_ptys, list_session_files, list_sessions, list_stored_sessions, log_coordination_message,
    mark_plan_ready, operator_inject, paste_to_pty, queen_inject, queen_switch_branch, resize_pty,
    resume_session, stop_agent, stop_session, switch_branch, update_app_config,
    update_session_metadata, write_to_pty, CoordinationState, PtyManagerState,
    SessionControllerState, StorageState,
};
#[cfg(not(test))]
use pty::PtyManager;
#[cfg(not(test))]
use session::SessionController;
#[cfg(not(test))]
use storage::{ApplicationStateDb, SessionStorage};
#[cfg(not(test))]
use coordination::InjectionManager;
#[cfg(not(test))]
use events::EventBus;

#[cfg(not(test))]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Initialize session storage
    let storage = Arc::new(SessionStorage::new().expect("Failed to initialize session storage"));

    // Initialize the SQLite application_state DB alongside file storage (runs migrations
    // idempotently). Shared via Arc onto AppState for HTTP + downstream subsystems.
    let app_state_db = Arc::new(
        ApplicationStateDb::open(storage.base_dir())
            .expect("Failed to initialize application_state db"),
    );

    let config = storage.load_config().expect("Failed to load config");
    let shared_config = Arc::new(tokio::sync::RwLock::new(config));
    let event_bus = EventBus::new(storage.base_dir().clone());

    // Create shared state
    let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
    let session_controller = Arc::new(RwLock::new(SessionController::new(Arc::clone(
        &pty_manager,
    ))));
    let injection_manager = Arc::new(RwLock::new(InjectionManager::new(
        Arc::clone(&pty_manager),
        SessionStorage::new().expect("Failed to initialize injection manager storage"),
    )));

    // #125: build the run journal + ledger store on the shared SQLite DB and ensure its
    // tables exist (idempotent CREATE TABLE IF NOT EXISTS, run once at startup — NOT a
    // register_migration hook). Threaded into the controller by construction so the
    // write-step seams can record/resume.
    let run_journal_store = crate::storage::RunJournalStore::new(Arc::clone(&app_state_db));
    run_journal_store
        .ensure_schema()
        .expect("Failed to initialize run_journal schema");

    // #126: build the durable sub-agent run queue on the same shared SQLite DB and ensure
    // its `agent_run_queue` table exists (idempotent CREATE TABLE IF NOT EXISTS, run once at
    // startup). The QueueManager wraps the repo + EventBus and is the source of truth for
    // queued/running/finalized workers; Session.agents stays a UI cache.
    let queue_repo = Arc::new(crate::storage::QueueRepo::new(Arc::clone(&app_state_db)));
    queue_repo
        .ensure_schema()
        .expect("Failed to initialize agent_run_queue schema");
    let queue_manager = Arc::new(crate::coordination::QueueManager::new(
        Arc::clone(&queue_repo),
        Arc::clone(&event_bus),
    ));

    // Set storage on session controller
    {
        let mut controller = session_controller.write();
        controller.set_storage(Arc::clone(&storage));
        controller.set_event_bus(Arc::clone(&event_bus));
        controller.set_run_journal(run_journal_store.clone());
    }

    // Unified action registry — the single registration point shared by the
    // Tauri #[command] wrappers (caller=Frontend) and the HTTP layer (caller=Http).
    let action_registry: Arc<ActionRegistry> = Arc::new(crate::actions::build_registry());

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
        .manage(Arc::clone(&action_registry))
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

            // Build the SINGLE shared AppState now that the app handle exists, and
            // hand the SAME Arc to both the Tauri-managed state (used by migrated
            // #[command]s via the action registry) and the HTTP server below.
            // This unifies the two former state holders onto one Arc<AppState>.
            let app_state = Arc::new(AppState::new(
                shared_config.clone(),
                Arc::clone(&pty_manager),
                Arc::clone(&session_controller),
                Arc::clone(&injection_manager),
                Arc::clone(&storage),
                Arc::clone(&event_bus),
                Arc::clone(&app_state_db),
                Arc::clone(&queue_manager),
                Some(app.handle().clone()),
            ));
            // Attach the shared registry so HTTP handlers can dispatch actions.
            app_state.set_registry(Arc::clone(&action_registry));
            app.manage(Arc::clone(&app_state));

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
                        .filter(|s| s.state.is_monitorable())
                        .map(|s| s.id.clone())
                        .collect();
                    drop(sessions);

                    let mut currently_stalled: HashSet<(String, String)> = HashSet::new();
                    for session_id in &running_session_ids {
                        let stalled = controller.get_stalled_agents(session_id, stall_threshold);
                        for (agent_id, _last_activity) in stalled {
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

            // #126: durable run-queue maintenance — every 30s, reclaim stuck running rows
            // (heartbeat older than STUCK_CUTOFF flips back to 'queued', emits
            // WorkerReclaimed) and finalize no-progress / continuation-exceeded runs (emits
            // WorkerFinalized). Reclaim never kills a live PTY; it only marks a row claimable.
            let maintenance_queue_manager = Arc::clone(&queue_manager);
            let maintenance_app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(30));
                loop {
                    interval.tick().await;
                    match maintenance_queue_manager.run_maintenance().await {
                        Ok(()) => {
                            // Nudge the frontend store to refetch /queue after a maintenance pass.
                            let _ = maintenance_app_handle.emit("queue-updated", serde_json::json!({}));
                        }
                        Err(e) => tracing::warn!("Queue maintenance pass failed: {e}"),
                    }
                }
            });

            let cell_event_controller = session_controller.clone();
            let cell_event_storage = storage.clone();
            let cell_event_bus = event_bus.clone();
            let cell_event_app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut receiver = cell_event_bus.subscribe();

                let emit_cell_updates_for_sessions = |session_ids: &mut HashSet<String>| {
                    for session_id in session_ids.drain() {
                        let session = {
                            let controller = cell_event_controller.read();
                            controller.get_session(&session_id)
                        };

                        if let Some(session) = session {
                            let payload = serde_json::json!({
                                "session_id": session_id,
                                "cells": build_cells(&session, &cell_event_storage),
                            });
                            let _ = cell_event_app_handle.emit("cell-updated", payload);
                        }
                    }
                };

                loop {
                    let event = match receiver.recv().await {
                        Ok(event) => event,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let mut pending_sessions = {
                                let controller = cell_event_controller.read();
                                controller
                                    .list_sessions()
                                    .into_iter()
                                    .filter(|session| session.state.is_monitorable())
                                    .map(|session| session.id)
                                    .collect::<HashSet<_>>()
                            };
                            emit_cell_updates_for_sessions(&mut pending_sessions);
                            continue;
                        }
                    };

                    if !matches!(
                        event.event_type,
                        EventType::AgentLaunched
                            | EventType::AgentCompleted
                            | EventType::AgentWaitingInput
                            | EventType::AgentFailed
                            | EventType::ArtifactUpdated
                            | EventType::CellCreated
                            | EventType::CellStatusChanged
                            | EventType::WorkspaceCreated
                    ) {
                        continue;
                    }

                    let mut pending_sessions = HashSet::from([event.session_id]);

                    tokio::time::sleep(Duration::from_millis(50)).await;

                    let mut closed = false;

                    loop {
                        match receiver.try_recv() {
                            Ok(event) => {
                                if matches!(
                                    event.event_type,
                                    EventType::AgentLaunched
                                        | EventType::AgentCompleted
                                        | EventType::AgentWaitingInput
                                        | EventType::AgentFailed
                                        | EventType::ArtifactUpdated
                                        | EventType::CellCreated
                                        | EventType::CellStatusChanged
                                        | EventType::WorkspaceCreated
                                ) {
                                    pending_sessions.insert(event.session_id);
                                }
                            }
                            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                                closed = true;
                                break;
                            }
                            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                                let controller = cell_event_controller.read();
                                pending_sessions.extend(
                                    controller
                                        .list_sessions()
                                        .into_iter()
                                        .filter(|session| session.state.is_monitorable())
                                        .map(|session| session.id),
                                );
                                break;
                            }
                        }
                    }

                    emit_cell_updates_for_sessions(&mut pending_sessions);

                    if closed {
                        break;
                    }
                }
            });

            // Start HTTP server if enabled, sharing the SAME Arc<AppState> the
            // Tauri command surface uses so both surfaces see identical state.
            // (The app_state_db from #124 is already folded into this unified
            // Arc<AppState>, so the HTTP server sees the same SQLite layer.)
            let http_state = Arc::clone(&app_state);
            tauri::async_runtime::spawn(async move {
                let (enabled, port) = {
                    let cfg = http_state.config.read().await;
                    (cfg.api.enabled, cfg.api.port)
                };

                if enabled {
                    tracing::info!("Starting HTTP API on port {}", port);
                    if let Err(e) = http::serve(http_state, port).await {
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

            let debate_controller_clone = session_controller.clone();
            app.listen("debate-round-completed", move |event: tauri::Event| {
                let payload = event.payload();

                if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) {
                    let session_id = json.get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let debater_index = json.get("debater_index")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u8;
                    let round = json.get("round")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u8;

                    if session_id.is_empty() || debater_index == 0 || round == 0 {
                        tracing::warn!("Invalid debate-round-completed payload: {}", payload);
                        return;
                    }

                    tracing::info!(
                        "Debate debater {} completed round {} for session {}, checking next step",
                        debater_index,
                        round,
                        session_id
                    );

                    let controller = debate_controller_clone.clone();
                    let session_id_clone = session_id.to_string();
                    tauri::async_runtime::spawn_blocking(move || {
                        let result = tauri::async_runtime::block_on(async {
                            let controller_read = controller.read();
                            controller_read
                                .on_debate_round_completed(&session_id_clone, debater_index, round)
                                .await
                        });

                        if let Err(e) = result {
                            tracing::error!("Failed to handle debate round completion: {}", e);
                        }
                    });
                } else {
                    tracing::warn!("Failed to parse debate-round-completed payload: {}", payload);
                }
            });

            let milestone_controller_clone = session_controller.clone();
            app.listen("milestone-ready", move |event: tauri::Event| {
                let payload = event.payload();

                if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) {
                    let session_id = json
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if session_id.is_empty() {
                        tracing::warn!("Invalid milestone-ready payload: {}", payload);
                        return;
                    }

                    tracing::info!(
                        "Milestone-ready signal observed for session {}, checking evaluator launch/respawn",
                        session_id
                    );

                    let controller = milestone_controller_clone.clone();
                    let session_id_clone = session_id.to_string();
                    tauri::async_runtime::spawn_blocking(move || {
                        let controller_read = controller.read();
                        if let Err(err) = controller_read.on_milestone_ready(&session_id_clone) {
                            tracing::error!(
                                "Failed to handle milestone-ready signal for {}: {}",
                                session_id_clone,
                                err
                            );
                        }
                    });
                } else {
                    tracing::warn!("Failed to parse milestone-ready payload: {}", payload);
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // PTY commands
            create_pty,
            write_to_pty,
            paste_to_pty,
            inject_to_pty,
            resize_pty,
            kill_pty,
            get_pty_status,
            list_ptys,
            // Session commands
            launch_hive,
            launch_hive_v2,
            launch_research,
            launch_swarm,
            launch_solo,
            launch_fusion,
            launch_debate,
            get_session,
            list_sessions,
            stop_session,
            close_session,
            stop_agent,
            update_session_metadata,
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
            get_run_journal,
            list_session_files,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
pub fn run() {}
