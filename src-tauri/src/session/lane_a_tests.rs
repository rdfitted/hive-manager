use super::*;

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
