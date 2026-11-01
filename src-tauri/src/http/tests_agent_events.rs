//! All transcript fixtures here are synthetic, under TempDir. Never read live homes.
use super::{tests::make_test_session_with_agents, tests_transport::FakeTransport};
use crate::{
    coordination::{InjectionManager, QueueManager},
    events::EventBus,
    http::{routes::create_router, state::AppState},
    pty::PtyManager,
    session::SessionController,
    storage::{ApplicationStateDb, QueueRepo, SessionStorage},
    transport::{
        claude_transcript_source::ClaudeTranscriptSource,
        feed::{Cursor, MAX_READ_BYTES},
        AgentEvent, AgentEventKind as Kind, TransportRegistry, MAX_PAYLOAD_BYTES, MAX_TEXT_BYTES,
    },
};
use axum::{
    body::{Body, Bytes},
    http::{Request, StatusCode},
};
use futures::{Stream, StreamExt};
use parking_lot::RwLock;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tower::ServiceExt;

pub(super) fn setup() -> (tempfile::TempDir, Arc<AppState>, axum::Router) {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(SessionStorage::new_with_base(temp.path().join("storage")).unwrap());
    let pty = Arc::new(RwLock::new(PtyManager::new()));
    let controller = Arc::new(RwLock::new(SessionController::new(pty.clone())));
    controller.write().set_storage(storage.clone());
    let injection = Arc::new(RwLock::new(InjectionManager::new(
        pty.clone(),
        SessionStorage::new_with_base(temp.path().join("storage")).unwrap(),
    )));
    let bus = EventBus::new(storage.base_dir().clone());
    let db = Arc::new(ApplicationStateDb::open_in_memory().unwrap());
    let queue_repo = Arc::new(QueueRepo::new(db.clone()));
    queue_repo.ensure_schema().unwrap();
    let queue = Arc::new(QueueManager::new(queue_repo, bus.clone()));
    let state = Arc::new(AppState::new(
        Arc::new(tokio::sync::RwLock::new(storage.load_config().unwrap())),
        pty,
        controller,
        injection,
        storage.clone(),
        bus,
        db,
        queue,
        None,
    ));
    state
        .transports
        .set(Arc::new(TransportRegistry::new(
            storage.sessions_dir(),
            Some(temp.path().join("claude-projects")),
        )))
        .ok()
        .unwrap();
    let app = create_router(state.clone());
    (temp, state, app)
}

