//! Shared fake-transport contract tests; D then E extend this file after B's handoff.
use crate::{
    delivery::{self, Confirmation, Delivery, ProtocolConfirmation},
    transport::{AgentEvent, AgentTransport, Capabilities, EventSource, TransportRegistry},
};
use async_trait::async_trait;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::broadcast;

pub(super) struct FakeTransport {
    pub stopped: AtomicBool,
    events: broadcast::Sender<AgentEvent>,
}
impl FakeTransport {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            stopped: AtomicBool::new(false),
            events: broadcast::channel(16).0,
        })
    }
}

#[tokio::test]
async fn claude_protocol_inject_and_feed_use_one_source_even_with_a_transcript() {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use serde_json::{json, Value};
    use tower::ServiceExt;
    let (temp, state, app) = super::tests_agent_events::setup();
    state.storage.create_session_dir("session").unwrap();
    let mut session = super::tests::make_test_session_with_agents(
        "session",
        temp.path().to_str().unwrap(),
        &["worker-a"],
    );
    session.no_git = true;
    session.agents[0].config.cli = "claude".into();
    session.agents[0].transcript_session_id = Some("held-claude".into());
    let slug: String = temp
        .path()
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let directory = temp.path().join("claude-projects").join(slug);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("held-claude.jsonl"),format!("{}\n",json!({"type":"user","uuid":"transcript-record","message":{"role":"user","content":"transcript-only"}}))).unwrap();
    state.session_controller.read().insert_test_session(session);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session/agents/worker-a/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let before: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(
        before["events"][0]["payload"]["text"], "transcript-only",
        "the transcript fixture must be readable before switching sources"
    );
    let fake = FakeTransport::new();
    state
        .transports()
        .register("session", "worker-a", Some("held-claude".into()), fake)
        .unwrap();
    state
        .transports()
        .append_event(
            "session",
            "worker-a",
            super::tests_agent_events::event("worker-a", "protocol-only"),
        )
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session/inject")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"target_agent_id":"worker-a","message":"confirm","submit":true})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(body["submit_confirmed"], true);
    assert_eq!(body["submit_confirmation_source"], "protocol");
    assert_eq!(body["submit_bytes_written"], 0);
    assert_eq!(body["submit_keystroke_issued"], false);
    assert!(body["evidence_scope"]
        .as_str()
        .unwrap()
        .contains("turn-start evidence"));
    for uri in [
        "/api/sessions/session/agent-feeds",
        "/api/sessions/session/agents/worker-a/events",
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        if uri.ends_with("agent-feeds") {
            assert_eq!(
                body["agents"][0]["capabilities"]["event_source"],
                "protocol"
            );
            assert_eq!(body["agents"][0]["capabilities"]["has_terminal"], false);
        } else {
            assert_eq!(
                body["events"].as_array().unwrap().len(),
                1,
                "Claude stream-json must never mix transcript and protocol"
            );
            assert_eq!(body["events"][0]["payload"]["text"], "protocol-only");
        }
    }
    state.pty_manager.read().kill("worker-a").unwrap();
}
#[async_trait]
impl AgentTransport for FakeTransport {
    fn send_input(&self, text: &str, submit: bool) -> Result<Delivery, String> {
        Ok(if submit && text == "confirm" {
            Delivery::protocol_turn_started("fixture-turn".into())
        } else {
            Delivery::protocol_pending()
        })
    }
    fn is_alive(&self) -> bool {
        !self.stopped.load(Ordering::Acquire)
    }
    fn stop_sync(&self) -> Result<(), String> {
        self.stopped.store(true, Ordering::Release);
        Ok(())
    }
    fn interrupt_sync(&self) -> Result<(), String> {
        Ok(())
    }
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
        self.stopped.store(true, Ordering::Release);
        Ok(())
    }
    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            has_terminal: false,
            can_interrupt: true,
            supports_approvals: true,
            event_source: EventSource::Protocol,
        }
    }
}

