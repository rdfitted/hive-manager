use parking_lot::RwLock as PLRwLock;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use crate::actions::render::envelope_for_content;
use crate::actions::ActionRegistry;
use crate::coordination::{InjectionManager, QueueManager};
use crate::domain::event::{Event, EventType, Severity};
use crate::events::EventBus;
use crate::pty::PtyManager;
use crate::session::SessionController;
use crate::storage::ConversationMessage;
use crate::storage::{AppConfig, ApplicationStateDb, SessionStorage};

#[allow(dead_code)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub pty_manager: Arc<PLRwLock<PtyManager>>,
    pub session_controller: Arc<PLRwLock<SessionController>>,
    pub injection_manager: Arc<PLRwLock<InjectionManager>>,
    pub storage: Arc<SessionStorage>,
    pub event_bus: Arc<EventBus>,
    pub app_state_db: Arc<ApplicationStateDb>,
    /// Durable sub-agent run queue (#126). The `agent_run_queue` table is the source of
    /// truth for queued/running/finalized workers; `Session.agents` is a UI cache.
    pub queue_manager: Arc<QueueManager>,
    pub app_handle: Option<AppHandle>,
    /// Unified action registry, dispatched by both the Tauri and HTTP surfaces.
    /// Wrapped in `OnceLock` so `AppState` can be constructed before the registry
    /// exists and then have it attached once (avoids a construction-order cycle:
    /// the registry's actions reach back into `AppState` via `ActionContext`).
    pub registry: std::sync::OnceLock<Arc<ActionRegistry>>,
}

impl AppState {
    pub fn new(
        config: Arc<RwLock<AppConfig>>,
        pty_manager: Arc<PLRwLock<PtyManager>>,
        session_controller: Arc<PLRwLock<SessionController>>,
        injection_manager: Arc<PLRwLock<InjectionManager>>,
        storage: Arc<SessionStorage>,
        event_bus: Arc<EventBus>,
        app_state_db: Arc<ApplicationStateDb>,
        queue_manager: Arc<QueueManager>,
        app_handle: Option<AppHandle>,
    ) -> Self {
        Self {
            config,
            pty_manager,
            session_controller,
            injection_manager,
            storage,
            event_bus,
            app_state_db,
            queue_manager,
            app_handle,
            registry: std::sync::OnceLock::new(),
        }
    }

    /// Attach the action registry. Idempotent — the first set wins.
    pub fn set_registry(&self, registry: Arc<ActionRegistry>) {
        let _ = self.registry.set(registry);
    }

    /// The attached registry. Panics if dispatched-through before
    /// [`AppState::set_registry`] ran (a startup-wiring bug, not a runtime path).
    pub fn registry(&self) -> &Arc<ActionRegistry> {
        self.registry
            .get()
            .expect("ActionRegistry not attached to AppState")
    }

    pub async fn emit_conversation_message(
        &self,
        session_id: &str,
        agent_id: &str,
        message: &ConversationMessage,
    ) -> Result<(), String> {
        let data = serde_json::json!({
            "session_id": session_id,
            "agent_id": agent_id,
            "timestamp": message.timestamp,
            "from": message.from,
            "content": message.content,
        });
        let envelope = envelope_for_content(data.clone(), &message.content);
        let mut payload = data;
        if let (Some(payload), Some(envelope)) = (payload.as_object_mut(), envelope.as_object()) {
            for (key, value) in envelope {
                payload.insert(key.clone(), value.clone());
            }
        }

        if let Some(app_handle) = self.app_handle.as_ref() {
            app_handle
                .emit("conversation-message", payload.clone())
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
                payload,
                severity: Severity::Info,
            })
            .await
    }
}