pub(super) fn event(agent: &str, text: &str) -> AgentEvent {
    AgentEvent {
        agent_id: agent.into(),
        cursor: String::new(),
        ts: "2026-01-01T00:00:00Z".into(),
        kind: Kind::AssistantMessage,
        source_id: Some(text.into()),
        truncated: false,
        payload: json!({"text":text}),
    }
}
fn transcript_path(root: &Path, cwd: &Path, id: &str) -> PathBuf {
    // Fixture identity is independent of the reader under mutation.
    let slug: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    root.join(slug).join(format!("{id}.jsonl"))
}
fn write_records(path: &Path, records: &[Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = records.iter().map(|v| format!("{v}\n")).collect();
    std::fs::write(path, text).unwrap();
}
async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}
async fn get(app: &axum::Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn g8_identical_cwd_and_start_time_use_only_held_transcript_ids() {
    // Mutation witness: cwd-directory discovery fails the own-transcript assertion.
    // Keep fixture path construction independent of the production locator.
    let (temp, state, app) = setup();
    let mut session =
        make_test_session_with_agents("session", "D:/fixture/shared", &["worker-a", "worker-b"]);
    session.no_git = true;
    for (index, agent) in session.agents.iter_mut().enumerate() {
        agent.transcript_spawned_at = Some(session.created_at);
        agent.transcript_session_id = Some(format!("transcript-{index}"));
        let path = transcript_path(
            &temp.path().join("claude-projects"),
            &session.project_path,
            agent.transcript_session_id.as_deref().unwrap(),
        );
        write_records(
            &path,
            &[
                json!({"type":"user","uuid":format!("user-{index}"),"message":{"role":"user","content":format!("message-for-{index}")}}),
                json!({"type":"assistant","uuid":format!("record-{index}"),"message":{"id":format!("message-{index}"),"role":"assistant","content":[{"type":"text","text":format!("answer-for-{index}")}],"stop_reason":"end_turn"}}),
            ],
        );
    }
    state.session_controller.read().insert_test_session(session);
    for (index, aid) in ["worker-a", "worker-b"].iter().enumerate() {
        let page =
            body_json(get(&app, &format!("/api/sessions/session/agents/{aid}/events")).await).await;
        let text = page.to_string();
        assert!(
            text.contains(&format!("answer-for-{index}")),
            "G8 each feed contains its own transcript"
        );
        assert!(
            !text.contains(&format!("answer-for-{}", 1 - index)),
            "G8 no cross-agent records for identical cwd/time"
        );
        assert!(page["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["agent_id"] == *aid));
    }
}

#[tokio::test]
async fn transcript_allowlist_merge_tools_and_missing_file_are_safe() {
    let (temp, state, app) = setup();
    let mut session = make_test_session_with_agents("session", "D:/fixture/shared", &["worker-a"]);
    session.no_git = true;
    session.agents[0].transcript_session_id = Some("main-transcript".into());
    let path = transcript_path(
        &temp.path().join("claude-projects"),
        &session.project_path,
        "main-transcript",
    );
    write_records(
        &path,
        &[
            json!({"type":"api-request-blob","message":{"role":"assistant","content":"PRIVATE_BLOB"}}),
            json!({"type":"unknown","message":{"role":"user","content":"UNKNOWN_TEXT"}}),
            json!({"type":"assistant","uuid":"record-1","message":{"role":"assistant","id":"message-1","content":[{"type":"thinking","thinking":"PRIVATE_THINKING"},{"type":"text","text":"first "},{"type":"tool_use","id":"tool-1","name":"fixture_tool","input":{"value":1}}]}}),
            json!({"type":"assistant","uuid":"record-2","message":{"role":"assistant","id":"message-1","content":[{"type":"text","text":"second"}],"stop_reason":"end_turn"}}),
            json!({"type":"user","uuid":"record-3","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"synthetic tool result"}]}}),
            json!({"type":"system","subtype":"turn_duration"}),
        ],
    );
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"malformed json\n")
        .unwrap();
    write_records(
        &path
            .parent()
            .unwrap()
            .join("main-transcript/subagents/subagent.jsonl"),
        &[json!({"type":"user","message":{"role":"user","content":"SUBAGENT_PRIVATE"}})],
    );
    state.session_controller.read().insert_test_session(session);
    let page = body_json(get(&app, "/api/sessions/session/agents/worker-a/events").await).await;
    let events = page["events"].as_array().unwrap();
    assert!(events.iter().any(|e| e["kind"] == "tool_start"));
    assert!(events.iter().any(|e| e["kind"] == "tool_result"));
    assert!(
        !events.iter().any(|e| e["kind"] == "user_message"),
        "tool result is never a user message"
    );
    let messages: Vec<_> = events
        .iter()
        .filter(|e| e["kind"] == "assistant_message")
        .collect();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["source_id"], messages[1]["source_id"]);
    assert_eq!(messages[1]["payload"]["text"], "first second");
    for excluded in [
        "PRIVATE_BLOB",
        "UNKNOWN_TEXT",
        "PRIVATE_THINKING",
        "SUBAGENT_PRIVATE",
    ] {
        assert!(!page.to_string().contains(excluded));
    }
    std::fs::remove_file(path).unwrap();
    assert_eq!(
        get(&app, "/api/sessions/session/agents/worker-a/events")
            .await
            .status(),
        StatusCode::OK
    );
}

#[test]
fn source_read_and_partial_line_are_bounded_to_two_mib() {
    let temp = tempfile::tempdir().unwrap();
    let mut source = ClaudeTranscriptSource::new(
        temp.path().into(),
        Path::new("fixture"),
        "transcript",
        "worker-a",
    )
    .unwrap();
    let path = source.path().to_owned();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut bytes = vec![b'x'; MAX_READ_BYTES as usize + 10];
    bytes.push(b'\n');
    bytes.extend_from_slice(
        b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"after oversized\"}}\n",
    );
    std::fs::write(path, bytes).unwrap();
    let (events, more, _) = source.poll().unwrap();
    assert!(events.is_empty() && more);
    assert_eq!(source.offset(), MAX_READ_BYTES);
    let (events, more, _) = source.poll().unwrap();
    assert!(!more);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload["text"], "after oversized");
}

#[tokio::test]
async fn no_source_capabilities_history_and_invalid_identity_paths() {
    let (_temp, state, app) = setup();
    let mut session =
        make_test_session_with_agents("session", "fixture", &["worker-a", "worker-b"]);
    session.agents[0].config.cli = "cursor".into();
    session.agents[1].config.cli = "codex".into();
    state.session_controller.read().insert_test_session(session);
    let feeds = body_json(get(&app, "/api/sessions/session/agent-feeds").await).await;
    for agent in feeds["agents"].as_array().unwrap() {
        assert_eq!(agent["capabilities"]["event_source"], "none");
    }
    let page = body_json(get(&app, "/api/sessions/session/agents/worker-a/events").await).await;
    assert_eq!(page["events"], json!([]));
    for uri in [
        "/api/sessions/C:escape/agent-feeds",
        "/api/sessions/..evil/agent-feeds",
        "/api/sessions/session/agents/..evil/events",
        "/api/sessions/session/agents/worker-a/events?before=invalid",
    ] {
        assert_eq!(
            get(&app, uri).await.status(),
            StatusCode::BAD_REQUEST,
            "{uri}"
        );
    }
    assert_eq!(
        get(&app, "/api/sessions/session/agents/worker-other/events")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn protocol_overrides_transcript_and_caps_every_payload() {
    let (temp, state, app) = setup();
    let mut session = make_test_session_with_agents("session", "fixture", &["worker-a"]);
    session.no_git = true;
    session.agents[0].transcript_session_id = Some("transcript".into());
    let path = transcript_path(
        &temp.path().join("claude-projects"),
        &session.project_path,
        "transcript",
    );
    write_records(
        &path,
        &[json!({"type":"user","message":{"role":"user","content":"TRANSCRIPT_MUST_NOT_APPEAR"}})],
    );
    state.session_controller.read().insert_test_session(session);
    // Exercise the harder ordering: a PTY transcript was already imported.
    let transcript =
        body_json(get(&app, "/api/sessions/session/agents/worker-a/events").await).await;
    assert!(transcript
        .to_string()
        .contains("TRANSCRIPT_MUST_NOT_APPEAR"));
    let registry = state.transports();
    registry
        .register(
            "session",
            "worker-a",
            Some("thread-a".into()),
            FakeTransport::new(),
        )
        .unwrap();
    let huge = "🦀".repeat(MAX_TEXT_BYTES);
    let mut item = event("worker-a", "bounded");
    item.payload = json!({"text": huge, "input": vec!["x".repeat(MAX_TEXT_BYTES); 10]});
    let stored = registry.append_event("session", "worker-a", item).unwrap();
    assert!(stored.truncated);
    assert!(stored.payload.to_string().len() <= MAX_PAYLOAD_BYTES);
    assert!(registry
        .append_event("session", "worker-a", event("worker-b", "wrong"))
        .is_err());
    let feeds = body_json(get(&app, "/api/sessions/session/agent-feeds").await).await;
    assert_eq!(
        feeds["agents"][0]["capabilities"]["event_source"],
        "protocol"
    );
    assert_eq!(feeds["agents"][0]["capabilities"]["has_terminal"], false);
    let page = body_json(get(&app, "/api/sessions/session/agents/worker-a/events").await).await;
    assert_eq!(page["events"].as_array().unwrap().len(), 1);
    assert!(!page.to_string().contains("TRANSCRIPT_MUST_NOT_APPEAR"));
    assert_eq!(page["events"][0]["truncated"], true);
}

#[test]
fn text_cap_preserves_utf8_without_triggering_aggregate_fallback() {
    let text = "🦀".repeat(5000); // 20,000 bytes, below aggregate cap.
    let item = event("worker-a", &text).normalize();
    assert!(item.truncated);
    assert_eq!(item.payload["text"].as_str().unwrap().len(), MAX_TEXT_BYTES);
    assert!(item.payload["text"]
        .as_str()
        .unwrap()
        .chars()
        .all(|c| c == '🦀'));
    assert!(item.payload.to_string().len() < MAX_PAYLOAD_BYTES);
}

#[test]
fn distinct_repeated_fragments_cumulative_snapshots_and_record_replay_merge() {
    let temp = tempfile::tempdir().unwrap();
    let mut source = ClaudeTranscriptSource::new(
        temp.path().into(),
        Path::new("fixture"),
        "transcript",
        "worker-a",
    )
    .unwrap();
    let record = |uuid: &str, id: &str, text: &str| {
        json!({"type":"assistant", "uuid":uuid,
        "message":{"role":"assistant","id":id,"content":[{"type":"text","text":text}]}})
    };
    let path = source.path().to_owned();
    write_records(
        &path,
        &[
            record("record-1", "message-a", "ha"),
            record("record-2", "message-a", "ha"),
            record("record-3", "message-b", "first "),
            record("record-4", "message-b", "first second"),
            record("record-4", "message-b", "first second"),
        ],
    );
    let (events, _, _) = source.poll().unwrap();
    let messages: Vec<_> = events
        .iter()
        .filter(|e| e.kind == Kind::AssistantMessage)
        .collect();
    assert_eq!(
        messages.len(),
        4,
        "replayed record UUID/content is not another fragment"
    );
    assert_eq!(messages[1].payload["text"], "haha");
    assert_eq!(messages[3].payload["text"], "first second");
}

#[test]
fn replaced_transcript_and_changed_held_identity_rotate_generation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("claude-projects");
    let registry = TransportRegistry::new(temp.path().join("sessions"), Some(root.clone()));
    let cwd = Path::new("fixture");
    let path = transcript_path(&root, cwd, "transcript-a");
    let user =
        |text: &str| json!({"type":"user", "uuid":text,"message":{"role":"user","content":text}});
    write_records(&path, &[user("old")]);
    registry
        .feeds
        .use_transcript("session", "worker-a", cwd, "transcript-a")
        .unwrap();
    registry
        .feeds
        .refresh_transcript("session", "worker-a")
        .unwrap();
    let old = registry
        .feeds
        .history("session", "worker-a", None, None, 100)
        .unwrap()
        .latest_cursor
        .unwrap();
    // Same path and a larger replacement must not leave its prefix skipped.
    write_records(&path, &[user("new replacement longer than old")]);
    registry
        .feeds
        .refresh_transcript("session", "worker-a")
        .unwrap();
    let page = registry
        .feeds
        .history("session", "worker-a", None, Some(&old), 100)
        .unwrap();
    assert!(page.reset);
    assert_eq!(
        page.events[0].payload["text"],
        "new replacement longer than old"
    );
    let second_path = transcript_path(&root, cwd, "transcript-b");
    write_records(&second_path, &[user("second held session")]);
    registry
        .feeds
        .use_transcript("session", "worker-a", cwd, "transcript-b")
        .unwrap();
    registry
        .feeds
        .refresh_transcript("session", "worker-a")
        .unwrap();
    let page = registry
        .feeds
        .history(
            "session",
            "worker-a",
            None,
            page.latest_cursor.as_deref(),
            100,
        )
        .unwrap();
    assert!(page.reset);
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].payload["text"], "second held session");
}

#[test]
fn restart_source_mode_prevents_double_feed_and_preserves_protocol_history() {
    let temp = tempfile::tempdir().unwrap();
    let sessions = temp.path().join("sessions");
    let root = temp.path().join("claude-projects");
    let cwd = Path::new("fixture");
    write_records(
        &transcript_path(&root, cwd, "transcript"),
        &[json!({"type":"user","message":{"role":"user","content":"old transcript"}})],
    );
    let first = TransportRegistry::new(sessions.clone(), Some(root.clone()));
    first
        .feeds
        .use_transcript("session", "worker-a", cwd, "transcript")
        .unwrap();
    first
        .feeds
        .refresh_transcript("session", "worker-a")
        .unwrap();
    drop(first);
    let second = TransportRegistry::new(sessions.clone(), Some(root.clone()));
    second
        .register("session", "worker-a", None, FakeTransport::new())
        .unwrap();
    second
        .append_event("session", "worker-a", event("worker-a", "protocol history"))
        .unwrap();
    let page = second
        .feeds
        .history("session", "worker-a", None, None, 100)
        .unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].payload["text"], "protocol history");
    let cursor = page.latest_cursor;
    drop(second);
    let third = TransportRegistry::new(sessions, Some(root));
    third
        .register("session", "worker-a", None, FakeTransport::new())
        .unwrap();
    let page = third
        .feeds
        .history("session", "worker-a", None, None, 100)
        .unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(
        page.latest_cursor, cursor,
        "protocol journal survives restart without a generation change"
    );
}

