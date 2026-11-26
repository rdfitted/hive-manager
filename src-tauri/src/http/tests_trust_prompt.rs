//! #466: the trust-prompt sweep against captured CLI screens, end to end through a stub
//! PTY, the session roster, the event log and the Queen's inbox. The fixtures are real
//! captures from scratch worktrees, with the account email and user name scrubbed;
//! `retarget` points them at a managed worktree laid out for the test.
use super::tests::make_test_session_with_agents;
use super::tests_agent_events::setup;
use crate::cli::folder_trust::managed_worktree_fixture;
use crate::http::state::AppState;
use crate::pty::trust_prompt::tests::{
    reframe, retarget, AGY_AFTER, AGY_CWD, AGY_PROMPT, CLAUDE_CWD, CLAUDE_PROMPT, CODEX_CWD, CODEX_PROMPT, CODEX_ROOT,
};
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
    repo: String,
}

impl Hive {
    /// A worktree Hive Manager would have made for `name`.
    fn worktree(&self, name: &str) -> String {
        let (_, cwd) = managed_worktree_fixture(self.temp.path(), "project", SESSION, name);
        cwd.to_string_lossy().into_owned()
    }

    /// Spawn `n` on a stub PTY in `cwd` as `cli`.
    fn spawn(&self, n: u8, cli: &str, cwd: &str) {
        let command = if cli == "antigravity" { "agy" } else { cli };
        let pty = self.state.pty_manager.read();
        let _ = pty.kill(&worker(n));
        // Wide enough for a hive worktree path on one row: the fixtures were laid out
        // for their own shorter paths at 120 columns.
        pty.create_session(worker(n), AgentRole::Worker { index: n, parent: None }, command, &[], Some(cwd), 200, 50)
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
    let repo = temp.path().join("project").to_string_lossy().into_owned();
    Hive { temp, state, repo }
}

fn status(state: &AppState, id: &str) -> AgentStatus {
    let controller = state.session_controller.read();
    let session = controller.get_session(SESSION).unwrap();
    session.agents.iter().find(|agent| agent.id == id).unwrap().status.clone()
}

fn sweep(state: &AppState, at: Instant) -> Vec<(String, &'static str, Option<&'static str>)> {
    let mut outcomes: Vec<_> = state.session_controller.read().sweep_trust_prompts_at(SESSION, at)
        .into_iter().map(|outcome| (outcome.agent_id, outcome.action, outcome.reason)).collect();
    outcomes.sort();
    outcomes
}

fn output(state: &AppState, id: &str, bytes: &[u8]) {
    state.pty_manager.read().record_output_for_test(id, bytes);
}

fn enters(state: &AppState, id: &str) -> usize {
    state.pty_manager.read().write_records_for_test(id).unwrap().iter().filter(|write| write.as_slice() == b"\r").count()
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
async fn agy_prompt_is_accepted_once_with_status_and_events_then_cleared() {
    let h = hive(&["antigravity"]);
    let cwd = h.worktree("worker-1");
    h.spawn(1, "antigravity", &cwd);
    let t0 = Instant::now();
    assert!(sweep(&h.state, t0).is_empty(), "nothing on screen yet");

    output(&h.state, &worker(1), &retarget(AGY_PROMPT, &[(AGY_CWD, &cwd)]));
    assert_eq!(sweep(&h.state, t0), [(worker(1), "accepted", None)]);
    assert_eq!(status(&h.state, &worker(1)), waiting());
    assert_eq!(enters(&h.state, &worker(1)), 1);
    assert!(sweep(&h.state, t0 + Duration::from_secs(5)).is_empty(), "still waiting, no second Enter");
    assert_eq!(enters(&h.state, &worker(1)), 1);

    output(&h.state, &worker(1), AGY_AFTER);
    assert_eq!(sweep(&h.state, t0 + Duration::from_secs(6)), [(worker(1), "cleared", None)]);
    assert_eq!(status(&h.state, &worker(1)), AgentStatus::Running);

    // The same prompt drawn again later is never answered a second time.
    output(&h.state, &worker(1), b"\x1b[2J\x1b[H");
    output(&h.state, &worker(1), &retarget(AGY_PROMPT, &[(AGY_CWD, &cwd)]));
    assert!(sweep(&h.state, t0 + Duration::from_secs(20)).is_empty());
    assert_eq!(enters(&h.state, &worker(1)), 1, "at most one accept per spawn");

    wait_for(|| trust_events(&h.state).len() == 2).await;
    let events = trust_events(&h.state);
    let actions: Vec<_> = events.iter().map(|event| event["payload"]["action"].as_str().unwrap().to_string()).collect();
    assert_eq!(actions, ["accepted", "cleared"]);
    assert_eq!(events[0]["agent_id"], worker(1));
    assert_eq!(events[0]["event_type"], "agent_waiting_input");
    assert_eq!(events[0]["payload"]["adapter"], "antigravity");
    assert_eq!(events[0]["payload"]["path"], cwd);
    assert!(inbox(&h.state, &queen()).is_empty(), "a handled prompt does not bother the Queen");
}

#[tokio::test]
async fn codex_prompt_for_its_own_worktree_and_root_is_accepted_and_agy_runs_beside_it() {
    let h = hive(&["antigravity", "codex"]);
    let (agy, codex) = (h.worktree("worker-1"), h.worktree("worker-2"));
    h.spawn(1, "antigravity", &agy);
    h.spawn(2, "codex", &codex);
    output(&h.state, &worker(1), &retarget(AGY_PROMPT, &[(AGY_CWD, &agy)]));
    output(&h.state, &worker(2), &reframe(CODEX_PROMPT, &[(CODEX_CWD, &codex), (CODEX_ROOT, &h.repo)]));
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "accepted", None), (worker(2), "accepted", None)]);
    assert_eq!((enters(&h.state, &worker(1)), enters(&h.state, &worker(2))), (1, 1));
}

