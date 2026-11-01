//! Provider-neutral transport and attributed event contract. PTYs remain the default.
pub mod claude;
pub mod claude_transcript_source;
pub mod codex;
pub mod codex_rollout_source;
pub mod feed;
pub(crate) mod process;

#[cfg(test)]
mod tests_option_off;

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
    fn send_input(&self, _text: &str, _submit: bool) -> Result<Delivery, String> {
        Err("Transport does not support synchronous input".into())
    }
    fn is_alive(&self) -> bool {
        true
    }
    fn stop_sync(&self) -> Result<(), String> {
        Err("Transport does not support synchronous stop".into())
    }
    fn interrupt_sync(&self) -> Result<(), String> {
        Err("Transport does not support synchronous interrupt".into())
    }
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
    /// Session-less callers may resolve only an unambiguous registered identity.
    pub fn get_agent(&self, agent_id: &str) -> Option<(String, RegisteredTransport)> {
        let entries = self.entries.read();
        let mut matches = entries.iter().filter(|((_, agent), _)| agent == agent_id);
        let ((session, _), entry) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some((session.clone(), entry.clone()))
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
    pub fn stop_agent_sync(&self, session_id: &str, agent_id: &str) -> Result<(), String> {
        if let Some(entry) = self.get(session_id, agent_id) {
            // Keep the handle available for retries until stopping succeeds.
            // Do not hold the registry lock while calling the transport.
            entry.transport.stop_sync()?;
            let mut entries = self.entries.write();
            let key = (session_id.into(), agent_id.into());
            if entries
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(&current.transport, &entry.transport))
            {
                entries.remove(&key);
            }
        }
        Ok(())
    }
    pub async fn stop_agent(&self, session_id: &str, agent_id: &str) -> Result<(), String> {
        if let Some(entry) = self.get(session_id, agent_id) {
            entry.transport.stop().await?;
            let mut entries = self.entries.write();
            let key = (session_id.into(), agent_id.into());
            if entries
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(&current.transport, &entry.transport))
            {
                entries.remove(&key);
            }
        }
        Ok(())
    }
    pub fn stop_all_sync(&self) -> Vec<String> {
        let keys: Vec<_> = self.entries.read().keys().cloned().collect();
        let mut errors = Vec::new();
        for (session, agent) in keys {
            if let Err(error) = self.stop_agent_sync(&session, &agent) {
                errors.push(format!("{agent}: {error}"));
            }
        }
        errors
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

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeTransport {
        fail_stop: AtomicBool,
        stop_calls: AtomicUsize,
        on_stop: Mutex<Option<Box<dyn FnOnce() + Send>>>,
        delay_async_stop: AtomicBool,
        async_stop_started: tokio::sync::Notify,
        async_stop_release: tokio::sync::Notify,
    }

    #[async_trait]
    impl AgentTransport for FakeTransport {
        async fn start(&self) -> Result<(), String> {
            Ok(())
        }
        async fn send(&self, _text: &str) -> Result<Delivery, String> {
            Ok(Delivery::protocol_pending())
        }
        async fn interrupt(&self) -> Result<(), String> {
            Ok(())
        }
        async fn stop(&self) -> Result<(), String> {
            if self.delay_async_stop.swap(false, Ordering::SeqCst) {
                self.async_stop_started.notify_one();
                self.async_stop_release.notified().await;
            }
            self.stop_sync()
        }
        fn stop_sync(&self) -> Result<(), String> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_stop.load(Ordering::SeqCst) {
                return Err("fixture stop failed".into());
            }
            if let Some(on_stop) = self.on_stop.lock().take() {
                on_stop();
            }
            Ok(())
        }
        fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
            broadcast::channel(1).1
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::pty(false)
        }
    }

    #[tokio::test]
    async fn delayed_async_stop_cannot_unregister_a_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let registry = Arc::new(TransportRegistry::new(temp.path().join("feeds"), None));
        let old = Arc::new(FakeTransport::default());
        old.delay_async_stop.store(true, Ordering::Release);
        registry
            .register("session", "worker", None, old.clone())
            .unwrap();
        let stopping = registry.clone();
        let stop = tokio::spawn(async move { stopping.stop_agent("session", "worker").await });
        old.async_stop_started.notified().await;
        registry.stop_agent_sync("session", "worker").unwrap();
        let replacement = Arc::new(FakeTransport::default());
        registry
            .register("session", "worker", None, replacement.clone())
            .unwrap();
        old.async_stop_release.notify_one();
        stop.await.unwrap().unwrap();
        let expected: Arc<dyn AgentTransport> = replacement.clone();
        let held = registry
            .get("session", "worker")
            .expect("replacement survives delayed old stop");
        assert!(Arc::ptr_eq(&held.transport, &expected));
        assert_eq!(replacement.stop_calls.load(Ordering::Acquire), 0);
        registry.stop_agent_sync("session", "worker").unwrap();
    }

    #[test]
    fn synchronous_defaults_preserve_fake_liveness_and_reject_unsupported_input() {
        let transport = FakeTransport::default();
        assert!(transport.is_alive());
        assert_eq!(
            transport.send_input("fixture", true).unwrap_err(),
            "Transport does not support synchronous input"
        );
        assert_eq!(
            transport.interrupt_sync().unwrap_err(),
            "Transport does not support synchronous interrupt"
        );
    }

    #[test]
    fn get_agent_requires_unique_identity_across_sessions() {
        let root = tempfile::tempdir().unwrap();
        let registry = TransportRegistry::new(root.path().into(), None);
        let first = Arc::new(FakeTransport::default());
        registry
            .register(
                "session-a",
                "worker",
                Some("thread-a".into()),
                first.clone(),
            )
            .unwrap();
        assert!(registry.get_agent("missing").is_none());
        let (session, entry) = registry.get_agent("worker").unwrap();
        assert_eq!(session, "session-a");
        assert_eq!(entry.thread_id.as_deref(), Some("thread-a"));
        let expected: Arc<dyn AgentTransport> = first;
        assert!(Arc::ptr_eq(&entry.transport, &expected));

        registry
            .register(
                "session-b",
                "worker",
                None,
                Arc::new(FakeTransport::default()),
            )
            .unwrap();
        assert!(
            registry.get_agent("worker").is_none(),
            "ambiguous agent must not resolve"
        );
        assert!(registry.get("session-a", "worker").is_some());
        assert!(registry.get("session-b", "worker").is_some());
    }

    #[test]
    fn stop_agent_sync_removes_only_the_successfully_stopped_entry() {
        let root = tempfile::tempdir().unwrap();
        let registry = TransportRegistry::new(root.path().into(), None);
        let transport = Arc::new(FakeTransport::default());
        let other = Arc::new(FakeTransport::default());
        registry
            .register("session-a", "worker", None, transport.clone())
            .unwrap();
        registry
            .register("session-b", "worker", None, other.clone())
            .unwrap();

        registry.stop_agent_sync("session-a", "worker").unwrap();
        assert!(registry.get("session-a", "worker").is_none());
        assert!(registry.get("session-b", "worker").is_some());
        assert_eq!(transport.stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(other.stop_calls.load(Ordering::SeqCst), 0);
        registry.stop_agent_sync("session-a", "worker").unwrap();
        assert_eq!(transport.stop_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stop_agent_sync_keeps_failed_handle_for_retry() {
        let root = tempfile::tempdir().unwrap();
        let registry = TransportRegistry::new(root.path().into(), None);
        let transport = Arc::new(FakeTransport::default());
        transport.fail_stop.store(true, Ordering::SeqCst);
        registry
            .register("session", "worker", None, transport.clone())
            .unwrap();

        assert_eq!(
            registry.stop_agent_sync("session", "worker").unwrap_err(),
            "fixture stop failed"
        );
        let retained = registry.get("session", "worker").unwrap();
        let expected: Arc<dyn AgentTransport> = transport.clone();
        assert!(
            Arc::ptr_eq(&retained.transport, &expected),
            "failed stop must retain exact handle"
        );
        transport.fail_stop.store(false, Ordering::SeqCst);
        registry.stop_agent_sync("session", "worker").unwrap();
        assert!(registry.get("session", "worker").is_none());
        assert_eq!(transport.stop_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn stop_agent_sync_preserves_replacement_registered_during_stop() {
        let root = tempfile::tempdir().unwrap();
        let registry = Arc::new(TransportRegistry::new(root.path().into(), None));
        let transport = Arc::new(FakeTransport::default());
        let replacement: Arc<dyn AgentTransport> = Arc::new(FakeTransport::default());
        registry
            .register("session", "worker", None, transport.clone())
            .unwrap();
        let weak_registry = Arc::downgrade(&registry);
        let replacement_for_stop = replacement.clone();
        *transport.on_stop.lock() = Some(Box::new(move || {
            let registry = weak_registry.upgrade().unwrap();
            registry
                .entries
                .write()
                .remove(&("session".into(), "worker".into()));
            registry
                .register("session", "worker", None, replacement_for_stop)
                .unwrap();
        }));

        registry.stop_agent_sync("session", "worker").unwrap();
        let retained = registry
            .get("session", "worker")
            .expect("replacement must remain registered");
        assert!(Arc::ptr_eq(&retained.transport, &replacement));
        assert_eq!(transport.stop_calls.load(Ordering::SeqCst), 1);
    }
}
