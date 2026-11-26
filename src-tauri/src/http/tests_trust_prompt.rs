//! #466: the trust-prompt sweep against captured CLI screens, end to end through a stub
//! PTY, the session roster, the event log and the Queen's inbox. The fixtures are real
//! captures from scratch worktrees, with the account email and user name scrubbed.
use super::tests::make_test_session_with_agents;
use super::tests_agent_events::setup;
use crate::http::state::AppState;
use crate::pty::{AgentRole, AgentStatus};
use std::sync::Arc;
use std::time::{Duration, Instant};

const AGY_PROMPT: &[u8] = include_bytes!("../pty/trust_prompt_fixtures/agy-1.5.18-prompt.bin");
const AGY_AFTER: &[u8] = include_bytes!("../pty/trust_prompt_fixtures/agy-1.5.18-after-accept.bin");
const CODEX_PROMPT: &[u8] = include_bytes!("../pty/trust_prompt_fixtures/codex-0.171.0-prompt.bin");
const CLAUDE_PROMPT: &[u8] = include_bytes!("../pty/trust_prompt_fixtures/claude-2.1.332-prompt.bin");
const AGY_CWD: &str = r"D:\hm466-scratch\repo-agy-acc2";
const CLAUDE_CWD: &str = r"D:\hm466-scratch\repo-claude-wt1";
const SESSION: &str = "00000000-0000-4000-8000-000000000466";

fn queen() -> String {
    format!("{SESSION}-queen")
}

fn worker(n: u8) -> String {
    format!("{SESSION}-worker-{n}")
}

fn fixture(workers: &[(&str, &str)]) -> (tempfile::TempDir, Arc<AppState>) {
    let (temp, state, _app) = setup();
    state.session_controller.write().set_event_bus(state.event_bus.clone());
    let mut ids = vec![queen()];
    ids.extend((1..=workers.len() as u8).map(worker));
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut session = make_test_session_with_agents(SESSION, "fixture", &id_refs);
    session.agents[0].role = AgentRole::Queen;
    for (agent, (cli, _)) in session.agents[1..].iter_mut().zip(workers) {
        agent.config.cli = cli.to_string();
    }
    state.session_controller.read().insert_test_session(session);
    std::fs::create_dir_all(state.storage.session_dir(SESSION)).unwrap();
    for (n, (cli, cwd)) in workers.iter().enumerate() {
        let command = if *cli == "antigravity" { "agy" } else { *cli };
        state.pty_manager.read().create_session(
            worker(n as u8 + 1), AgentRole::Worker { index: n as u8 + 1, parent: None },
            command, &[], Some(cwd), 120, 40,
        ).unwrap();
    }
    (temp, state)
}

fn status(state: &AppState, id: &str) -> AgentStatus {
    let controller = state.session_controller.read();
    let session = controller.get_session(SESSION).unwrap();
    session.agents.iter().find(|agent| agent.id == id).unwrap().status.clone()
}

