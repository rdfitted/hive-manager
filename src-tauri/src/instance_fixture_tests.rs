//! Prove the public seeder against the real persistence, overlay, and journal readers.
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use crate::{
    instance,
    pty::{AgentStatus, PtyManager},
    session::{work_state::WorkState, SessionController},
    storage::SessionStorage,
};

fn manifest() -> serde_json::Value {
    serde_json::from_str(include_str!("../../tools/render/fixtures/manifest.json")).unwrap()
}

#[test]
fn g49_isolated_resume_refuses_coordinator_and_per_agent_spawns() {
    if std::env::var_os("HM_RESUME_ISOLATION_CHILD").is_some() {
        instance::bootstrap();
        let settings = instance::settings().expect("isolated resume child");
        let storage = Arc::new(SessionStorage::new_with_base(settings.data_dir()).unwrap());
        let manager = Arc::new(parking_lot::RwLock::new(PtyManager::new()));
        let mut controller = SessionController::new(manager.clone());
        controller.set_storage(storage.clone());
        let cwd = settings.root.join("saved-worktree");
        std::fs::create_dir_all(&cwd).unwrap();
        let provider = uuid::Uuid::new_v4().to_string();
        let sid = "fixture-resume";
        let queen = "fixture-resume-queen";
        let worker = "fixture-resume-worker-1";
        let mut saved: crate::storage::PersistedSession = serde_json::from_value(serde_json::json!({
            "id":sid,"session_type":{"Hive":{"worker_count":1}},"project_path":cwd,
            "created_at":chrono::Utc::now(),"state":"Completed","default_cli":"claude",
            "qa_workers":[],"max_qa_iterations":3,"qa_timeout_secs":300,"auth_strategy":"none",
            "agents":[{"id":queen,"role":"Queen","parent_id":null,"config":{"cli":"claude","flags":[]},
                "provider_session_id":provider,"transcript_session_id":provider,"transcript_cwd":cwd}]
        })).unwrap();
        // Ended runs load metadata, including agents with saved conversations.
        // Isolation still refuses every explicit per-agent process start.
        let mut worker_info = saved.agents[0].clone();
        worker_info.id = worker.into();
        worker_info.role = "Worker(1)".into();
        worker_info.parent_id = Some(queen.into());
        saved.agents.push(worker_info);
        storage.save_session(&saved).unwrap();
        let session = controller.resume_session(sid).unwrap();
        assert_eq!(controller.list_sessions().len(), 1);
        assert_eq!(session.state, crate::session::SessionState::Completed);
        let notice = session.agent_resume_report.iter()
            .find(|notice| notice.agent_id.as_deref() == Some(queen)).unwrap();
        assert_eq!(notice.reason, "manual_resume_required");
        assert!(notice.message.contains("saved session has ended"));
        assert!(session.agents[1].provider_session_id.is_some());
        for id in [queen, worker] {
            let error = controller.resume_agent(sid, id).unwrap_err();
            assert!(error.contains("Isolated instance"), "G40 per-agent Resume must be refused: {error}");
            assert!(manager.read().instance_id(id).is_none());
            assert!(!manager.read().is_alive(id));
        }
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(".hive-instance"), "fixture").unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "instance_fixture_tests::g49_isolated_resume_refuses_coordinator_and_per_agent_spawns", "--nocapture"])
        .env("HM_RESUME_ISOLATION_CHILD", "1")
        .env(instance::DIR, temp.path()).env(instance::PORT, "18897")
        .env_remove(instance::CDP_PORT);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed; 0 failed;"));
    assert!(!instance::is_isolated(), "G40 the parent stays on its normal profile");
}