#[tokio::test]
async fn a_prompt_outside_a_managed_worktree_or_for_another_path_is_surfaced_once() {
    let h = hive(&["antigravity", "codex"]);
    // worker-1 runs on a plain checkout (as a Queen would); worker-2 is shown a prompt
    // for some other folder.
    h.spawn(1, "antigravity", AGY_CWD);
    let codex = h.worktree("worker-2");
    h.spawn(2, "codex", &codex);
    output(&h.state, &worker(1), AGY_PROMPT);
    output(&h.state, &worker(2), CODEX_PROMPT);
    assert_eq!(sweep(&h.state, Instant::now()), [
        (worker(1), "escalated", Some("not_a_managed_worktree")),
        (worker(2), "escalated", Some("foreign_path")),
    ]);
    assert_eq!((enters(&h.state, &worker(1)), enters(&h.state, &worker(2))), (0, 0));
    assert_eq!(status(&h.state, &worker(2)), waiting());
    wait_for(|| inbox(&h.state, &queen()).matches("folder-trust prompt").count() == 2).await;
    let notice = inbox(&h.state, &queen());
    assert!(notice.contains("folder-trust prompt for D:\\hm466-scratch\\repo-codex2-acc"), "{notice}");
    assert!(notice.contains("foreign_path") && notice.contains("not_a_managed_worktree"), "{notice}");
    assert!(sweep(&h.state, Instant::now() + Duration::from_secs(30)).is_empty(), "surfaced once, not looped");
    wait_for(|| trust_events(&h.state).len() == 2).await;
    assert!(trust_events(&h.state).iter().all(|event| event["severity"] == "warning"));
}

#[tokio::test]
async fn an_operator_answered_prompt_clears_the_status() {
    let h = hive(&["claude"]);
    let cwd = h.worktree("worker-1");
    h.spawn(1, "claude", &cwd);
    output(&h.state, &worker(1), &retarget(CLAUDE_PROMPT, &[(CLAUDE_CWD, &cwd)]));
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "escalated", Some("operator_decides"))]);
    assert_eq!(enters(&h.state, &worker(1)), 0, "Claude's trust is always the operator's");
    assert_eq!(status(&h.state, &worker(1)), waiting());
    output(&h.state, &worker(1), b"\x1b[2J\x1b[H> working on it\r\n");
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "cleared", None)]);
    assert_eq!(status(&h.state, &worker(1)), AgentStatus::Running);
}

#[tokio::test]
async fn an_accept_that_leaves_the_screen_unchanged_is_escalated_once() {
    let h = hive(&["antigravity"]);
    let cwd = h.worktree("worker-1");
    h.spawn(1, "antigravity", &cwd);
    let t0 = Instant::now();
    output(&h.state, &worker(1), &retarget(AGY_PROMPT, &[(AGY_CWD, &cwd)]));
    assert_eq!(sweep(&h.state, t0), [(worker(1), "accepted", None)]);
    assert_eq!(sweep(&h.state, t0 + Duration::from_secs(10)), [(worker(1), "escalated", Some("screen_unchanged"))]);
    assert_eq!(status(&h.state, &worker(1)), waiting());
    assert!(sweep(&h.state, t0 + Duration::from_secs(60)).is_empty());
    assert_eq!(enters(&h.state, &worker(1)), 1);
    wait_for(|| inbox(&h.state, &queen()).contains("screen_unchanged")).await;
    assert!(inbox(&h.state, &queen()).contains("screen_unchanged"));
}