#[tokio::test]
async fn persisted_worker_uses_its_own_cwd_and_held_transcript_id() {
    let (temp, state, app) = setup();
    let mut session = make_test_session_with_agents("session", "D:/fixture/project", &["worker-a"]);
    session.execution_policy.workspace_strategy = crate::domain::WorkspaceStrategy::IsolatedCell;
    session.worktree_path = Some("D:/fixture/project/.hive-manager/worktrees/session/queen".into());
    session.agents[0].transcript_session_id = Some("worker-transcript".into());
    session.agents[0].role = crate::pty::AgentRole::Worker {
        index: 2,
        parent: Some("session-queen".into()),
    };
    let cwd = session
        .project_path
        .join(".hive-manager/worktrees/session/worker-2");
    write_records(
        &transcript_path(
            &temp.path().join("claude-projects"),
            &cwd,
            "worker-transcript",
        ),
        &[json!({"type":"user","message":{"role":"user","content":"archived own transcript"}})],
    );
    state.session_controller.read().insert_test_session(session);
    state
        .session_controller
        .read()
        .persist_test_session("session");
    let mut cold = SessionController::new(state.pty_manager.clone());
    cold.set_storage(state.storage.clone());
    *state.session_controller.write() = cold;
    let page = body_json(get(&app, "/api/sessions/session/agents/worker-a/events").await).await;
    assert_eq!(
        page["events"][0]["payload"]["text"],
        "archived own transcript"
    );
}

