use super::*;
use crate::pty::PtyManager;
use parking_lot::RwLock;
use std::sync::Arc;
use uuid::Uuid;

#[test]
fn g49_resume_commands_keep_saved_model_flags_and_conversation_identity() {
    let id = Uuid::new_v4().to_string();
    let claude = AgentConfig {
        cli: "claude".into(),
        model: Some("synthetic-model".into()),
        flags: vec![
            "--session-id".into(),
            Uuid::new_v4().to_string(),
            "--resume=old".into(),
            "--continue".into(),
            "--fork-session".into(),
            "--effort".into(),
            "high".into(),
        ],
        initial_prompt: Some("This original task must not run again".into()),
        ..AgentConfig::default()
    };
    let (command, args) = SessionController::build_resume_command(&claude, &id).unwrap();
    assert_eq!(command, "claude");
    assert_eq!(&args[..2], &["--resume", id.as_str()]);
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--model", "synthetic-model"]));
    assert!(args.windows(2).any(|pair| pair == ["--effort", "high"]));
    assert!(!args.iter().any(|arg| arg.contains("old")
        || ["--session-id", "--continue", "--fork-session"].contains(&arg.as_str())));
    assert!(!args.iter().any(|arg| arg.contains("original task")));
    assert_eq!(
        SessionController::transcript_key(&command, &args),
        Some(id.clone())
    );

    let codex = AgentConfig {
        cli: "codex".into(),
        model: Some("synthetic-codex".into()),
        flags: vec!["-c".into(), "model_reasoning_effort=high".into()],
        ..AgentConfig::default()
    };
    let (command, args) = SessionController::build_resume_command(&codex, &id).unwrap();
    assert_eq!(command, "codex");
    assert_eq!(&args[..2], &["resume", id.as_str()]);
    assert!(args
        .windows(2)
        .any(|pair| pair == ["-m", "synthetic-codex"]));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["-c", "model_reasoning_effort=high"]));
    assert_eq!(
        SessionController::transcript_key(&command, &args),
        Some(format!("codex-resume-{id}"))
    );
    assert!(SessionController::build_resume_command(&codex, "invalid").is_err());
    assert!(SessionController::build_resume_command(
        &AgentConfig {
            cli: "cursor".into(),
            ..AgentConfig::default()
        },
        &id
    )
    .is_err());
}

#[test]
fn g49_provider_fields_preserve_pre_change_json_without_migration() {
    let temp = tempfile::tempdir().unwrap();
    let storage =
        Arc::new(crate::storage::SessionStorage::new_with_base(temp.path().join("data")).unwrap());
    let mut controller = SessionController::new(Arc::new(RwLock::new(PtyManager::new())));
    controller.set_storage(storage.clone());
    let mut source = waiting_worker_session("legacy-claude", temp.path(), 1);
    let provider = Uuid::new_v4().to_string();
    source.agents[0].transcript_session_id = Some(provider.clone());
    let mut persisted = SessionController::session_to_persisted_snapshot(&source);
    persisted.agents[0].transcript_cwd = Some(temp.path().to_string_lossy().into_owned());
    assert_eq!(
        persisted.agents[0].provider_session_id.as_deref(),
        Some(provider.as_str())
    );
    storage.save_session(&persisted).unwrap();
    let mut old = serde_json::to_value(&persisted).unwrap();
    for agent in old["agents"].as_array_mut().unwrap() {
        agent.as_object_mut().unwrap().remove("provider_session_id");
    }
    old.as_object_mut().unwrap().remove("agent_resume_report");
    let bytes = serde_json::to_vec(&old).unwrap();
    let path = storage.session_dir(&source.id).join("session.json");
    std::fs::write(&path, &bytes).unwrap();
    let loaded = storage.load_session(&source.id).unwrap();
    assert!(loaded.agents[0].provider_session_id.is_none());
    let restored = controller.session_from_persisted(&loaded).unwrap();
    assert_eq!(
        restored.agents[0].provider_session_id.as_deref(),
        Some(provider.as_str())
    );
    assert!(restored.agent_resume_report.is_empty());
    assert_eq!(
        std::fs::read(path).unwrap(),
        bytes,
        "G49 legacy reads never migrate session.json"
    );

    let mut old_live = serde_json::to_value(&source).unwrap();
    old_live
        .as_object_mut()
        .unwrap()
        .remove("agent_resume_report");
    for agent in old_live["agents"].as_array_mut().unwrap() {
        agent.as_object_mut().unwrap().remove("provider_session_id");
    }
    let legacy: AgentInfo = serde_json::from_value(old_live["agents"][0].clone()).unwrap();
    assert!(legacy.provider_session_id.is_none());
    assert!(!old_live
        .as_object()
        .unwrap()
        .contains_key("agent_resume_report"));
}