fn sweep(state: &AppState, at: Instant) -> Vec<(String, &'static str, Option<&'static str>)> {
    state.session_controller.read().sweep_trust_prompts_at(SESSION, at)
        .into_iter().map(|outcome| (outcome.agent_id, outcome.action, outcome.reason)).collect()
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

#[tokio::test]
async fn agy_prompt_is_accepted_once_with_status_and_events_then_cleared() {
    let (_temp, state) = fixture(&[("antigravity", AGY_CWD)]);
    let t0 = Instant::now();
    assert!(sweep(&state, t0).is_empty(), "nothing on screen yet");

    output(&state, &worker(1), AGY_PROMPT);
    assert_eq!(sweep(&state, t0), [(worker(1), "accepted", None)]);
    assert_eq!(status(&state, &worker(1)), AgentStatus::WaitingForInput("trust_prompt".into()));
    assert_eq!(enters(&state, &worker(1)), 1);
    assert!(sweep(&state, t0 + Duration::from_secs(5)).is_empty(), "no output yet: still waiting, no second Enter");
    assert_eq!(enters(&state, &worker(1)), 1);

    output(&state, &worker(1), AGY_AFTER);
    assert_eq!(sweep(&state, t0 + Duration::from_secs(6)), [(worker(1), "cleared", None)]);
    assert_eq!(status(&state, &worker(1)), AgentStatus::Running);

    // The same prompt drawn again later is never answered a second time.
    output(&state, &worker(1), b"\x1b[2J\x1b[H");
    output(&state, &worker(1), AGY_PROMPT);
    assert!(sweep(&state, t0 + Duration::from_secs(20)).is_empty());
    assert_eq!(enters(&state, &worker(1)), 1, "at most one accept per spawn");

    wait_for(|| trust_events(&state).len() == 2).await;
    let events = trust_events(&state);
    let actions: Vec<_> = events.iter().map(|event| event["payload"]["action"].as_str().unwrap().to_string()).collect();
    assert_eq!(actions, ["accepted", "cleared"]);
    assert_eq!(events[0]["agent_id"], worker(1));
    assert_eq!(events[0]["event_type"], "agent_waiting_input");
    assert_eq!(events[0]["payload"]["adapter"], "antigravity");
    assert_eq!(events[0]["payload"]["path"], AGY_CWD);
    assert!(inbox(&state, &queen()).is_empty(), "a handled prompt does not bother the Queen");
}

#[tokio::test]
async fn a_prompt_for_a_foreign_path_is_surfaced_to_the_queen_and_never_answered() {
    let (_temp, state) = fixture(&[("antigravity", r"D:\hm466-scratch\another-worktree")]);
    output(&state, &worker(1), AGY_PROMPT);
    assert_eq!(sweep(&state, Instant::now()), [(worker(1), "escalated", Some("foreign_path"))]);
    assert_eq!(enters(&state, &worker(1)), 0);
    assert_eq!(status(&state, &worker(1)), AgentStatus::WaitingForInput("trust_prompt".into()));
    wait_for(|| !inbox(&state, &queen()).is_empty()).await;
    let notice = inbox(&state, &queen());
    assert!(notice.contains("folder-trust prompt for D:\\hm466-scratch\\repo-agy-acc2"), "{notice}");
    assert!(notice.contains("foreign_path"), "{notice}");
    assert!(sweep(&state, Instant::now() + Duration::from_secs(30)).is_empty(), "surfaced once, not looped");
    wait_for(|| trust_events(&state).len() == 1).await;
    assert_eq!(trust_events(&state)[0]["severity"], "warning");
}

#[tokio::test]
async fn claude_prompt_is_surfaced_because_its_default_is_no() {
    let (_temp, state) = fixture(&[("claude", CLAUDE_CWD)]);
    output(&state, &worker(1), CLAUDE_PROMPT);
    assert_eq!(sweep(&state, Instant::now()), [(worker(1), "escalated", Some("default_is_not_trust"))]);
    assert_eq!(enters(&state, &worker(1)), 0);
}

#[tokio::test]
async fn an_accept_that_leaves_the_screen_unchanged_is_escalated_once() {
    let (_temp, state) = fixture(&[("antigravity", AGY_CWD)]);
    let t0 = Instant::now();
    output(&state, &worker(1), AGY_PROMPT);
    assert_eq!(sweep(&state, t0), [(worker(1), "accepted", None)]);
    assert_eq!(sweep(&state, t0 + Duration::from_secs(10)), [(worker(1), "escalated", Some("screen_unchanged"))]);
    assert_eq!(status(&state, &worker(1)), AgentStatus::WaitingForInput("trust_prompt".into()));
    assert!(sweep(&state, t0 + Duration::from_secs(60)).is_empty());
    assert_eq!(enters(&state, &worker(1)), 1);
    wait_for(|| inbox(&state, &queen()).contains("screen_unchanged")).await;
    assert!(inbox(&state, &queen()).contains("screen_unchanged"));
}

#[tokio::test]
async fn codex_and_agy_workers_are_watched_independently() {
    // The codex prompt says its trust would apply to a repository that is not the
    // worktree's own, so it is surfaced rather than answered.
    let (_temp, state) = fixture(&[("antigravity", AGY_CWD), ("codex", r"D:\hm466-scratch\repo-codex2-acc")]);
    let codex = String::from_utf8(CODEX_PROMPT.to_vec()).unwrap()
        .replace(r"d:\hm466-scratch\repo-codex2", r"d:\hm466-fixture\not-this-repo");
    output(&state, &worker(1), AGY_PROMPT);
    output(&state, &worker(2), codex.as_bytes());
    let mut outcomes = sweep(&state, Instant::now());
    outcomes.sort();
    assert_eq!(outcomes, [(worker(1), "accepted", None), (worker(2), "escalated", Some("foreign_repo_root"))]);
    assert_eq!(enters(&state, &worker(2)), 0);
}

#[cfg(windows)]
#[test]
fn agy_spawns_disable_the_auto_updater_and_pre_trust_their_worktree() {
    let manager = crate::pty::PtyManager::new();
    let temp = tempfile::TempDir::new().unwrap();
    let cwd = temp.path().join("worker-1");
    let cwd = cwd.to_string_lossy().into_owned();
    let id = format!("{SESSION}-worker-1");
    manager
        .create_session(id.clone(), AgentRole::Worker { index: 1, parent: None }, "agy",
            &["--model", "gemini-4-neon-high"], Some(&cwd), 80, 24)
        .unwrap();
    let env = manager.spawn_env_for_test(&id).unwrap();
    assert!(env.contains(&("AGY_CLI_DISABLE_AUTO_UPDATE".to_string(), "true".to_string())), "{env:?}");
    let args = manager.spawn_args_for_test(&id).unwrap();
    let at = args.iter().position(|arg| arg == "--settings").expect("trust overlay flag");
    let overlay: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&args[at + 1]).unwrap()).unwrap();
    assert_eq!(overlay["trustedWorkspaces"], serde_json::json!([cwd]));
    assert_eq!(&args[args.len() - 2..], ["--model", "gemini-4-neon-high"], "CLI args stay last");
    assert_eq!(manager.pty_size(&id), Some((80, 24)));

    let claude = format!("{SESSION}-worker-2");
    manager.create_session(claude.clone(), AgentRole::Worker { index: 2, parent: None }, "claude", &[], Some(&cwd), 80, 24).unwrap();
    assert!(!manager.spawn_env_for_test(&claude).unwrap().iter().any(|(k, _)| k == "AGY_CLI_DISABLE_AUTO_UPDATE"));
    assert!(manager.spawn_args_for_test(&claude).unwrap().is_empty(), "claude argv is untouched");
}

