use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::domain::event::Event;

const CHANNEL_CAPACITY: usize = 1024;

#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
    data_dir: PathBuf,
}

impl EventBus {
    /// Create a new EventBus. `data_dir` is the root `.hive-manager/` directory
    /// where per-session `events.jsonl` files will be written.
    pub fn new(data_dir: PathBuf) -> Arc<Self> {
        let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
        Arc::new(Self { sender, data_dir })
    }

    /// Publish an event to all subscribers and persist to JSONL.
    pub async fn publish(&self, event: Event) -> Result<(), String> {
        self.persist_jsonl(&event).await?;

        // broadcast::send only fails when there are no receivers, which is fine
        let _ = self.sender.send(event);
        Ok(())
    }

    /// Subscribe to all events on the bus.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }

    /// Subscribe and filter events by session_id.
    pub fn subscribe_session(&self, session_id: String) -> FilteredReceiver {
        FilteredReceiver {
            inner: self.sender.subscribe(),
            filter: EventFilter::Session(session_id),
        }
    }

    /// Subscribe and filter events by cell_id.
    pub fn subscribe_cell(&self, cell_id: String) -> FilteredReceiver {
        FilteredReceiver {
            inner: self.sender.subscribe(),
            filter: EventFilter::Cell(cell_id),
        }
    }

    /// Subscribe and filter events by agent_id.
    pub fn subscribe_agent(&self, agent_id: String) -> FilteredReceiver {
        FilteredReceiver {
            inner: self.sender.subscribe(),
            filter: EventFilter::Agent(agent_id),
        }
    }

    async fn persist_jsonl(&self, event: &Event) -> Result<(), String> {
        let session_dir = self.data_dir.join(&event.session_id);
        tokio::fs::create_dir_all(&session_dir)
            .await
            .map_err(|e| format!("Failed to create session dir: {e}"))?;

        let path = session_dir.join("events.jsonl");
        let mut line =
            serde_json::to_string(event).map_err(|e| format!("Failed to serialize event: {e}"))?;
        line.push('\n');

        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| format!("Failed to open events.jsonl: {e}"))?
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("Failed to write event: {e}"))?;

        Ok(())
    }
}

// Need AsyncWriteExt for write_all
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone)]
enum EventFilter {
    Session(String),
    Cell(String),
    Agent(String),
}

pub struct FilteredReceiver {
    inner: broadcast::Receiver<Event>,
    filter: EventFilter,
}

impl FilteredReceiver {
    /// Receive the next event matching the filter.
    pub async fn recv(&mut self) -> Result<Event, broadcast::error::RecvError> {
        loop {
            let event = self.inner.recv().await?;
            let matches = match &self.filter {
                EventFilter::Session(id) => event.session_id == *id,
                EventFilter::Cell(id) => event.cell_id.as_deref() == Some(id.as_str()),
                EventFilter::Agent(id) => event.agent_id.as_deref() == Some(id.as_str()),
            };
            if matches {
                return Ok(event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::event::{EventType, Severity};
    use chrono::Utc;
    use tempfile::TempDir;

    fn make_event(session_id: &str, event_type: EventType) -> Event {
        Event {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            cell_id: None,
            agent_id: None,
            event_type,
            timestamp: Utc::now(),
            payload: serde_json::json!({}),
            severity: Severity::Info,
        }
    }

    #[tokio::test]
    async fn test_publish_subscribe() {
        let tmp = TempDir::new().unwrap();
        let bus = EventBus::new(tmp.path().to_path_buf());

        let mut rx = bus.subscribe();
        let event = make_event("sess-1", EventType::SessionCreated);
        let event_id = event.id.clone();

        bus.publish(event).await.unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received.id, event_id);
    }

    #[tokio::test]
    async fn test_session_filter() {
        let tmp = TempDir::new().unwrap();
        let bus = EventBus::new(tmp.path().to_path_buf());

        let mut filtered = bus.subscribe_session("sess-A".to_string());

        // Publish to different sessions
        let event_a = make_event("sess-A", EventType::CellCreated);
        let event_b = make_event("sess-B", EventType::CellCreated);
        let event_a2 = make_event("sess-A", EventType::AgentLaunched);
        let id_a = event_a.id.clone();
        let id_a2 = event_a2.id.clone();

        bus.publish(event_a).await.unwrap();
        bus.publish(event_b).await.unwrap();
        bus.publish(event_a2).await.unwrap();

        let r1 = filtered.recv().await.unwrap();
        assert_eq!(r1.id, id_a);
        let r2 = filtered.recv().await.unwrap();
        assert_eq!(r2.id, id_a2);
    }

    #[tokio::test]
    async fn test_jsonl_persistence() {
        let tmp = TempDir::new().unwrap();
        let bus = EventBus::new(tmp.path().to_path_buf());

        let event = make_event("sess-persist", EventType::SessionCreated);
        bus.publish(event.clone()).await.unwrap();

        let contents =
            tokio::fs::read_to_string(tmp.path().join("sess-persist/events.jsonl"))
                .await
                .unwrap();
        let deserialized: Event = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(deserialized.id, event.id);
        assert_eq!(deserialized.event_type, EventType::SessionCreated);
    }

    #[tokio::test]
    async fn test_jsonl_append() {
        let tmp = TempDir::new().unwrap();
        let bus = EventBus::new(tmp.path().to_path_buf());

        bus.publish(make_event("sess-append", EventType::SessionCreated))
            .await
            .unwrap();
        bus.publish(make_event("sess-append", EventType::AgentLaunched))
            .await
            .unwrap();

        let contents =
            tokio::fs::read_to_string(tmp.path().join("sess-append/events.jsonl"))
                .await
                .unwrap();
        let lines: Vec<&str> = contents.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        let e1: Event = serde_json::from_str(lines[0]).unwrap();
        let e2: Event = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(e1.event_type, EventType::SessionCreated);
        assert_eq!(e2.event_type, EventType::AgentLaunched);
    }
}