#[test]
fn g49_old_codex_session_recovers_sidecar_id_and_persists_it_on_next_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let storage =
        Arc::new(crate::storage::SessionStorage::new_with_base(temp.path().join("data")).unwrap());
    let mut controller = SessionController::new(Arc::new(RwLock::new(PtyManager::new())));
    controller.set_storage(storage.clone());
    let mut source = waiting_worker_session("legacy-codex", temp.path(), 1);
    let agent = &mut source.agents[0];
    agent.config.cli = "codex".into();
    agent.transcript_session_id = Some("codex-prompt-synthetic".into());
    agent.transcript_spawned_at = Some(chrono::Utc::now());
    let mut persisted = SessionController::session_to_persisted_snapshot(&source);
    persisted.agents[0].transcript_cwd = Some(temp.path().to_string_lossy().into_owned());
    storage.save_session(&persisted).unwrap();
    let before = std::fs::read(storage.session_dir(&source.id).join("session.json")).unwrap();
    let provider = Uuid::new_v4().to_string();
    let sidecars = storage.session_dir(&source.id).join("agent-feeds");
    std::fs::create_dir_all(&sidecars).unwrap();
    let binding = crate::transport::codex_rollout_source::CodexBinding {
        key: source.agents[0].transcript_session_id.clone().unwrap(),
        spawned_at: source.agents[0].transcript_spawned_at.unwrap(),
        thread_id: provider.clone(),
        rollout_path: temp.path().join("synthetic-rollout.jsonl"),
    };
    std::fs::write(
        sidecars.join(format!("{}.binding", source.agents[0].id)),
        serde_json::to_vec(&binding).unwrap(),
    )
    .unwrap();
    let restored = controller
        .session_from_persisted(&storage.load_session(&source.id).unwrap())
        .unwrap();
    assert_eq!(
        restored.agents[0].provider_session_id.as_deref(),
        Some(provider.as_str())
    );
    assert_eq!(
        restored.agents[0].transcript_session_id,
        source.agents[0].transcript_session_id
    );
    assert_eq!(
        controller.session_to_persisted(&restored).agents[0]
            .provider_session_id
            .as_deref(),
        Some(provider.as_str())
    );
    assert_eq!(
        std::fs::read(storage.session_dir(&source.id).join("session.json")).unwrap(),
        before
    );
}

