//! Provider-neutral transport and attributed event contract. PTYs remain the default.
pub mod claude_transcript_source;
pub mod feed;

use crate::delivery::Delivery;
use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::broadcast;

pub const MAX_TEXT_BYTES: usize = 16 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    None,
    Transcript,
    Protocol,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capabilities {
    pub has_terminal: bool,
    pub can_interrupt: bool,
    pub supports_approvals: bool,
    pub event_source: EventSource,
}
impl Capabilities {
    pub fn pty(has_transcript: bool) -> Self {
        Self {
            has_terminal: true,
            can_interrupt: true,
            supports_approvals: false,
            event_source: if has_transcript {
                EventSource::Transcript
            } else {
                EventSource::None
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventKind {
    UserMessage,
    AssistantDelta,
    AssistantMessage,
    ToolStart,
    ToolResult,
    TurnStart,
    TurnEnd,
    Status,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentEvent {
    pub agent_id: String,
    pub cursor: String,
    pub ts: String,
    pub kind: AgentEventKind,
    pub source_id: Option<String>,
    pub truncated: bool,
    pub payload: Value,
}
fn clip(text: &mut String, limit: usize) -> bool {
    if text.len() <= limit {
        return false;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    true
}
impl AgentEvent {
    /// The journal repeats this for protocol callers as defense in depth.
    pub fn normalize(mut self) -> Self {
        fn cap(value: &mut Value, truncated: &mut bool) {
            match value {
                Value::String(text) => *truncated |= clip(text, MAX_TEXT_BYTES),
                Value::Array(items) => {
                    if items.len() > 128 {
                        items.truncate(128);
                        *truncated = true;
                    }
                    for item in items {
                        cap(item, truncated);
                    }
                }
                Value::Object(fields) => {
                    for value in fields.values_mut() {
                        cap(value, truncated);
                    }
                }
                _ => {}
            }
        }
        cap(&mut self.payload, &mut self.truncated);
        let serialized = self.payload.to_string();
        if serialized.len() > MAX_PAYLOAD_BYTES {
            let mut preview = serialized;
            clip(&mut preview, MAX_TEXT_BYTES / 2);
            self.payload = serde_json::json!({"text": preview});
            self.truncated = true;
        }
        if let Some(id) = &mut self.source_id {
            self.truncated |= clip(id, 512);
        }
        self.truncated |= clip(&mut self.ts, 64);
        self
    }
}

#[async_trait]
pub trait AgentTransport: Send + Sync {
    async fn start(&self) -> Result<(), String>;
    async fn send(&self, text: &str) -> Result<Delivery, String>;
    async fn interrupt(&self) -> Result<(), String>;
    async fn stop(&self) -> Result<(), String>;
    fn subscribe(&self) -> broadcast::Receiver<AgentEvent>;
    fn capabilities(&self) -> Capabilities;
}

#[derive(Clone)]
pub struct RegisteredTransport {
    pub transport: Arc<dyn AgentTransport>,
    pub thread_id: Option<String>,
}

/// IDs are held by Hive, never discovered from cwd, timestamps or rollout scans.
pub struct TransportRegistry {
    entries: RwLock<HashMap<(String, String), RegisteredTransport>>,
    pub feeds: feed::FeedStore,
}
impl TransportRegistry {
    pub fn new(sessions_root: PathBuf, claude_projects_root: Option<PathBuf>) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            feeds: feed::FeedStore::new(sessions_root, claude_projects_root),
        }
    }
    /// Register after start succeeds. Never silently replace a live child.
    pub fn register(
        &self,
        session_id: &str,
        agent_id: &str,
        thread_id: Option<String>,
        transport: Arc<dyn AgentTransport>,
    ) -> Result<(), String> {
        feed::validate_ids(session_id, agent_id)?;
        let mut entries = self.entries.write();
        let key = (session_id.to_owned(), agent_id.to_owned());
        if entries.contains_key(&key) {
            return Err("Transport already registered".into());
        }
        self.feeds.use_protocol(session_id, agent_id)?;
        entries.insert(
            key,
            RegisteredTransport {
                transport,
                thread_id,
            },
        );
        Ok(())
    }
    pub fn get(&self, session_id: &str, agent_id: &str) -> Option<RegisteredTransport> {
        self.entries
            .read()
            .get(&(session_id.into(), agent_id.into()))
            .cloned()
    }
    pub fn set_thread_id(
        &self,
        session_id: &str,
        agent_id: &str,
        thread_id: String,
    ) -> Result<(), String> {
        let mut entries = self.entries.write();
        let entry = entries
            .get_mut(&(session_id.into(), agent_id.into()))
            .ok_or("Transport not registered")?;
        entry.thread_id = Some(thread_id);
        Ok(())
    }
    /// Adapter supplies its registered identity, never identity from an HTTP payload.
    pub fn append_event(
        &self,
        session_id: &str,
        agent_id: &str,
        event: AgentEvent,
    ) -> Result<AgentEvent, String> {
        if self.get(session_id, agent_id).is_none() {
            return Err("Transport not registered".into());
        }
        self.feeds.append_protocol(session_id, agent_id, event)
    }
    pub async fn stop_agent(&self, session_id: &str, agent_id: &str) -> Result<(), String> {
        if let Some(entry) = self.get(session_id, agent_id) {
            entry.transport.stop().await?;
            self.entries
                .write()
                .remove(&(session_id.into(), agent_id.into()));
        }
        Ok(())
    }
    pub async fn stop_all(&self) -> Vec<String> {
        let keys: Vec<_> = self.entries.read().keys().cloned().collect();
        let mut errors = Vec::new();
        for (session, agent) in keys {
            if let Err(error) = self.stop_agent(&session, &agent).await {
                errors.push(format!("{agent}: {error}"));
            }
        }
        errors
    }
}
