//! #466: the trust-prompt sweep against captured CLI screens, end to end through a stub
//! PTY, the session roster, the event log and the Queen's inbox. The fixtures are real
//! captures from scratch worktrees, with the account email and user name scrubbed.
//! Hive Manager only surfaces these prompts; no test here may see a keystroke.
use super::tests::make_test_session_with_agents;
use super::tests_agent_events::setup;
use crate::cli::folder_trust::managed_worktree_fixture;
use crate::http::state::AppState;
use crate::pty::trust_prompt::tests::{AGY_AFTER, AGY_PROMPT, CLAUDE_PROMPT, CODEX_PROMPT};
use crate::pty::{AgentRole, AgentStatus};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SESSION: &str = "00000000-0000-4000-8000-000000000466";

fn queen() -> String {
    format!("{SESSION}-queen")
}

fn worker(n: u8) -> String {
    format!("{SESSION}-worker-{n}")
}

struct Hive {
    temp: tempfile::TempDir,
    state: Arc<AppState>,
}

impl Hive {
    /// A worktree Hive Manager would have made for `name` in this session's project.
    fn worktree(&self, name: &str) -> String {
        let (_, cwd) = managed_worktree_fixture(self.temp.path(), "project", SESSION, name);
        cwd.to_string_lossy().into_owned()
    }

    /// Spawn `n` on a stub PTY in `cwd` as `cli`.
    fn spawn(&self, n: u8, cli: &str, cwd: &str) {
        let command = if cli == "antigravity" { "agy" } else { cli };
        let pty = self.state.pty_manager.read();
        let _ = pty.kill(&worker(n));
        pty.create_session(worker(n), AgentRole::Worker { index: n, parent: None }, command, &[], Some(cwd), 120, 40)
            .unwrap();
    }
}

fn hive(clis: &[&str]) -> Hive {
    let (temp, state, _app) = setup();
    state.session_controller.write().set_event_bus(state.event_bus.clone());
    let mut ids = vec![queen()];
    ids.extend((1..=clis.len() as u8).map(worker));
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut session = make_test_session_with_agents(SESSION, "fixture", &id_refs);
    session.project_path = temp.path().join("project");
    session.agents[0].role = AgentRole::Queen;
    for (agent, cli) in session.agents[1..].iter_mut().zip(clis) {
        agent.config.cli = cli.to_string();
    }
    state.session_controller.read().insert_test_session(session);
    std::fs::create_dir_all(state.storage.session_dir(SESSION)).unwrap();
    Hive { temp, state }
}

fn status(state: &AppState, id: &str) -> AgentStatus {
    let controller = state.session_controller.read();
    let session = controller.get_session(SESSION).unwrap();
    session.agents.iter().find(|agent| agent.id == id).unwrap().status.clone()
}

fn set_status(state: &AppState, id: &str, status: AgentStatus) {
    let controller = state.session_controller.read();
    let mut session = controller.get_session(SESSION).unwrap();
    session.agents.iter_mut().find(|agent| agent.id == id).unwrap().status = status;
    controller.insert_test_session(session);
}

fn sweep(state: &AppState, at: Instant) -> Vec<(String, &'static str)> {
    let mut outcomes: Vec<_> = state.session_controller.read().sweep_trust_prompts_at(SESSION, at)
        .into_iter().map(|outcome| (outcome.agent_id, outcome.action)).collect();
    outcomes.sort();
    outcomes
}

fn output(state: &AppState, id: &str, bytes: &[u8]) {
    state.pty_manager.read().record_output_for_test(id, bytes);
}

fn writes(state: &AppState, id: &str) -> usize {
    state.pty_manager.read().write_records_for_test(id).unwrap().len()
}

fn trust_events(state: &AppState) -> Vec<serde_json::Value> {
    std::fs::read_to_string(state.storage.base_dir().join(SESSION).join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["payload"]["prompt"] == "trust_prompt")
        .collect()
}