#[test]
#[ignore = "process-heavy fixture proof; run serially by CI"]
fn seeded_output_loads_through_real_storage_and_isolated_controller() {
    // Re-exec keeps bootstrap's process environment and OnceLocks away from other tests.
    if std::env::var_os("HM_SEEDED_FIXTURE_CHILD").is_some() {
        instance::bootstrap();
        let settings = instance::settings().expect("isolated test child");
        let storage = Arc::new(SessionStorage::new_with_base(settings.data_dir()).unwrap());
        let manager = Arc::new(parking_lot::RwLock::new(PtyManager::new()));
        let mut controller = SessionController::new(manager.clone());
        controller.set_storage(storage.clone());
        let manifest = manifest();
        let sessions = manifest["sessions"].as_array().unwrap();
        for session in sessions {
            let id = session["id"].as_str().unwrap();
            let stored = storage
                .load_session(id)
                .expect("actual generated session.json loads");
            assert_eq!(stored.id, id);
            assert_eq!(
                stored.agents.len(),
                session["agents"].as_array().unwrap().len()
            );
            settings.contains(Path::new(&stored.project_path)).unwrap();
            for agent in stored.agents {
                settings
                    .contains(Path::new(agent.transcript_cwd.as_deref().unwrap()))
                    .unwrap();
            }
            assert!(!storage.load_retirement_settings(id).unwrap().enabled);
        }
        instance::load_fixtures(&controller, &storage).unwrap();
        assert_eq!(controller.list_sessions().len(), sessions.len());
        let feeds = crate::transport::feed::FeedStore::new(
            storage.sessions_dir(),
            Some(settings.root.join("home/.claude/projects")),
        );
        feeds.set_codex_root(Some(settings.root.join("home/.codex/sessions")));
        for spec in sessions {
            let id = spec["id"].as_str().unwrap();
            let session = controller.get_session(id).unwrap();
            let stalled =
                controller.get_stalled_agent_reports(id, std::time::Duration::from_secs(180));
            for expected in spec["agents"].as_array().unwrap() {
                let agent_id = expected["id"].as_str().unwrap();
                let agent = session
                    .agents
                    .iter()
                    .find(|agent| agent.id == agent_id)
                    .unwrap();
                assert!(!manager.read().is_alive(agent_id));
                match expected["state"].as_str().unwrap() {
                    "finished" => {
                        assert_eq!(agent.work_state, Some(WorkState::Finished));
                        assert_eq!(agent.status, AgentStatus::Completed);
                        assert!(!stalled.iter().any(|(id, _, _)| id == agent_id));
                    }
                    "stalled" => {
                        assert_eq!(agent.work_state, Some(WorkState::Working));
                        assert_eq!(agent.status, AgentStatus::Running);
                        assert!(stalled.iter().any(|(id, _, _)| id == agent_id));
                    }
                    "exited_during_startup" => assert_eq!(
                        agent.status,
                        AgentStatus::Error("exited_during_startup:17".into())
                    ),
                    "delivery_failed" => assert!(
                        matches!(&agent.status, AgentStatus::WaitingForInput(reason) if reason.starts_with("injected_delivery_failed:"))
                    ),
                    _ => {}
                }
                assert!(feeds.is_protocol(id, agent_id).unwrap());
                let history = feeds.history(id, agent_id, None, None, 100).unwrap();
                assert_eq!(history.events.len(), 2, "durable journal is not empty");
                if expected["state"] == "markdown" {
                    let text = history.events[1].payload["text"].as_str().unwrap();
                    for key in ["tableText", "codeText", "link", "literalHtml"] {
                        assert!(text.contains(manifest["chat"][key].as_str().unwrap()));
                    }
                }
            }
            controller.sweep_dead_startup_seats(id);
            let after = controller.get_session(id).unwrap();
            for (before, after) in session.agents.iter().zip(after.agents.iter()) {
                assert_eq!(before.status, after.status);
                assert_eq!(before.work_state, after.work_state);
            }
        }
        // Advance heartbeat age, not wall time: no 180-second sleep or production clock change.
        let now = chrono::Utc::now();
        let aged_at = now - chrono::Duration::seconds(241);
        for spec in sessions {
            let id = spec["id"].as_str().unwrap();
            for agent in spec["agents"].as_array().unwrap() {
                controller.set_heartbeat_last_activity_for_test(id, agent["id"].as_str().unwrap(), aged_at);
            }
        }
        let before_maintenance: std::collections::HashMap<_, _> = sessions.iter().map(|spec| {
            let id = spec["id"].as_str().unwrap();
            (id.to_string(), (controller.get_session(id).unwrap(), controller.get_heartbeat_info(id)))
        }).collect();
        instance::maintain_fixture_heartbeats(&controller, now);
        let mut active_failures = Vec::new();
        for spec in sessions {
            let id = spec["id"].as_str().unwrap();
            let (before, previous_heartbeats) = &before_maintenance[id];
            let (snapshot, heartbeats) = controller.session_with_heartbeats(controller.get_session(id).unwrap());
            let served = controller.get_heartbeat_info(id);
            let stalled = controller.get_stalled_agent_reports(id, std::time::Duration::from_secs(180));
            for expected in spec["agents"].as_array().unwrap() {
                let agent_id = expected["id"].as_str().unwrap();
                let state = expected["state"].as_str().unwrap();
                let heartbeat = &heartbeats[agent_id];
                let reported = stalled.iter().any(|(id, _, _)| id == agent_id);
                if matches!(state, "working" | "markdown" | "idle") {
                    // This is the active API's timestamp serialization, consumed by UI staleness.
                    let serialized = serde_json::json!({"last_activity": heartbeat.last_activity.to_rfc3339()});
                    let timestamp = chrono::DateTime::parse_from_rfc3339(serialized["last_activity"].as_str().unwrap()).unwrap();
                    if reported || timestamp != now || served[agent_id].last_activity != now {
                        active_failures.push(format!("{agent_id} ({state}): reported={reported}, snapshot={}, served={}",
                            heartbeat.last_activity, served[agent_id].last_activity));
                    }
                } else {
                    assert_eq!(heartbeat.last_activity, aged_at, "{agent_id} keeps its deliberate age");
                    assert_eq!(served[agent_id].last_activity, aged_at);
                    if state == "stalled" { assert!(reported, "{agent_id} remains stalled"); }
                    if state == "finished" { assert!(!reported, "{agent_id} remains exempt"); }
                }
                let previous = before.agents.iter().find(|agent| agent.id == agent_id).unwrap();
                let current = snapshot.agents.iter().find(|agent| agent.id == agent_id).unwrap();
                assert_eq!(previous.status, current.status, "{agent_id} status is not replayed");
                assert_eq!(heartbeat.status, previous_heartbeats[agent_id].status);
                assert_eq!(heartbeat.summary, previous_heartbeats[agent_id].summary);
                assert_eq!(heartbeat.status, served[agent_id].status);
                assert_eq!(heartbeat.summary, served[agent_id].summary);
            }
        }
        assert!(active_failures.is_empty(), "active_fixture_heartbeats_stay_fresh_after_180_seconds:\n{}", active_failures.join("\n"));

        // These are the same home-derived roots that the production feed factory uses.
        let home = PathBuf::from(
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .unwrap(),
        );
        for path in [
            home.join(".codex/sessions"),
            home.join(".claude/projects"),
            PathBuf::from(std::env::var_os("CODEX_HOME").unwrap()),
            PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").unwrap()),
        ] {
            settings.contains(&path).unwrap();
            assert!(crate::instance::resolved_path(&path)
                .unwrap()
                .starts_with(crate::instance::resolved_path(&settings.root.join("home")).unwrap()));
        }
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/render/seed.mjs");
    let output = Command::new("node")
        .arg(script)
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "seeder failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "instance_fixture_tests::seeded_output_loads_through_real_storage_and_isolated_controller", "--nocapture"])
        .env("HM_SEEDED_FIXTURE_CHILD", "1")
        .env(instance::DIR, temp.path()).env(instance::PORT, "18897")
        .env_remove(instance::CDP_PORT)
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;")),
        "fixture child must run and pass exactly one test:\n{stdout}"
    );
    assert!(!instance::is_isolated(), "test parent remains normal");
    // Even an explicitly flagged overlay cannot refresh heartbeat state in a normal process.
    let overlay: instance::FixtureState = serde_json::from_slice(
        &std::fs::read(temp.path().join("fixture-state.json")).unwrap(),
    ).unwrap();
    let controller = SessionController::new(Arc::new(parking_lot::RwLock::new(PtyManager::new())));
    let old = chrono::Utc::now() - chrono::Duration::seconds(241);
    for fixture in &overlay.sessions {
        for agent in &fixture.agents {
            let heartbeat = agent.heartbeat.as_ref().unwrap();
            controller.update_heartbeat(&fixture.session_id, &agent.agent_id, &heartbeat.status, heartbeat.summary.as_deref()).unwrap();
            controller.set_heartbeat_last_activity_for_test(&fixture.session_id, &agent.agent_id, old);
        }
        controller.refresh_instance_fixture_heartbeats(fixture, chrono::Utc::now());
        instance::maintain_fixture_heartbeats(&controller, chrono::Utc::now());
        assert!(controller.get_heartbeat_info(&fixture.session_id).values().all(|heartbeat| heartbeat.last_activity == old),
            "normal processes preserve all heartbeat timestamps even for flagged fixtures");
    }
}

#[test]
fn seeded_output_remains_in_serial_ci_step() {
    let workflow = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/rust-tests.yml"),
    )
    .unwrap();
    let serial_step = workflow
        .split("      - name:")
        .find(|step| {
            step.starts_with(" Overhead benchmarks (isolated)\n")
                || step.starts_with(" Overhead benchmarks (isolated)\r\n")
        })
        .expect("isolated serial CI step must exist");
    let command = serial_step
        .lines()
        .find_map(|line| line.trim().strip_prefix("run: "))
        .expect("serial step must run cargo test");
    assert!(command.starts_with("cargo test --lib -- "));
    for required in [
        "--ignored",
        "--exact",
        "--test-threads=1",
        "instance_fixture_tests::seeded_output_loads_through_real_storage_and_isolated_controller",
    ] {
        assert!(
            command.split_whitespace().any(|arg| arg == required),
            "serial CI command must contain {required}"
        );
    }
}
