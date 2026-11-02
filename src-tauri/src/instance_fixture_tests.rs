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
        .args(["--exact", "instance_fixture_tests::seeded_output_loads_through_real_storage_and_isolated_controller", "--nocapture"])
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
    assert!(!instance::is_isolated(), "test parent remains normal");
}