struct PersistenceTransport;
#[async_trait::async_trait]
impl crate::transport::AgentTransport for PersistenceTransport {
    async fn start(&self) -> Result<(), String> {
        Ok(())
    }
    async fn send(&self, _: &str) -> Result<crate::delivery::Delivery, String> {
        Ok(crate::delivery::Delivery::protocol_pending())
    }
    async fn interrupt(&self) -> Result<(), String> {
        Ok(())
    }
    async fn stop(&self) -> Result<(), String> {
        Ok(())
    }
    fn stop_sync(&self) -> Result<(), String> {
        Ok(())
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<crate::transport::AgentEvent> {
        tokio::sync::broadcast::channel(1).1
    }
    fn capabilities(&self) -> crate::transport::Capabilities {
        crate::transport::Capabilities {
            has_terminal: false,
            can_interrupt: true,
            supports_approvals: false,
            event_source: crate::transport::EventSource::Protocol,
        }
    }
}

#[test]
fn g49_protocol_uuid_survives_stop_but_never_crosses_a_new_incarnation() {
    let temp = tempfile::tempdir().unwrap();
    let storage =
        Arc::new(crate::storage::SessionStorage::new_with_base(temp.path().join("data")).unwrap());
    let manager = Arc::new(RwLock::new(PtyManager::new()));
    let registry = Arc::new(crate::transport::TransportRegistry::new(
        storage.sessions_dir(),
        None,
    ));
    manager.read().attach_transports(registry.clone());
    let mut controller = SessionController::new(manager.clone());
    controller.set_storage(storage.clone());
    let mut source = waiting_worker_session("protocol-persistence", temp.path(), 1);
    source.agents[0].config.cli = "codex".into();
    source.agents[0].transcript_session_id = Some("codex-prompt-synthetic".into());
    source.agents[0].transcript_spawned_at = Some(chrono::Utc::now());
    let agent_id = source.agents[0].id.clone();
    let provider = Uuid::new_v4().to_string();
    manager
        .read()
        .seed_spawn_cwd(&agent_id, temp.path().to_str().unwrap());
    registry
        .register(
            &source.id,
            &agent_id,
            Some(provider.clone()),
            Arc::new(PersistenceTransport),
        )
        .unwrap();
    controller
        .sessions
        .write()
        .insert(source.id.clone(), source.clone());
    assert_eq!(
        controller.get_session(&source.id).unwrap().agents[0]
            .provider_session_id
            .as_deref(),
        Some(provider.as_str())
    );
    let saved = controller.session_to_persisted(&source);
    assert_eq!(
        saved.agents[0].provider_session_id.as_deref(),
        Some(provider.as_str())
    );
    storage.save_session(&saved).unwrap();
    registry.stop_agent_sync(&source.id, &agent_id).unwrap();
    assert_eq!(
        controller.session_to_persisted(&source).agents[0]
            .provider_session_id
            .as_deref(),
        Some(provider.as_str())
    );
    source.agents[0].transcript_spawned_at =
        Some(source.agents[0].transcript_spawned_at.unwrap() + chrono::Duration::seconds(1));
    assert!(
        controller.session_to_persisted(&source).agents[0]
            .provider_session_id
            .is_none(),
        "G49 an old UUID cannot cross a new incarnation"
    );
}

fn resume_fixture(
    solo: bool,
) -> (
    tempfile::TempDir,
    SessionController,
    Arc<crate::storage::SessionStorage>,
    Session,
) {
    let temp = tempfile::tempdir().unwrap();
    let storage =
        Arc::new(crate::storage::SessionStorage::new_with_base(temp.path().join("data")).unwrap());
    let mut controller = SessionController::new(Arc::new(RwLock::new(PtyManager::new())));
    controller.set_storage(storage.clone());
    let mut source = waiting_worker_session(&Uuid::new_v4().to_string(), temp.path(), 1);
    source.state = SessionState::Completed;
    source.agents[0].transcript_session_id = Some(Uuid::new_v4().to_string());
    source.agents[0].transcript_spawned_at = Some(chrono::Utc::now() - chrono::Duration::days(1));
    if solo {
        source.session_type = SessionType::Solo {
            cli: "claude".into(),
            model: None,
        };
        source.agents[0].role = AgentRole::Worker {
            index: 1,
            parent: None,
        };
        source.agents[0].parent_id = None;
        let mut evaluator = source.agents[0].clone();
        evaluator.id = format!("{}-evaluator", source.id);
        evaluator.role = AgentRole::Evaluator;
        evaluator.transcript_session_id = Some(Uuid::new_v4().to_string());
        source.agents.push(evaluator);
    } else {
        source.session_type = SessionType::Hive { worker_count: 2 };
        let mut queen = source.agents[0].clone();
        queen.id = format!("{}-queen", source.id);
        queen.role = AgentRole::Queen;
        queen.parent_id = None;
        queen.transcript_session_id = Some(Uuid::new_v4().to_string());
        source.agents.insert(0, queen);
        let mut missing = source.agents[1].clone();
        missing.id = format!("{}-worker-2", source.id);
        missing.role = AgentRole::Worker {
            index: 2,
            parent: Some(format!("{}-queen", source.id)),
        };
        missing.transcript_session_id = None;
        missing.provider_session_id = None;
        source.agents.push(missing);
    }
    let saved_cwd = temp.path().join("saved worktree");
    std::fs::create_dir(&saved_cwd).unwrap();
    let mut persisted = SessionController::session_to_persisted_snapshot(&source);
    for agent in &mut persisted.agents {
        agent.transcript_cwd = Some(saved_cwd.to_string_lossy().into_owned());
    }
    storage.save_session(&persisted).unwrap();
    (temp, controller, storage, source)
}

#[test]
fn g49_load_resumes_only_queen_then_explicitly_resumes_one_worker_in_saved_cwd() {
    let (temp, controller, storage, source) = resume_fixture(false);
    let queen = source.agents[0].id.clone();
    let worker = source.agents[1].id.clone();
    let missing = source.agents[2].id.clone();
    let resumed = controller.resume_session(&source.id).unwrap();
    let queen_instance = controller.pty_manager.read().instance_id(&queen).unwrap();
    assert!(controller.pty_manager.read().is_alive(&queen));
    assert!(
        controller.pty_manager.read().instance_id(&worker).is_none(),
        "G49 Load never mass-spawns workers"
    );
    assert!(controller
        .pty_manager
        .read()
        .instance_id(&missing)
        .is_none());
    assert_eq!(resumed.agent_resume_report.len(), 1);
    assert!(resumed.agent_resume_report[0].contains(&missing));
    assert!(matches!(resumed.agents[1].status, AgentStatus::Completed));
    let resumed = controller.resume_agent(&source.id, &worker).unwrap();
    assert_eq!(
        controller.pty_manager.read().instance_id(&queen),
        Some(queen_instance)
    );
    let worker_instance = controller.pty_manager.read().instance_id(&worker).unwrap();
    assert_ne!(worker_instance, queen_instance);
    let saved_cwd = temp
        .path()
        .join("saved worktree")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        controller.pty_manager.read().spawn_cwd(&worker),
        Some(saved_cwd.clone())
    );
    let saved = storage.load_session(&source.id).unwrap();
    let agent = saved
        .agents
        .iter()
        .find(|agent| agent.id == worker)
        .unwrap();
    assert_eq!(agent.transcript_cwd, Some(saved_cwd));
    assert_eq!(
        agent.provider_session_id,
        source.agents[1].transcript_session_id
    );
    assert_eq!(
        agent.transcript_spawned_at,
        controller.pty_manager.read().spawned_at(&worker)
    );
    assert!(matches!(resumed.agents[1].status, AgentStatus::Running));
    assert!(controller
        .resume_agent(&source.id, &worker)
        .unwrap_err()
        .contains("already running"));
    assert_eq!(
        controller.pty_manager.read().instance_id(&worker),
        Some(worker_instance)
    );
    assert!(controller
        .resume_agent(&source.id, &missing)
        .unwrap_err()
        .contains("no saved conversation ID"));
    controller.pty_manager.read().kill(&queen).unwrap();
    controller.pty_manager.read().kill(&worker).unwrap();
}