#[cfg(unix)]
#[test]
fn feed_symlink_parents_cannot_create_or_truncate_another_session() {
    let temp = tempfile::tempdir().unwrap();
    let sessions = temp.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, sessions.join("session")).unwrap();
    let registry = TransportRegistry::new(sessions.clone(), None);
    assert!(registry
        .register("session", "worker-a", None, FakeTransport::new())
        .is_err());
    assert!(
        !outside.join("agent-feeds").exists(),
        "reject parent before any directory mutation"
    );
    std::fs::remove_file(sessions.join("session")).unwrap();
    std::fs::create_dir(sessions.join("session")).unwrap();
    let other = sessions.join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("worker-a.jsonl"), "sentinel").unwrap();
    std::os::unix::fs::symlink(&other, sessions.join("session/agent-feeds")).unwrap();
    assert!(registry
        .register("session", "worker-a", None, FakeTransport::new())
        .is_err());
    assert_eq!(
        std::fs::read_to_string(other.join("worker-a.jsonl")).unwrap(),
        "sentinel"
    );
}

#[cfg(unix)]
#[test]
fn dangling_file_symlink_cannot_create_an_external_target() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("sessions/session/agent-feeds");
    std::fs::create_dir_all(&directory).unwrap();
    let external = temp.path().join("external.jsonl");
    std::os::unix::fs::symlink(&external, directory.join("worker-a.jsonl")).unwrap();
    let registry = TransportRegistry::new(temp.path().join("sessions"), None);
    assert!(registry
        .register("session", "worker-a", None, FakeTransport::new())
        .is_err());
    assert!(!external.exists());
}

