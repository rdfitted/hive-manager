use std::sync::Arc;
use tokio::sync::RwLock;
use parking_lot::RwLock as PLRwLock;
use tauri::{AppHandle, Emitter};

use crate::domain::event::{Event, EventType, Severity};
use crate::storage::{AppConfig, SessionStorage};
use crate::storage::ConversationMessage;
use crate::pty::PtyManager;
use crate::session::SessionController;
use crate::coordination::InjectionManager;
use crate::events::EventBus;

#[allow(dead_code)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub pty_manager: Arc<PLRwLock<PtyManager>>,
    pub session_controller: Arc<PLRwLock<SessionController>>,
    pub injection_manager: Arc<PLRwLock<InjectionManager>>,
    pub storage: Arc<SessionStorage>,
    pub event_bus: Arc<EventBus>,
    pub app_handle: Option<AppHandle>,
}

impl AppState {
    pub fn new(
        config: Arc<RwLock<AppConfig>>,
        pty_manager: Arc<PLRwLock<PtyManager>>,
        session_controller: Arc<PLRwLock<SessionController>>,
        injection_manager: Arc<PLRwLock<InjectionManager>>,
        storage: Arc<SessionStorage>,
        event_bus: Arc<EventBus>,
        app_handle: Option<AppHandle>,
    ) -> Self {
        Self {
            config,
            pty_manager,
            session_controller,
            injection_manager,
            storage,
            event_bus,
            app_handle,
        }
    }

    pub async fn emit_conversation_message(
        &self,
        session_id: &str,
        agent_id: &str,
        message: &ConversationMessage,
    ) -> Result<(), String> {
        if let Some(app_handle) = self.app_handle.as_ref() {
            app_handle
                .emit("conversation-message", serde_json::json!({
                    "agent_id": agent_id,
                    "timestamp": message.timestamp,
                    "from": message.from,
                    "content": message.content,
                }))
                .map_err(|error| format!("Failed to emit Tauri conversation message: {error}"))?;
        }

        self.event_bus
            .publish(Event {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: session_id.to_string(),
                cell_id: None,
                agent_id: Some(agent_id.to_string()),
                event_type: EventType::ConversationMessage,
                timestamp: message.timestamp,
                payload: serde_json::json!({
                    "timestamp": message.timestamp,
                    "from": message.from,
                    "content": message.content,
                }),
                severity: Severity::Info,
            })
            .await
    }
}