#[tokio::test]
async fn protocol_inject_keeps_receipt_shape_without_terminal_writes_and_cleanup() {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use serde_json::{json, Value};
    use tower::ServiceExt;
    let (temp, state, app) = super::tests_agent_events::setup();
    state.storage.create_session_dir("session").unwrap();
    let mut session = super::tests::make_test_session_with_agents(
        "session",
        temp.path().to_str().unwrap(),
        &["worker-a"],
    );
    session.agents[0].config.cli = "codex".into();
    state.session_controller.read().insert_test_session(session);
    let fake = FakeTransport::new();
    state
        .transports()
        .register(
            "session",
            "worker-a",
            Some("held-fixture".into()),
            fake.clone(),
        )
        .unwrap();
    assert!(
        state.pty_manager.read().is_alive("worker-a"),
        "protocol worker must survive the startup liveness check"
    );
    for (message, submit, confirmed) in [
        ("confirm", true, Some(true)),
        ("staged", false, None),
        ("queued", true, None),
        ("", true, None),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions/session/inject")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"target_agent_id":"worker-a","message":message,"submit":submit})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(value["submit_confirmed"].as_bool(), confirmed);
        assert_eq!(value["submit_confirmation_source"], "protocol");
        assert_eq!(value["submit_bytes_written"], 0);
        assert_eq!(value["submit_attempts"], 0);
        assert_eq!(value["submit_keystroke_issued"], false);
        assert_eq!(value["pty_observation_available"], false);
        assert!(value["submit_retry_keystroke"].is_null());
        assert_eq!(
            value["submit_confirmation_basis"],
            if confirmed.is_some() {
                "protocol-turn-started"
            } else {
                "protocol-pending"
            }
        );
    }
    for write in [b"\t".as_slice(), b"\r".as_slice()] {
        assert!(state
            .pty_manager
            .read()
            .write("worker-a", write)
            .unwrap_err()
            .to_string()
            .contains("no terminal"));
    }
    state.pty_manager.read().kill("worker-a").unwrap();
    assert!(fake.stopped.load(Ordering::Acquire));
    assert!(!state.pty_manager.read().is_alive("worker-a"));
    assert!(state.transports().get("session", "worker-a").is_none());
}

#[tokio::test]
async fn contract_fake_start_send_interrupt_subscribe_and_exit_stop_all() {
    let temp = tempfile::tempdir().unwrap();
    let registry = Arc::new(TransportRegistry::new(temp.path().join("sessions"), None));
    let manager = crate::pty::PtyManager::new();
    manager.attach_transports(registry.clone());
    let a = FakeTransport::new();
    let b = FakeTransport::new();
    a.start().await.unwrap();
    a.interrupt().await.unwrap();
    assert!(a.send("fixture").await.unwrap().is_pending());
    let _receiver = a.subscribe();
    registry
        .register("session-a", "worker-a", Some("thread-a".into()), a.clone())
        .unwrap();
    registry
        .register("session-b", "worker-a", Some("thread-b".into()), b.clone())
        .unwrap();
    assert_eq!(
        registry
            .get("session-a", "worker-a")
            .unwrap()
            .thread_id
            .as_deref(),
        Some("thread-a")
    );
    assert!(registry
        .register("session-a", "worker-a", None, a.clone())
        .is_err());
    registry
        .set_thread_id("session-a", "worker-a", "thread-resumed".into())
        .unwrap();
    assert_eq!(
        registry
            .get("session-b", "worker-a")
            .unwrap()
            .thread_id
            .as_deref(),
        Some("thread-b")
    );
    assert!(manager.shutdown_transports().is_empty());
    assert!(
        a.stopped.load(Ordering::Acquire) && b.stopped.load(Ordering::Acquire),
        "G15 exit stops every registered transport"
    );
    assert!(registry.get("session-a", "worker-a").is_none());
    assert!(manager.shutdown_transports().is_empty());
    // The desktop callback invokes the same exercised shutdown function.
    let desktop = include_str!("../lib.rs");
    assert!(
        desktop.contains("tauri::RunEvent::Exit")
            && desktop.contains("state.pty_manager.read().shutdown_transports()")
    );
}

#[test]
fn protocol_confirmation_requires_both_held_thread_and_expected_turn() {
    let evidence = ProtocolConfirmation::new("thread-a".into(), "turn-a".into());
    let mut transaction = Confirmation::Protocol(evidence.clone());
    assert!(
        delivery::confirm(&mut transaction, "unused", std::time::SystemTime::now()).is_pending()
    );
    assert!(!evidence.observe_started("thread-b", "turn-a"));
    assert!(!evidence.observe_started("thread-a", "turn-b"));
    assert_eq!(evidence.delivery().confirmed, None);
    assert!(evidence.observe_started("thread-a", "turn-a"));
    let receipt = delivery::confirm(&mut transaction, "unused", std::time::SystemTime::now());
    assert_eq!(receipt, Delivery::protocol_turn_started("turn-a".into()));
    assert_eq!(receipt.source, "protocol");
    assert!(!receipt.is_pending());
    assert_eq!(receipt.basis, "protocol-turn-started");
}