#[test]
fn history_tail_first_pages_and_forward_replay_cross_two_mib_windows() {
    let temp = tempfile::tempdir().unwrap();
    let registry = TransportRegistry::new(temp.path().into(), None);
    registry
        .register("session", "worker-a", None, FakeTransport::new())
        .unwrap();
    let mut expected = Vec::new();
    for index in 0..350 {
        let mut item = event("worker-a", &format!("record-{index}"));
        item.payload = json!({"text": "x".repeat(16000), "index":index});
        expected.push(
            registry
                .append_event("session", "worker-a", item)
                .unwrap()
                .cursor,
        );
    }
    let first = registry
        .feeds
        .history("session", "worker-a", None, None, 20)
        .unwrap();
    assert_eq!(first.events.len(), 20);
    assert_eq!(first.events.last().unwrap().payload["index"], 349);
    let previous = registry
        .feeds
        .history(
            "session",
            "worker-a",
            first.next_before.as_deref(),
            None,
            20,
        )
        .unwrap();
    assert_eq!(previous.events.last().unwrap().payload["index"], 329);
    let mut actual = vec![expected[0].clone()];
    loop {
        let page = registry
            .feeds
            .history(
                "session",
                "worker-a",
                None,
                actual.last().map(String::as_str),
                50,
            )
            .unwrap();
        actual.extend(page.events.iter().map(|e| e.cursor.clone()));
        if !page.has_more {
            break;
        }
        assert!(!page.events.is_empty());
    }
    assert_eq!(
        actual, expected,
        "forward replay cannot duplicate or skip across bounded windows"
    );
    assert!(
        Cursor::decode(&actual[1]).unwrap().byte_offset
            > Cursor::decode(&actual[0]).unwrap().byte_offset
    );
}