async fn wait_for(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn inbox(state: &AppState, id: &str) -> String {
    std::fs::read_to_string(state.storage.session_dir(SESSION).join("conversations").join(format!("{id}.md")))
        .unwrap_or_default()
}

fn waiting() -> AgentStatus {
    AgentStatus::WaitingForInput("trust_prompt".into())
}

#[tokio::test]
async fn a_prompt_is_surfaced_once_with_status_event_and_notice_and_never_answered() {
    let h = hive(&["antigravity"]);
    let cwd = h.worktree("worker-1");
    h.spawn(1, "antigravity", &cwd);
    let t0 = Instant::now();
    assert!(sweep(&h.state, t0).is_empty(), "nothing on screen yet");

    output(&h.state, &worker(1), AGY_PROMPT);
    assert_eq!(sweep(&h.state, t0), [(worker(1), "detected")]);
    assert_eq!(status(&h.state, &worker(1)), waiting());
    assert!(sweep(&h.state, t0 + Duration::from_secs(60)).is_empty(), "surfaced once, not looped");

    // The operator answers it in the terminal; the CLI carries on.
    output(&h.state, &worker(1), AGY_AFTER);
    assert_eq!(sweep(&h.state, t0 + Duration::from_secs(65)), [(worker(1), "cleared")]);
    assert_eq!(status(&h.state, &worker(1)), AgentStatus::Running);
    assert_eq!(writes(&h.state, &worker(1)), 0, "Hive Manager never typed into the terminal");

    wait_for(|| trust_events(&h.state).len() == 2).await;
    let events = trust_events(&h.state);
    let kinds: Vec<_> = events.iter()
        .map(|event| (event["event_type"].as_str().unwrap().to_string(), event["payload"]["action"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(kinds, [
        ("agent_waiting_input".to_string(), "detected".to_string()),
        ("agent_input_resolved".to_string(), "cleared".to_string()),
    ], "the clear never reads as a second wait");
    assert!(events[0]["payload"]["instance_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(events[0]["payload"]["instance_id"], events[1]["payload"]["instance_id"]);
    assert_eq!(events[0]["agent_id"], worker(1));
    assert_eq!(events[0]["severity"], "warning");
    assert_eq!(events[0]["payload"]["adapter"], "antigravity");
    assert_eq!(events[0]["payload"]["path"], cwd, "the known spawn folder");
    wait_for(|| !inbox(&h.state, &queen()).is_empty()).await;
    let notice = inbox(&h.state, &queen());
    assert!(notice.contains(&format!("folder-trust prompt in {cwd}")), "{notice}");
    assert!(!notice.contains("hm466-scratch"), "screen text never reaches the Queen: {notice}");
}

#[tokio::test]
async fn codex_claude_and_agy_prompts_are_all_surfaced_without_input() {
    let h = hive(&["antigravity", "codex", "claude"]);
    for (n, cli) in [(1, "antigravity"), (2, "codex"), (3, "claude")] {
        let cwd = h.worktree(&format!("worker-{n}"));
        h.spawn(n, cli, &cwd);
    }
    output(&h.state, &worker(1), AGY_PROMPT);
    output(&h.state, &worker(2), CODEX_PROMPT);
    output(&h.state, &worker(3), CLAUDE_PROMPT);
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "detected"), (worker(2), "detected"), (worker(3), "detected")]);
    for n in 1..=3 {
        assert_eq!(status(&h.state, &worker(n)), waiting());
        assert_eq!(writes(&h.state, &worker(n)), 0);
    }
}

#[tokio::test]
async fn a_cleared_prompt_puts_back_exactly_the_status_it_replaced() {
    for before in [AgentStatus::Idle, AgentStatus::Starting] {
        let h = hive(&["claude"]);
        set_status(&h.state, &worker(1), before.clone());
        let cwd = h.worktree("worker-1");
        h.spawn(1, "claude", &cwd);
        output(&h.state, &worker(1), CLAUDE_PROMPT);
        assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "detected")]);
        output(&h.state, &worker(1), b"\x1b[2J\x1b[H> next\r\n");
        assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "cleared")]);
        assert_eq!(status(&h.state, &worker(1)), before, "a gone dialog is not proof of a start");
    }
}

#[tokio::test]
async fn a_newer_status_is_never_overwritten_when_the_prompt_goes() {
    let h = hive(&["antigravity"]);
    let cwd = h.worktree("worker-1");
    h.spawn(1, "antigravity", &cwd);
    output(&h.state, &worker(1), AGY_PROMPT);
    assert_eq!(sweep(&h.state, Instant::now())[0].1, "detected");
    set_status(&h.state, &worker(1), AgentStatus::Idle);
    output(&h.state, &worker(1), AGY_AFTER);
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "cleared")]);
    assert_eq!(status(&h.state, &worker(1)), AgentStatus::Idle);
}

