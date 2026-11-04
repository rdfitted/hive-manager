use super::*;

#[tokio::test]
async fn g53_verified_stop_preserves_projected_earned_completion_and_event_flag() {
    let temp = tempfile::tempdir().unwrap();
    let mut controller = test_controller();
    let bus = crate::events::EventBus::new(temp.path().to_path_buf());
    controller.set_event_bus(bus.clone());
    planning_session_with_planner(&controller, "g53-preserved", &[]);
    // Master Planners deliberately cannot earn heartbeat completion. Exercise
    // a worker with readable empty queue evidence, as the HTTP roster does.
    controller.sessions.write().get_mut("g53-preserved").unwrap().agents[0].role =
        AgentRole::Worker { index: 1, parent: None };
    let db = Arc::new(crate::storage::ApplicationStateDb::open_in_memory().unwrap());
    let queue = Arc::new(crate::storage::QueueRepo::new(db));
    queue.ensure_schema().unwrap();
    controller.set_queue_repo(queue);
    let id = "g53-preserved-master-planner";
    let instance = controller.pty_manager.read().instance_id(id).unwrap();
    controller.update_heartbeat_with_source("g53-preserved", id, "completed", None, true).unwrap();
    assert_eq!(controller.get_session("g53-preserved").unwrap().agents[0].work_state,
        Some(crate::session::work_state::WorkState::Finished), "fixture must have earned completion before stop");
    let mut events = bus.subscribe();
    controller.stop_agent_instance("g53-preserved", id, instance).unwrap();
    assert_eq!(controller.get_session("g53-preserved").unwrap().agents[0].work_state,
        Some(crate::session::work_state::WorkState::Finished));
    let event = tokio::time::timeout(std::time::Duration::from_secs(3), events.recv()).await.unwrap().unwrap();
    assert_eq!(event.event_type, crate::domain::event::EventType::AgentStopped);
    assert_eq!(event.payload["preserved"], true);
    assert_eq!(controller.pty_manager.read().instance_id(id), None);
}

#[tokio::test]
async fn g53_stop_cannot_overwrite_replacement_published_after_verified_cleanup() {
    let controller = test_controller();
    let session_id = "g53-replacement";
    let id = "g53-replacement-master-planner";
    planning_session_with_planner(&controller, session_id, &[]);
    let instance = controller.pty_manager.read().instance_id(id).unwrap();
    let manager = controller.pty_manager.clone();
    SessionController::test_sync_hook(session_id, "stop_after_verified_cleanup", Some(Box::new(move || {
        manager.read().create_session(id.into(), AgentRole::MasterPlanner, "fixture", &[], None, 80, 24).unwrap();
    })));
    assert!(controller.stop_agent_instance(session_id, id, instance).unwrap_err().contains("replaced"));
    let replacement = controller.pty_manager.read().instance_id(id).unwrap();
    assert_ne!(replacement, instance);
    assert!(matches!(controller.get_session(session_id).unwrap().agents[0].status, AgentStatus::Running));
    controller.stop_agent_instance(session_id, id, replacement).unwrap();
}

#[tokio::test]
async fn g53_verified_stop_updates_roster_releases_instance_and_emits_preserved_flag() {
    let temp = tempfile::tempdir().unwrap();
    let mut controller = test_controller();
    let storage = Arc::new(crate::storage::SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap());
    controller.set_storage(storage.clone());
    let bus = crate::events::EventBus::new(temp.path().to_path_buf());
    controller.set_event_bus(bus.clone());
    planning_session_with_planner(&controller, "g53-exited", &["--stub-die-on-start"]);
    let id = "g53-exited-master-planner";
    let instance = controller.pty_manager.read().instance_id(id).unwrap();
    let mut events = bus.subscribe();
    controller.stop_agent_instance("g53-exited", id, instance).unwrap();
    assert_eq!(controller.pty_manager.read().instance_id(id), None);
    let session = controller.get_session("g53-exited").unwrap();
    assert!(matches!(&session.agents[0].status, AgentStatus::WaitingForInput(reason) if reason == "stopped_by_operator"));
    assert!(storage.load_session("g53-exited").is_ok(), "verified stop saves the session");
    let event = tokio::time::timeout(std::time::Duration::from_secs(3), events.recv()).await.unwrap().unwrap();
    assert_eq!(event.event_type, crate::domain::event::EventType::AgentStopped);
    assert_eq!(event.payload["preserved"], false);
    assert_eq!(event.payload["instance_id"], instance.to_string());
}

#[tokio::test]
async fn g53_failed_live_stop_keeps_roster_instance_and_emits_no_stop() {
    let temp = tempfile::tempdir().unwrap();
    let mut controller = test_controller();
    let bus = crate::events::EventBus::new(temp.path().to_path_buf());
    controller.set_event_bus(bus.clone());
    planning_session_with_planner(&controller, "g53-live", &["--stub-tree-kill-fails-once"]);
    let id = "g53-live-master-planner";
    let instance = controller.pty_manager.read().instance_id(id).unwrap();
    let mut events = bus.subscribe();
    assert!(controller.stop_agent_instance("g53-live", id, instance).is_err());
    assert_eq!(controller.pty_manager.read().instance_id(id), Some(instance));
    assert!(matches!(controller.get_session("g53-live").unwrap().agents[0].status, AgentStatus::Running));
    assert!(events.try_recv().is_err());
    controller.stop_agent_instance("g53-live", id, instance).unwrap();
}

#[tokio::test]
async fn g46_master_planner_retirement_emits_stop_without_completion() {
    use crate::domain::event::EventType;
    use crate::orchestrator::work_graph::runtime::{derive_runtime_graph, RuntimeOutcomeStatus};
    let temp = tempfile::tempdir().unwrap();
    let mut controller = test_controller();
    let bus = crate::events::EventBus::new(temp.path().to_path_buf());
    controller.set_event_bus(bus.clone());
    planning_session_with_planner(&controller, "g46-planner", &[]);
    let id = "g46-planner-master-planner";
    let instance = controller.pty_manager.read().instance_id(id).unwrap();
    let mut events = bus.subscribe();
    controller.retire_master_planner("g46-planner", "Queen");
    assert!(agent_ids(&controller, "g46-planner").is_empty());
    assert_eq!(controller.pty_manager.read().instance_id(id), None);
    let event = tokio::time::timeout(std::time::Duration::from_secs(3), events.recv())
        .await.expect("planner emits stop event").unwrap();
    assert_eq!(event.event_type, EventType::AgentStopped, "planner retirement must not emit AgentCompleted");
    assert_eq!(event.payload["instance_id"], instance.to_string());
    let mut recorded = vec![event];
    while let Ok(event) = events.try_recv() { recorded.push(event); }
    assert!(recorded.iter().all(|event| event.event_type != EventType::AgentCompleted));
    let derived = derive_runtime_graph(None, &recorded, &[], &[], &[]);
    assert!(derived.outcomes.iter().all(|outcome| outcome.completion_evidence.is_none()
        && !matches!(outcome.status, RuntimeOutcomeStatus::Completed | RuntimeOutcomeStatus::Finalized)),
        "planner retirement must create no runtime Completion");
}
