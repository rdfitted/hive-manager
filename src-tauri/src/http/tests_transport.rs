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
async fn contract_fake_start_send_interrupt_subscribe_and_exit_stop_all() {
    let temp = tempfile::tempdir().unwrap();
    let registry = TransportRegistry::new(temp.path().join("sessions"), None);
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
    assert!(registry.stop_all().await.is_empty());
    assert!(
        a.stopped.load(Ordering::Acquire) && b.stopped.load(Ordering::Acquire),
        "G15 exit stops every registered transport"
    );
    assert!(registry.get("session-a", "worker-a").is_none());
    assert!(registry.stop_all().await.is_empty());
    // The desktop callback invokes the same exercised shutdown function.
    let desktop = include_str!("../lib.rs");
    assert!(
        desktop.contains("tauri::RunEvent::Exit")
            && desktop.contains("block_on(registry.stop_all())")
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