#[tokio::test]
async fn a_respawn_gets_a_fresh_guard_and_the_old_prompt_never_reaches_it() {
    let h = hive(&["antigravity"]);
    let cwd = h.worktree("worker-1");
    h.spawn(1, "antigravity", &cwd);
    let prompt = retarget(AGY_PROMPT, &[(AGY_CWD, &cwd)]);
    output(&h.state, &worker(1), &prompt);
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "accepted", None)]);
    h.spawn(1, "antigravity", &cwd);
    assert_eq!(enters(&h.state, &worker(1)), 0, "the replacement starts with no input");
    assert!(sweep(&h.state, Instant::now()).is_empty(), "its own screen is still empty");
    output(&h.state, &worker(1), &prompt);
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "accepted", None)], "one accept per spawn");
    assert_eq!(enters(&h.state, &worker(1)), 1);
}

#[cfg(windows)]
#[test]
fn agy_spawns_disable_the_auto_updater_and_pre_trust_only_a_managed_worktree() {
    let temp = tempfile::TempDir::new().unwrap();
    let manager = crate::pty::PtyManager::new();
    let (repo, cwd) = managed_worktree_fixture(temp.path(), "project", SESSION, "worker-9");
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

#[tokio::test]
async fn a_cleared_prompt_puts_back_the_status_it_replaced() {
    let h = hive(&["claude"]);
    {
        let controller = h.state.session_controller.read();
        let mut session = controller.get_session(SESSION).unwrap();
        session.agents[1].status = AgentStatus::Idle;
        controller.insert_test_session(session);
    }
    let cwd = h.worktree("worker-1");
    h.spawn(1, "claude", &cwd);
    output(&h.state, &worker(1), &retarget(CLAUDE_PROMPT, &[(CLAUDE_CWD, &cwd)]));
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "escalated", Some("operator_decides"))]);
    assert_eq!(status(&h.state, &worker(1)), waiting());
    output(&h.state, &worker(1), b"\x1b[2J\x1b[H> idle again\r\n");
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "cleared", None)]);
    assert_eq!(status(&h.state, &worker(1)), AgentStatus::Idle);
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
async fn a_managed_worktree_of_another_project_is_not_this_seats() {
    let h = hive(&["antigravity"]);
    let (_, foreign) = managed_worktree_fixture(h.temp.path(), "other-project", SESSION, "worker-1");
    let cwd = foreign.to_string_lossy().into_owned();
    h.spawn(1, "antigravity", &cwd);
    output(&h.state, &worker(1), &retarget(AGY_PROMPT, &[(AGY_CWD, &cwd)]));
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "escalated", Some("not_a_managed_worktree"))]);
    assert_eq!(enters(&h.state, &worker(1)), 0);
}

#[test]
fn the_accept_is_refused_once_other_input_has_reached_the_terminal() {
    let manager = crate::pty::PtyManager::new();
    manager.create_session(worker(7), AgentRole::Worker { index: 7, parent: None }, "agy", &[], None, 80, 24).unwrap();
    let (instance, revision) = manager.input_fence(&worker(7)).unwrap();
    manager.write(&worker(7), b"x").unwrap();
    assert!(manager.write_instance_at_revision(&worker(7), instance, revision, b"\r").is_err(), "the operator typed first");
    let (instance, revision) = manager.input_fence(&worker(7)).unwrap();
    manager.write_instance_at_revision(&worker(7), instance, revision, b"\r").unwrap();
    assert_eq!(manager.write_records_for_test(&worker(7)).unwrap().last().map(Vec::as_slice), Some(b"\r".as_slice()));
}

#[tokio::test]
async fn an_operator_trust_override_keeps_the_prompt_theirs() {
    let h = hive(&["antigravity"]);
    let cwd = h.worktree("worker-1");
    {
        let pty = h.state.pty_manager.read();
        pty.create_session(worker(1), AgentRole::Worker { index: 1, parent: None }, "agy",
            &["--settings", "operator.json"], Some(&cwd), 200, 50).unwrap();
    }
    output(&h.state, &worker(1), &retarget(AGY_PROMPT, &[(AGY_CWD, &cwd)]));
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "escalated", Some("operator_trust_override"))]);
    assert_eq!(enters(&h.state, &worker(1)), 0);
}

#[tokio::test]
async fn a_seat_still_starting_stays_starting_when_its_prompt_goes() {
    let h = hive(&["claude"]);
    {
        let controller = h.state.session_controller.read();
        let mut session = controller.get_session(SESSION).unwrap();
        session.agents[1].status = AgentStatus::Starting;
        controller.insert_test_session(session);
    }
    let cwd = h.worktree("worker-1");
    h.spawn(1, "claude", &cwd);
    output(&h.state, &worker(1), &retarget(CLAUDE_PROMPT, &[(CLAUDE_CWD, &cwd)]));
    assert_eq!(sweep(&h.state, Instant::now())[0].1, "escalated");
    output(&h.state, &worker(1), b"\x1b[2J\x1b[H");
    assert_eq!(sweep(&h.state, Instant::now()), [(worker(1), "cleared", None)]);
    assert_eq!(status(&h.state, &worker(1)), AgentStatus::Starting, "a gone dialog is not proof of a start");
}
