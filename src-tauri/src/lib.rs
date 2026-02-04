mod commands;
mod pty;
mod session;

use std::sync::Arc;
use parking_lot::RwLock;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use commands::{
    create_pty, get_pty_status, kill_pty, list_ptys, resize_pty, write_to_pty,
    launch_hive, get_session, list_sessions, stop_session, stop_agent,
    PtyManagerState, SessionControllerState,
};
use pty::PtyManager;
use session::SessionController;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Create shared state
    let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
    let session_controller = Arc::new(RwLock::new(SessionController::new(Arc::clone(&pty_manager))));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .manage(PtyManagerState(Arc::clone(&pty_manager)))
        .manage(SessionControllerState(Arc::clone(&session_controller)))
        .setup(move |app| {
            // Set app handle for event emission
            {
                let mut controller = session_controller.write();
                controller.set_app_handle(app.handle().clone());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // PTY commands
            create_pty,
            write_to_pty,
            resize_pty,
            kill_pty,
            get_pty_status,
            list_ptys,
            // Session commands
            launch_hive,
            get_session,
            list_sessions,
            stop_session,
            stop_agent,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