#[test]
fn g51_resume_preserves_concurrent_roster_status_activity_and_target_metadata() {
    let (_temp, controller, storage, source) = resume_fixture(false);
    let session_id = source.id.clone();
    let worker = source.agents[1].id.clone();
    let other = source.agents[0].id.clone();
    controller.resume_session(&session_id).unwrap();
    let before = controller.sessions.read()[&session_id].clone();
    let newer_activity = chrono::Utc::now() + chrono::Duration::days(1);
    let mut added = before.agents[2].clone();
    added.id = format!("{session_id}-worker-concurrent");
    let added_id = added.id.clone();
    let (prepared_tx, prepared_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    SessionController::test_sync_hook(&session_id, "resume-agent-prepared", Some(Box::new(move || {
        prepared_tx.send(()).unwrap();
        release_rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
    })));
    let resumed = std::thread::scope(|scope| {
        let resume = scope.spawn(|| controller.resume_agent(&session_id, &worker));
        prepared_rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        controller.update_heartbeat(&session_id, &other, "working", Some("concurrent activity")).unwrap();
        controller.record_injection_submission(&session_id, &other, chrono::Utc::now(), true).unwrap();
        {
            let mut sessions = controller.sessions.write();
            let current = sessions.get_mut(&session_id).unwrap();
            current.agents.push(added);
            if let SessionType::Hive { worker_count } = &mut current.session_type {
                *worker_count += 1;
            } else { panic!("expected Hive fixture"); }
            current.agents.iter_mut().find(|agent| agent.id == worker).unwrap().config.label = Some("concurrent label".into());
            current.last_activity_at = newer_activity;
            current.name = Some("concurrent session name".into());
        }
        release_tx.send(()).unwrap();
        resume.join().unwrap().unwrap()
    });
    for snapshot in [&resumed, &controller.sessions.read()[&session_id]] {
        assert!(snapshot.agents.iter().any(|agent| agent.id == added_id), "CR3 concurrent worker survives Resume");
        assert!(matches!(snapshot.session_type, SessionType::Hive { worker_count: 3 }));
        assert!(matches!(&snapshot.agents.iter().find(|agent| agent.id == other).unwrap().status,
            AgentStatus::WaitingForInput(reason) if reason.starts_with(SessionController::AWAITING_INJECTED_TURN_PREFIX)));
        assert_eq!(snapshot.last_activity_at, newer_activity);
        assert_eq!(snapshot.name.as_deref(), Some("concurrent session name"));
        let target = snapshot.agents.iter().find(|agent| agent.id == worker).unwrap();
        assert_eq!(target.config.label.as_deref(), Some("concurrent label"));
        assert!(matches!(target.status, AgentStatus::Running));
        assert_eq!(target.provider_session_id, source.agents[1].transcript_session_id);
        assert_eq!(target.transcript_spawned_at, controller.pty_manager.read().spawned_at(&worker));
        assert_ne!(target.transcript_spawned_at, before.agents[1].transcript_spawned_at);
        assert!(snapshot.agent_resume_report.iter().any(|line| line.contains(&added_id)));
    }
    let saved = storage.load_session(&session_id).unwrap();
    assert!(saved.agents.iter().any(|agent| agent.id == added_id));
    assert_eq!(saved.agents.len(), before.agents.len() + 1);
    assert!(matches!(saved.session_type, crate::storage::SessionTypeInfo::Hive { worker_count: 3 }));
    assert_eq!(saved.last_activity_at, Some(newer_activity));
    assert_eq!(saved.name.as_deref(), Some("concurrent session name"));
    let target = saved.agents.iter().find(|agent| agent.id == worker).unwrap();
    assert_eq!(target.config.label.as_deref(), Some("concurrent label"));
    assert_eq!(target.provider_session_id, source.agents[1].transcript_session_id);
    assert_eq!(target.transcript_spawned_at, controller.pty_manager.read().spawned_at(&worker));
    controller.pty_manager.read().kill(&source.agents[0].id).unwrap();
    controller.pty_manager.read().kill(&worker).unwrap();
}

#[test]
fn g49_solo_load_resumes_original_agent_without_starting_evaluator() {
    let (_temp, controller, _storage, source) = resume_fixture(true);
    let resumed = controller.resume_session(&source.id).unwrap();
    assert!(controller.pty_manager.read().is_alive(&source.agents[0].id));
    assert!(controller
        .pty_manager
        .read()
        .instance_id(&source.agents[1].id)
        .is_none());
    assert!(matches!(resumed.state, SessionState::Running));
    controller
        .pty_manager
        .read()
        .kill(&source.agents[0].id)
        .unwrap();
}

// The callback runs after the new instance is published and before Resume merges.
// Channels make these races deterministic without holding either roster lock.
fn g51_resume_during(
    update: impl FnOnce(&SessionController, &str, &str),
) -> (
    tempfile::TempDir,
    SessionController,
    Arc<crate::storage::SessionStorage>,
    Session,
    Result<Session, String>,
) {
    let (temp, controller, storage, source) = resume_fixture(false);
    let session_id = source.id.clone();
    let worker = source.agents[1].id.clone();
    controller.resume_session(&session_id).unwrap();
    assert_eq!(controller.sessions.read()[&session_id].state, SessionState::Running);
    let (prepared_tx, prepared_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    SessionController::test_sync_hook(&session_id, "resume-agent-prepared", Some(Box::new(move || {
        prepared_tx.send(()).unwrap();
        release_rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
    })));
    let resumed = std::thread::scope(|scope| {
        let resume = scope.spawn(|| controller.resume_agent(&session_id, &worker));
        prepared_rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        update(&controller, &session_id, &worker);
        release_tx.send(()).unwrap();
        resume.join().unwrap()
    });
    (temp, controller, storage, source, resumed)
}

fn g51_cleanup_resume(controller: &SessionController, source: &Session) {
    for agent in &source.agents {
        controller.pty_manager.read().kill(&agent.id).unwrap();
    }
}

#[test]
fn g51_resume_same_target_stop_cannot_resurrect_removed_runtime() {
    let (_temp, controller, storage, source, resumed) = g51_resume_during(|controller, id, worker| {
        let instance = controller.pty_manager.read().instance_id(worker).unwrap();
        controller.stop_agent_instance(id, worker, instance).unwrap();
    });
    let worker = &source.agents[1].id;
    let current = controller.sessions.read()[&source.id].clone();
    let saved = storage.load_session(&source.id).unwrap();
    assert_eq!(controller.pty_manager.read().instance_id(worker), None);
    g51_cleanup_resume(&controller, &source);
    assert!(resumed.is_err(), "Resume must reject an instance stopped before its merge");
    let target = current.agents.iter().find(|agent| &agent.id == worker).unwrap();
    assert_eq!(target.status, AgentStatus::WaitingForInput("stopped_by_operator".into()));
    assert_eq!(target.work_state, Some(crate::session::work_state::WorkState::Working));
    assert_eq!(saved.state, SessionController::session_to_persisted_snapshot(&current).state);
}

fn g51_check_same_target_injection(fail_before_merge: bool) {
    let mut submission = None;
    let (_temp, controller, _storage, source, resumed) = g51_resume_during(|controller, id, worker| {
        let instance = controller.pty_manager.read().instance_id(worker).unwrap();
        let submitted_at = chrono::Utc::now().max(controller.pty_manager.read().spawned_at(worker).unwrap());
        controller.record_instance_injection_submission(id, worker, instance, submitted_at, true).unwrap();
        if fail_before_merge {
            controller.record_instance_injection_delivery(id, worker, instance, submitted_at,
                &crate::delivery::Delivery::timeout(true)).unwrap();
        }
        submission = Some((instance, submitted_at));
    });
    let worker = &source.agents[1].id;
    let (instance, submitted_at) = submission.unwrap();
    let pending = format!("{}{}", SessionController::AWAITING_INJECTED_TURN_PREFIX, submitted_at.to_rfc3339());
    let failed = format!("injected_delivery_failed:transcript-timeout:{}", submitted_at.to_rfc3339());
    let expected = AgentStatus::WaitingForInput(if fail_before_merge { failed.clone() } else { pending });
    let resumed = resumed.unwrap();
    let current = controller.sessions.read()[&source.id].clone();
    // Resolve after the merge too: losing the pending marker also discards this timeout.
    controller.record_instance_injection_delivery(&source.id, worker, instance, submitted_at,
        &crate::delivery::Delivery::timeout(true)).unwrap();
    let after_delivery = controller.sessions.read()[&source.id].clone();
    let spawned_at = controller.pty_manager.read().spawned_at(worker).unwrap();
    g51_cleanup_resume(&controller, &source);
    for snapshot in [&resumed, &current] {
        let target = snapshot.agents.iter().find(|agent| &agent.id == worker).unwrap();
        assert_eq!(target.status, expected, "same-instance delivery state must survive Resume");
        assert_eq!(target.transcript_spawned_at, Some(spawned_at));
        assert_eq!(target.provider_session_id, source.agents[1].transcript_session_id);
    }
    assert_eq!(after_delivery.agents.iter().find(|agent| &agent.id == worker).unwrap().status,
        AgentStatus::WaitingForInput(failed), "visible timeout must still resolve the exact submission");
}

#[test]
fn g51_resume_same_target_injection_preserves_pending_delivery() {
    g51_check_same_target_injection(false);
}

#[test]
fn g51_resume_same_target_injection_preserves_delivery_failure_before_merge() {
    g51_check_same_target_injection(true);
}

#[tokio::test]
async fn g51_resume_preserves_concurrent_qa_state_transition() {
    let (_temp, controller, storage, source, resumed) = g51_resume_during(|controller, id, _| {
        controller.begin_qa_window(id).unwrap();
        assert!(matches!(controller.sessions.read()[id].state, SessionState::QaInProgress { .. }));
    });
    controller.cancel_qa_timeout(&source.id);
    let current = controller.sessions.read()[&source.id].clone();
    let saved = storage.load_session(&source.id).unwrap();
    g51_cleanup_resume(&controller, &source);
    assert!(matches!(resumed.unwrap().state, SessionState::QaInProgress { .. }));
    assert!(matches!(current.state, SessionState::QaInProgress { .. }));
    assert_eq!(saved.state, SessionController::session_to_persisted_snapshot(&current).state);
}

#[test]
fn g51_resume_preserves_concurrent_remediation_and_completion_states() {
    for state in [SessionState::PrinceRemediation, SessionState::QaPassed,
        SessionState::Completed, SessionState::Paused] {
        let (_temp, controller, storage, source, resumed) = g51_resume_during(|controller, id, _| {
            controller.sessions.write().get_mut(id).unwrap().state = state.clone();
        });
        let current = controller.sessions.read()[&source.id].clone();
        let saved = storage.load_session(&source.id).unwrap();
        g51_cleanup_resume(&controller, &source);
        assert_eq!(resumed.unwrap().state, state);
        assert_eq!(current.state, state);
        assert_eq!(saved.state, SessionController::session_to_persisted_snapshot(&current).state);
    }
}

fn g51_resume_with_manager_contention(release_on_busy: bool) -> (
    tempfile::TempDir, SessionController, Session, Result<Session, String>, bool,
) {
    let (temp, controller, _storage, source) = resume_fixture(false);
    let session_id = source.id.clone();
    let worker = source.agents[1].id.clone();
    controller.resume_session(&session_id).unwrap();
    let mut held = Some(controller.pty_manager.read());
    let (events_tx, events_rx) = std::sync::mpsc::channel::<Option<Result<Session, String>>>();
    let busy_tx = events_tx.clone();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    SessionController::test_sync_hook(&session_id, "resume-manager-busy", Some(Box::new(move || {
        busy_tx.send(None).unwrap();
        if release_on_busy {
            release_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        }
    })));
    let (resumed, saw_busy) = std::thread::scope(|scope| {
        let resume = scope.spawn(|| {
            let result = controller.resume_agent(&session_id, &worker);
            events_tx.send(Some(result)).unwrap();
        });
        let first = events_rx.recv_timeout(std::time::Duration::from_secs(5));
        let saw_busy = matches!(&first, Ok(None));
        if release_on_busy {
            drop(held.take());
            let _ = release_tx.send(());
        }
        let finished = match first {
            Ok(Some(result)) => Ok(Some(result)),
            Ok(None) => events_rx.recv_timeout(std::time::Duration::from_secs(5)),
            Err(error) => Err(error),
        };
        // Always release before joining, even under a blocking-lock mutation.
        drop(held.take());
        let _ = release_tx.send(());
        resume.join().unwrap();
        (finished.expect("Resume must finish within its contention budget").unwrap(), saw_busy)
    });
    drop(held);
    // Consume an unused hook if a mutation bypassed the contention branch.
    SessionController::test_sync_hook(&session_id, "resume-manager-busy", None);
    (temp, controller, source, resumed, saw_busy)
}

#[test]
fn g51_resume_retries_manager_contention_released_at_sync_hook() {
    let (_temp, controller, source, resumed, saw_busy) = g51_resume_with_manager_contention(true);
    let instance = controller.pty_manager.read().instance_id(&source.agents[1].id);
    g51_cleanup_resume(&controller, &source);
    assert!(saw_busy, "the test must reach the failed exclusive acquisition");
    assert!(instance.is_some());
    assert!(matches!(resumed.unwrap().agents[1].status, AgentStatus::Running));
}

#[test]
fn g51_resume_manager_contention_past_budget_is_retryable_and_creates_nothing() {
    let (_temp, controller, source, resumed, saw_busy) = g51_resume_with_manager_contention(false);
    let instance = controller.pty_manager.read().instance_id(&source.agents[1].id);
    g51_cleanup_resume(&controller, &source);
    assert!(saw_busy, "the test must hold contention throughout the retry budget");
    assert_eq!(instance, None, "a busy Resume must not create a runtime");
    assert!(resumed.unwrap_err().contains("runtime manager remained busy after waiting; try again"));
}

#[test]
fn g51_resume_raw_input_without_delivery_marker_updates_runtime_status() {
    let (_temp, controller, _storage, source, resumed) = g51_resume_during(|controller, id, worker| {
        let instance = controller.pty_manager.read().instance_id(worker).unwrap();
        controller.record_input_time_for_instance(id, worker, instance, chrono::Utc::now());
    });
    let current = controller.sessions.read()[&source.id].clone();
    g51_cleanup_resume(&controller, &source);
    assert_eq!(resumed.unwrap().agents[1].status, AgentStatus::Running);
    assert_eq!(current.agents[1].status, AgentStatus::Running);
}

#[test]
fn g49_missing_worktree_or_saved_cwd_refuses_without_project_root_fallback() {
    for missing_path in [false, true] {
        let (temp, controller, storage, source) = resume_fixture(false);
        let mut saved = storage.load_session(&source.id).unwrap();
        let queen = saved
            .agents
            .iter_mut()
            .find(|agent| agent.role == "Queen")
            .unwrap();
        queen.transcript_cwd = missing_path.then(|| {
            temp.path()
                .join("gone worktree")
                .to_string_lossy()
                .into_owned()
        });
        storage.save_session(&saved).unwrap();
        let error = controller.resume_session(&source.id).unwrap_err();
        assert!(
            error.contains("working directory"),
            "G49 clear refusal: {error}"
        );
        assert!(
            temp.path().is_dir(),
            "the project root is available but must never be used instead"
        );
        assert!(controller.sessions.read().is_empty());
        for agent in &source.agents {
            assert!(controller
                .pty_manager
                .read()
                .instance_id(&agent.id)
                .is_none());
        }
    }
}

#[test]
fn g49_load_without_coordinator_id_names_it_and_keeps_all_agents_stopped() {
    let (_temp, controller, storage, source) = resume_fixture(false);
    let mut saved = storage.load_session(&source.id).unwrap();
    let queen = saved
        .agents
        .iter_mut()
        .find(|agent| agent.role == "Queen")
        .unwrap();
    queen.provider_session_id = None;
    queen.transcript_session_id = None;
    storage.save_session(&saved).unwrap();
    let path = storage.session_dir(&source.id).join("session.json");
    let before = std::fs::read(&path).unwrap();
    let resumed = controller.resume_session(&source.id).unwrap();
    assert!(resumed
        .agent_resume_report
        .iter()
        .any(|line| line.contains(&source.agents[0].id)));
    for agent in &source.agents {
        assert!(controller
            .pty_manager
            .read()
            .instance_id(&agent.id)
            .is_none());
    }
    assert_eq!(
        std::fs::read(path).unwrap(),
        before,
        "metadata-only legacy loads do not rewrite storage"
    );
}

#[test]
fn g49_pre_resume_receipt_cannot_mark_or_write_the_respawned_instance() {
    let (_temp, controller, storage, source) = resume_fixture(false);
    controller.resume_session(&source.id).unwrap();
    let worker = source.agents[1].id.clone();
    controller.resume_agent(&source.id, &worker).unwrap();
    let injection = crate::coordination::InjectionManager::new(
        controller.pty_manager.clone(),
        crate::storage::SessionStorage::new_with_base(storage.base_dir().to_path_buf()).unwrap(),
    );
    let mut receipt = injection
        .operator_inject(&source.id, &worker, "synthetic old turn", true)
        .unwrap();
    controller.stop_agent(&source.id, &worker).unwrap();
    assert!(
        matches!(&controller.get_session(&source.id).unwrap().agents[1].status,
        AgentStatus::WaitingForInput(reason) if reason == "stopped_by_operator")
    );
    controller.resume_agent(&source.id, &worker).unwrap();
    let new_instance = controller.pty_manager.read().instance_id(&worker).unwrap();
    assert_ne!(receipt.instance_id, new_instance);
    receipt.submitted_at = chrono::Utc::now(); // A new timestamp cannot rescue an old UUID.
    controller.record_input_time_for_instance(
        &source.id,
        &worker,
        receipt.instance_id,
        receipt.submitted_at,
    );
    controller
        .record_instance_injection_submission(
            &source.id,
            &worker,
            receipt.instance_id,
            receipt.submitted_at,
            true,
        )
        .unwrap();
    assert!(controller.latest_input_at(&source.id, &worker).is_none());
    assert!(matches!(
        controller.get_session(&source.id).unwrap().agents[1].status,
        AgentStatus::Running
    ));
    let manager = controller.pty_manager.read();
    let fence = manager.input_fence(&worker);
    assert!(manager
        .write_instance(&worker, receipt.instance_id, b"stale retry")
        .is_err());
    assert!(manager
        .submit_instance(&worker, receipt.instance_id, b"stale submit")
        .is_err());
    assert_eq!(manager.input_fence(&worker), fence);
    // Errors and the unchanged input fence above cover stale input on every
    // platform; the Windows stub also exposes the exact bytes it received.
    #[cfg(windows)]
    {
        assert!(manager.write_records_for_test(&worker).unwrap().is_empty());
    }
    manager.kill(&source.agents[0].id).unwrap();
    manager.kill(&worker).unwrap();
}

#[cfg(windows)]
#[test]
fn g49_real_pty_resume_uses_only_fake_cli_shims_in_saved_worktrees() {
    if let Some(root) = std::env::var_os("HM_RESUME_PTY_CHILD") {
        let root = std::path::PathBuf::from(root);
        for cli in ["claude", "codex"] {
            let (temp, controller, storage, source) = resume_fixture(true);
            let mut saved = storage.load_session(&source.id).unwrap();
            let agent = &mut saved.agents[0];
            agent.config.cli = cli.into();
            agent.config.model = Some("synthetic-model".into());
            agent.config.flags = if cli == "claude" {
                vec![
                    "--real-pty-fixture".into(),
                    "--effort".into(),
                    "high".into(),
                ]
            } else {
                vec![
                    "--real-pty-fixture".into(),
                    "-c".into(),
                    "model_reasoning_effort=high".into(),
                ]
            };
            if cli == "codex" {
                agent.transcript_session_id = Some("codex-prompt-synthetic".into());
            }
            let provider = agent.provider_session_id.clone().unwrap();
            let cwd = temp.path().join("saved worktree");
            // ConPTY reconstructs PATH from the registry. CMD searches its cwd
            // first, so place the fake there rather than trusting a PATH override.
            let shim = cwd.join(format!("{cli}.cmd"));
            if std::env::var_os("HM_RESUME_PTY_MISSING_SHIM").is_none() {
                std::fs::write(&shim, format!("@echo off\r\nset \"SYNTHETIC_CLI={cli}\"\r\npowershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\" %*\r\n", root.join("resume.ps1").display())).unwrap();
            }
            assert!(
                shim.is_file(),
                "G49 fake shim missing; refusing to launch a provider"
            );
            let resolved = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "where", cli])
                .current_dir(&cwd)
                .output()
                .unwrap();
            assert!(resolved.status.success());
            assert_eq!(
                std::path::Path::new(
                    String::from_utf8_lossy(&resolved.stdout)
                        .lines()
                        .next()
                        .unwrap()
                        .trim()
                )
                .canonicalize()
                .unwrap(),
                shim.canonicalize().unwrap(),
                "G49 only the saved-cwd fake CLI may resolve"
            );
            let remembered = format!("synthetic-{cli}-memory");
            std::fs::write(
                cwd.join("memory.json"),
                serde_json::json!({(provider.clone()): remembered}).to_string(),
            )
            .unwrap();
            storage.save_session(&saved).unwrap();
            let resumed = controller.resume_session(&source.id).unwrap();
            let agent_id = &resumed.agents[0].id;
            let captured = controller
                .pty_manager
                .read()
                .spawn_args_for_test(agent_id)
                .unwrap();
            assert!(
                captured.iter().any(|arg| arg == "--real-pty-fixture"),
                "G49 real-PTY opt-in reaches spawn: {captured:?}"
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
            let result = loop {
                if let Ok(bytes) = std::fs::read(root.join(format!("{cli}-result.json"))) {
                    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                        break value;
                    }
                }
                assert!(
                    controller.pty_manager.read().is_alive(agent_id),
                    "G49 fake {cli} shim exited before its complete result"
                );
                if std::time::Instant::now() >= deadline {
                    let output = controller.pty_manager.read().recent_output(agent_id);
                    controller.pty_manager.read().kill(agent_id).unwrap();
                    panic!("G49 fake shim signal failsafe for {cli}; argv={captured:?}; output={output:?}");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            controller.pty_manager.read().kill(agent_id).unwrap();
            assert_eq!(result["fixture"], "g49-fake-resume");
            assert_eq!(result["cli"], cli);
            assert_eq!(result["provider"], provider);
            assert_eq!(result["remembered"], remembered);
            assert_eq!(
                std::path::Path::new(result["cwd"].as_str().unwrap())
                    .canonicalize()
                    .unwrap(),
                cwd.canonicalize().unwrap()
            );
            let args: Vec<&str> = result["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect();
            let flag = if cli == "claude" {
                "--resume"
            } else {
                "resume"
            };
            assert!(args
                .windows(2)
                .any(|pair| pair == [flag, provider.as_str()]));
            let model_flag = if cli == "claude" { "--model" } else { "-m" };
            assert!(args
                .windows(2)
                .any(|pair| pair == [model_flag, "synthetic-model"]));
            assert!(!args.contains(&"--session-id"));
            assert!(!args.contains(&"--real-pty-fixture"));
            if cli == "claude" {
                assert!(args.windows(2).any(|pair| pair == ["--effort", "high"]));
            } else {
                assert!(args
                    .windows(2)
                    .any(|pair| pair == ["-c", "model_reasoning_effort=high"]));
            }
        }
        return;
    }
    // A separate exact-test child contains environment overrides. The actual
    // ConPTY command resolves its fake shim from the synthetic saved cwd above.
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("resume.ps1"), r#"
$ErrorActionPreference='Stop'
$cli=$env:SYNTHETIC_CLI
$flag=if($cli -eq 'claude'){'--resume'}else{'resume'}
$index=[Array]::IndexOf($args,$flag)
if($index -lt 0){exit 9}
$provider=$args[$index+1]
$memory=Get-Content -LiteralPath 'memory.json' -Raw | ConvertFrom-Json
$remembered=$memory.$provider
if(!$remembered){exit 10}
$result=@{fixture='g49-fake-resume';cli=$cli;provider=$provider;remembered=$remembered;cwd=[Environment]::CurrentDirectory;args=@($args)}|ConvertTo-Json -Compress -Depth 8
$path=Join-Path $PSScriptRoot "$cli-result.json"
[IO.File]::WriteAllText("$path.pending",$result)
[IO.File]::Move("$path.pending",$path)
Start-Sleep -Seconds 300
"#).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "session::controller::tests::lane_e_tests::g49_real_pty_resume_uses_only_fake_cli_shims_in_saved_worktrees", "--nocapture"])
        .env("HM_RESUME_PTY_CHILD", root.path())
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed; 0 failed;")
    );
}
