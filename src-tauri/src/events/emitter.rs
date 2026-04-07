use std::sync::Arc;

use chrono::Utc;
use serde_json::json;

use crate::domain::event::{Event, EventType, Severity};
use super::bus::EventBus;

/// Convenience wrapper around `EventBus` providing typed emit methods.
#[derive(Clone)]
pub struct EventEmitter {
    bus: Arc<EventBus>,
}

impl EventEmitter {
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self { bus }
    }

    pub async fn emit_session_created(
        &self,
        session_id: &str,
        mode: &str,
    ) -> Result<(), String> {
        self.emit(session_id, None, None, EventType::SessionCreated, Severity::Info, json!({
            "mode": mode,
        })).await
    }

    pub async fn emit_session_status_changed(
        &self,
        session_id: &str,
        from: &str,
        to: &str,
    ) -> Result<(), String> {
        self.emit(session_id, None, None, EventType::SessionStatusChanged, Severity::Info, json!({
            "from": from,
            "to": to,
        })).await
    }

    pub async fn emit_cell_created(
        &self,
        session_id: &str,
        cell_id: &str,
        cell_type: &str,
    ) -> Result<(), String> {
        self.emit(session_id, Some(cell_id), None, EventType::CellCreated, Severity::Info, json!({
            "cell_type": cell_type,
        })).await
    }

    pub async fn emit_agent_launched(
        &self,
        session_id: &str,
        cell_id: &str,
        agent_id: &str,
        cli: &str,
    ) -> Result<(), String> {
        self.emit(session_id, Some(cell_id), Some(agent_id), EventType::AgentLaunched, Severity::Info, json!({
            "cli": cli,
        })).await
    }

    pub async fn emit_agent_completed(
        &self,
        session_id: &str,
        cell_id: &str,
        agent_id: &str,
    ) -> Result<(), String> {
        self.emit(session_id, Some(cell_id), Some(agent_id), EventType::AgentCompleted, Severity::Info, json!({})).await
    }

    pub async fn emit_agent_failed(
        &self,
        session_id: &str,
        cell_id: &str,
        agent_id: &str,
        error: &str,
    ) -> Result<(), String> {
        self.emit(session_id, Some(cell_id), Some(agent_id), EventType::AgentFailed, Severity::Error, json!({
            "error": error,
        })).await
    }

    pub async fn emit_artifact_updated(
        &self,
        session_id: &str,
        cell_id: &str,
        agent_id: &str,
        artifact_path: &str,
    ) -> Result<(), String> {
        self.emit(session_id, Some(cell_id), Some(agent_id), EventType::ArtifactUpdated, Severity::Info, json!({
            "path": artifact_path,
        })).await
    }

    pub async fn emit_resolver_selected_candidate(
        &self,
        session_id: &str,
        cell_id: &str,
        selected_agent_id: &str,
        reason: &str,
    ) -> Result<(), String> {
        self.emit(session_id, Some(cell_id), Some(selected_agent_id), EventType::ResolverSelectedCandidate, Severity::Info, json!({
            "reason": reason,
        })).await
    }

    async fn emit(
        &self,
        session_id: &str,
        cell_id: Option<&str>,
        agent_id: Option<&str>,
        event_type: EventType,
        severity: Severity,
        payload: serde_json::Value,
    ) -> Result<(), String> {
        let event = Event {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            cell_id: cell_id.map(String::from),
            agent_id: agent_id.map(String::from),
            event_type,
            timestamp: Utc::now(),
            payload,
            severity,
        };
        self.bus.publish(event).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_emit_session_created() {
        let tmp = TempDir::new().unwrap();
        let bus = EventBus::new(tmp.path().to_path_buf());
        let emitter = EventEmitter::new(bus.clone());

        let mut rx = bus.subscribe();
        emitter.emit_session_created("s1", "swarm").await.unwrap();

        let event = rx.recv().await.unwrap();
        assert_eq!(event.session_id, "s1");
        assert_eq!(event.event_type, EventType::SessionCreated);
        assert_eq!(event.payload["mode"], "swarm");
    }

    #[tokio::test]
    async fn test_emit_agent_failed_severity() {
        let tmp = TempDir::new().unwrap();
        let bus = EventBus::new(tmp.path().to_path_buf());
        let emitter = EventEmitter::new(bus.clone());

        let mut rx = bus.subscribe();
        emitter
            .emit_agent_failed("s1", "c1", "a1", "timeout")
            .await
            .unwrap();

        let event = rx.recv().await.unwrap();
        assert_eq!(event.severity, Severity::Error);
        assert_eq!(event.payload["error"], "timeout");
    }
}