async fn next_sse<S>(stream: &mut S) -> (String, Value)
where
    S: Stream<Item = Result<Bytes, axum::Error>> + Unpin,
{
    loop {
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = String::from_utf8(chunk.to_vec()).unwrap();
        if !text.contains("event: agent_event") {
            continue;
        }
        let id = text
            .lines()
            .find_map(|l| l.strip_prefix("id: "))
            .unwrap()
            .into();
        let data = text.lines().find_map(|l| l.strip_prefix("data: ")).unwrap();
        return (id, serde_json::from_str(data).unwrap());
    }
}

#[tokio::test]
async fn multiplexed_sse_reconnect_replays_both_agents_without_duplicate_or_skip() {
    let (_temp, state, app) = setup();
    let session = make_test_session_with_agents("session", "fixture", &["worker-a", "worker-b"]);
    state.session_controller.read().insert_test_session(session);
    let registry = state.transports();
    for aid in ["worker-a", "worker-b"] {
        registry
            .register("session", aid, None, FakeTransport::new())
            .unwrap();
    }
    registry
        .append_event("session", "worker-a", event("worker-a", "a1"))
        .unwrap();
    registry
        .append_event("session", "worker-b", event("worker-b", "b1"))
        .unwrap();
    let response = get(&app, "/api/sessions/session/agent-events/stream").await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let (_, first) = next_sse(&mut stream).await;
    let (checkpoint, second) = next_sse(&mut stream).await;
    assert_ne!(first["agent_id"], second["agent_id"]);
    drop(stream);
    let a2 = registry
        .append_event("session", "worker-a", event("worker-a", "a2"))
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session/agent-events/stream")
                .header("last-event-id", checkpoint)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let b2 = registry
        .append_event("session", "worker-b", event("worker-b", "b2"))
        .unwrap();
    let a3 = registry
        .append_event("session", "worker-a", event("worker-a", "a3"))
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let mut received = Vec::new();
    let mut last_checkpoint = String::new();
    for _ in 0..3 {
        let (checkpoint, frame) = next_sse(&mut stream).await;
        last_checkpoint = checkpoint;
        received.push(frame["source_id"].as_str().unwrap().to_owned());
    }
    received.sort();
    let mut expected = vec![a2, b2, a3]
        .into_iter()
        .map(|e| e.source_id.unwrap())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(
        received, expected,
        "SSE reconnect drops replay/live duplicates and misses no agent"
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(400), next_sse(&mut stream))
            .await
            .is_err(),
        "no duplicate frame after replay"
    );
    drop(stream);
    let mut session = state
        .session_controller
        .read()
        .get_session("session")
        .unwrap();
    session.agents.retain(|agent| agent.id != "worker-b");
    state.session_controller.read().insert_test_session(session);
    registry
        .append_event("session", "worker-a", event("worker-a", "a4"))
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session/agent-events/stream")
                .header("last-event-id", last_checkpoint)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "removed roster key cannot poison reconnect"
    );
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(next_sse(&mut stream).await.1["source_id"], "a4");
}
