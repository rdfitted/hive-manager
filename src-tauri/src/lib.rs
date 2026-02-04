mod commands;
mod pty;
mod session;
mod storage;
mod coordination;
mod templates;
mod cli;

use std::sync::Arc;
use parking_lot::RwLock;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use commands::{
    create_pty, get_pty_status, kill_pty, list_ptys, resize_pty, write_to_pty, inject_to_pty,
    launch_hive, launch_hive_v2, launch_swarm, get_session, list_sessions, stop_session, stop_agent,
    queen_inject, operator_inject, add_worker_to_session, get_coordination_log, log_coordination_message,
    get_workers_state, assign_task, get_session_storage_path, list_stored_sessions,
    get_app_config, update_app_config, get_session_plan,
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
            get_session,
            list_sessions,
            stop_session,
            stop_agent,
            // Coordination commands
            queen_inject,
            operator_inject,
            add_worker_to_session,
            get_coordination_log,
            log_coordination_message,
            get_workers_state,
            assign_task,
            get_session_storage_path,
            list_stored_sessions,
            get_app_config,
            update_app_config,
            get_session_plan,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