#[tokio::test]
async fn a_respawn_drops_the_status_its_predecessors_prompt_left() {
    let h = hive(&["antigravity"]);
    let cwd = h.worktree("worker-1");
    h.spawn(1, "antigravity", &cwd);
    output(&h.state, &worker(1), AGY_PROMPT);
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "detected")]);
    assert_eq!(status(&h.state, &worker(1)), waiting());

    // Same id, new PTY that never shows a prompt: the old status must not stick.
    h.spawn(1, "antigravity", &cwd);
    assert!(sweep(&h.state, Instant::now()).is_empty());
    assert_eq!(status(&h.state, &worker(1)), AgentStatus::Running, "the predecessor's status is put back");

    // Its own prompt is surfaced on its own terms.
    output(&h.state, &worker(1), AGY_PROMPT);
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "detected")]);
    output(&h.state, &worker(1), AGY_AFTER);
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "cleared")]);
    assert_eq!(status(&h.state, &worker(1)), AgentStatus::Running);
}

#[tokio::test]
async fn an_exited_pty_is_left_to_exit_handling() {
    let h = hive(&["antigravity"]);
    let cwd = h.worktree("worker-1");
    h.state.pty_manager.read().create_session(worker(1), AgentRole::Worker { index: 1, parent: None }, "agy",
        &["--stub-clean-exit"], Some(&cwd), 120, 40).unwrap();
    output(&h.state, &worker(1), AGY_PROMPT);
    assert!(!h.state.pty_manager.read().is_alive(&worker(1)), "the stub exited");
    assert!(sweep(&h.state, Instant::now()).is_empty());
}

#[tokio::test]
async fn spawn_time_pre_trust_needs_the_sessions_own_project() {
    let h = hive(&["antigravity", "antigravity"]);
    let own = h.worktree("worker-1");
    let (_, other) = managed_worktree_fixture(h.temp.path(), "other-project", SESSION, "worker-2");
    let pty = h.state.pty_manager.read();
    for (id, cwd) in [(worker(1), own.clone()), (worker(2), other.to_string_lossy().into_owned())] {
        pty.create_session(id, AgentRole::Worker { index: 1, parent: None }, "agy", &[], Some(&cwd), 120, 40).unwrap();
    }
    #[cfg(windows)]
    {
        let overlay = |id: &str| pty.spawn_args_for_test(id).unwrap().iter().any(|arg| arg == "--settings");
        assert!(overlay(&worker(1)), "the seat's own worktree of the recorded project");
        assert!(!overlay(&worker(2)), "a matching worktree of another project gets nothing");
    }
}

#[cfg(windows)]
#[test]
fn agy_spawns_disable_the_auto_updater_and_pre_trust_only_a_recorded_worktree() {
    let temp = tempfile::TempDir::new().unwrap();
    let manager = crate::pty::PtyManager::new();
    let (repo, cwd) = managed_worktree_fixture(temp.path(), "project", SESSION, "worker-9");
    let recorded = repo.clone();
    manager.set_session_projects(Arc::new(move |id| (id == SESSION).then(|| recorded.clone())));
    let cwd = cwd.to_string_lossy().into_owned();
    // An id no other test spawns: the per-agent store is keyed by agent id.
    let id = worker(9);
    manager
        .create_session(id.clone(), AgentRole::Worker { index: 9, parent: None }, "agy",
            &["--model", "gemini-4-neon-high"], Some(&cwd), 80, 24)
        .unwrap();
    let updater_off = ("AGY_CLI_DISABLE_AUTO_UPDATE".to_string(), "true".to_string());
    let env = manager.spawn_env_for_test(&id).unwrap();
    assert!(env.contains(&updater_off), "{env:?}");
    let args = manager.spawn_args_for_test(&id).unwrap();
    let at = args.iter().position(|arg| arg == "--settings").expect("trust overlay flag");
    let overlay: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&args[at + 1]).unwrap()).unwrap();
    assert_eq!(overlay["trustedWorkspaces"], serde_json::json!([cwd]));
    assert_eq!(&args[args.len() - 2..], ["--model", "gemini-4-neon-high"], "CLI args stay last");
    assert_eq!(manager.pty_size(&id), Some((80, 24)));

    // A Queen on the operator's checkout keeps agy's own consent prompt.
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let queen = queen();
    manager.create_session(queen.clone(), AgentRole::Queen, "agy", &[], Some(&repo.to_string_lossy()), 80, 24).unwrap();
    assert!(!manager.spawn_args_for_test(&queen).unwrap().iter().any(|arg| arg == "--settings"));
    assert!(manager.spawn_env_for_test(&queen).unwrap().contains(&updater_off));

    let claude = worker(8);
    manager.create_session(claude.clone(), AgentRole::Worker { index: 8, parent: None }, "claude", &[], Some(&cwd), 80, 24).unwrap();
    assert!(!manager.spawn_env_for_test(&claude).unwrap().iter().any(|(key, _)| key == "AGY_CLI_DISABLE_AUTO_UPDATE"));
    assert!(manager.spawn_args_for_test(&claude).unwrap().is_empty(), "claude argv is untouched");
}

