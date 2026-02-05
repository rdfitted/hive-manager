use std::sync::Arc;
use tokio::sync::RwLock;
use parking_lot::RwLock as PLRwLock;
use crate::storage::{AppConfig, SessionStorage};
use crate::pty::PtyManager;
use crate::session::SessionController;
use crate::coordination::InjectionManager;

#[allow(dead_code)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub pty_manager: Arc<PLRwLock<PtyManager>>,
    pub session_controller: Arc<PLRwLock<SessionController>>,
    pub injection_manager: Arc<PLRwLock<InjectionManager>>,
    pub storage: Arc<SessionStorage>,
}

impl AppState {
    pub fn new(
        config: Arc<RwLock<AppConfig>>,
        pty_manager: Arc<PLRwLock<PtyManager>>,
        session_controller: Arc<PLRwLock<SessionController>>,
        injection_manager: Arc<PLRwLock<InjectionManager>>,
        storage: Arc<SessionStorage>,
    ) -> Self {
        Self {
            config,
            pty_manager,
            session_controller,
            injection_manager,
            storage,
        }
    }
}
