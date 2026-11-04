use super::*;

#[tokio::test]
async fn g49_resume_reconcile_uses_actual_runtimes_and_requeues_orphaned_rows() {
    let temp = tempfile::tempdir().unwrap();
    let storage =
        Arc::new(crate::storage::SessionStorage::new_with_base(temp.path().join("data")).unwrap());
    let manager = Arc::new(RwLock::new(crate::pty::PtyManager::new()));
    let mut controller = SessionController::new(manager.clone());
    controller.set_storage(storage.clone());
    let sid = uuid::Uuid::new_v4().to_string();
    let queen = format!("{sid}-queen");
    let live = format!("{sid}-worker-1");
    let orphan = format!("{sid}-worker-2");
    let agents: Vec<_> = [(&queen, "Queen"), (&live, "Worker(1)"), (&orphan, "Worker(2)")].into_iter()
        .map(|(id, role)| json!({"id":id,"role":role,"config":{"cli":"claude","flags":[]},"parent_id":null})).collect();
    let saved: crate::storage::PersistedSession = serde_json::from_value(json!({
        "id":sid,"session_type":{"Hive":{"worker_count":2}},"project_path":temp.path(),
        "created_at":chrono::Utc::now(),"agents":agents,"state":"Completed","default_cli":"claude",
        "qa_workers":[],"max_qa_iterations":3,"qa_timeout_secs":300,"auth_strategy":"none"
    }))
    .unwrap();
    storage.save_session(&saved).unwrap();
    let mut session = controller.resume_session(&sid).unwrap();
    for (id, role) in [
        (&queen, crate::pty::AgentRole::Queen),
        (
            &live,
            crate::pty::AgentRole::Worker {
                index: 1,
                parent: None,
            },
        ),
    ] {
        manager
            .read()
            .create_session(
                id.clone(),
                role,
                "claude",
                &[],
                Some(temp.path().to_str().unwrap()),
                80,
                24,
            )
            .unwrap();
    }
    // The roster deliberately disagrees with runtime liveness in both directions.
    session
        .agents
        .iter_mut()
        .find(|agent| agent.id == orphan)
        .unwrap()
        .status = crate::pty::AgentStatus::Running;
    let ids = live_resume_agent_ids(&session, &manager.read());

    let db =
        Arc::new(crate::storage::application_state::ApplicationStateDb::open_in_memory().unwrap());
    let repo = Arc::new(crate::storage::queue::QueueRepo::new(db));
    repo.ensure_schema().unwrap();
    let queue = crate::coordination::queue_manager::QueueManager::new(
        repo.clone(),
        crate::events::EventBus::new(temp.path().to_path_buf()),
    );
    for (row, worker) in [("live-row", &live), ("orphan-row", &orphan)] {
        queue
            .enqueue_worker(row, &sid, worker, "backend", "claude", json!({}), None)
            .await
            .unwrap();
        queue.claim_and_spawn(row, &sid, worker).await.unwrap();
    }
    let reclaimed = queue.reconcile(&sid, &ids).await.unwrap();
    assert_eq!(reclaimed, vec!["orphan-row".to_string()]);
    assert_eq!(ids, vec![queen.clone(), live.clone()]);
    assert_eq!(
        repo.get_row("orphan-row").unwrap().unwrap().status,
        crate::storage::queue::QueueStatus::Queued
    );
    assert_eq!(
        repo.get_row("live-row").unwrap().unwrap().status,
        crate::storage::queue::QueueStatus::Running
    );
    manager.read().kill(&queen).unwrap();
    manager.read().kill(&live).unwrap();
}