#[test]
fn agy_variable_never_becomes_a_heartbeat_identity() {
    // `create_session` derives the heartbeat credential from the identity env, so agy's
    // own variable must not be mistaken for a session id on unidentified seats.
    let manager = crate::pty::PtyManager::new();
    for id in ["scratch:terminal-1".to_string(), "plain-agy-seat".to_string()] {
        let role = if id.starts_with("scratch:") { AgentRole::ScratchShell } else { AgentRole::Worker { index: 1, parent: None } };
        manager.create_session(id.clone(), role, "agy", &[], None, 80, 24).unwrap();
        assert!(!manager.has_heartbeat_credential_for_test(&id), "{id}");
    }
    manager.create_session(worker(1), AgentRole::Worker { index: 1, parent: None }, "agy", &[], None, 80, 24).unwrap();
    assert!(manager.has_heartbeat_credential_for_test(&worker(1)), "an identified seat still gets one");
}

#[tokio::test]
async fn a_seat_that_exits_or_disappears_at_its_prompt_is_not_left_waiting() {
    let h = hive(&["antigravity", "antigravity"]);
    for n in [1, 2] {
        let cwd = h.worktree(&format!("worker-{n}"));
        h.spawn(n, "antigravity", &cwd);
        output(&h.state, &worker(n), AGY_PROMPT);
    }
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "detected"), (worker(2), "detected")]);
    // worker-1's process exits (the operator chose "No, exit"); worker-2's PTY is removed.
    let cwd = h.worktree("worker-1");
    {
        let pty = h.state.pty_manager.read();
        let _ = pty.kill(&worker(1));
        pty.create_session(worker(1), AgentRole::Worker { index: 1, parent: None }, "agy",
            &["--stub-clean-exit"], Some(&cwd), 120, 40).unwrap();
        let _ = pty.kill(&worker(2));
    }
    let _ = sweep(&h.state, Instant::now());
    let _ = sweep(&h.state, Instant::now());
    // worker-1 has a new PTY instance: its predecessor's exit never marks it failed.
    assert_eq!(status(&h.state, &worker(1)), AgentStatus::Running, "the replacement keeps the status the prompt replaced");
    assert_eq!(status(&h.state, &worker(2)), AgentStatus::Error("exited_at_trust_prompt".into()));
    // Both prompts are reported resolved, each under the instance that showed it.
    wait_for(|| trust_events(&h.state).iter().filter(|event| event["event_type"] == "agent_input_resolved").count() == 2).await;
    let resolved: Vec<_> = trust_events(&h.state).into_iter().filter(|event| event["event_type"] == "agent_input_resolved").collect();
    assert_eq!(resolved.len(), 2);
    let detected: Vec<_> = trust_events(&h.state).into_iter().filter(|event| event["event_type"] == "agent_waiting_input").collect();
    for done in &resolved {
        assert!(detected.iter().any(|up| up["agent_id"] == done["agent_id"] && up["payload"]["instance_id"] == done["payload"]["instance_id"]),
            "a resolution names the instance whose prompt it ends: {done}");
    }
}

#[tokio::test]
async fn a_trust_prompt_status_persisted_by_an_earlier_run_is_reset() {
    let h = hive(&["antigravity"]);
    set_status(&h.state, &worker(1), waiting());
    let cwd = h.worktree("worker-1");
    h.spawn(1, "antigravity", &cwd);
    assert!(sweep(&h.state, Instant::now()).is_empty());
    assert_eq!(status(&h.state, &worker(1)), AgentStatus::Running, "no record behind it: stale from a previous run");
    // If the prompt is in fact still up, the fresh guard surfaces it again.
    output(&h.state, &worker(1), AGY_PROMPT);
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "detected")]);
}
