//! HTTP tests for terminal continuity: the PTY snapshot route (#287) and the fixture
//! purge route (#288).

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use super::tests::{
    isolated_storage_base, make_test_session, setup_test_app_with_controller_at,
    test_default_max_qa_iterations,
};
use crate::storage::{PersistedSession, SessionTypeInfo};

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn persisted(id: &str, project_path: &str) -> PersistedSession {
    PersistedSession {
        id: id.to_string(),
        name: Some("Fixture".to_string()),
        color: None,
        session_type: SessionTypeInfo::Hive { worker_count: 1 },
        project_path: project_path.to_string(),
        created_at: chrono::Utc::now(),
        last_activity_at: None,
        agents: vec![],
        state: "Completed".to_string(),
        default_cli: "claude".to_string(),
        default_model: None,
        default_principal_cli: None,
        default_principal_model: None,
        default_principal_flags: Vec::new(),
        execution_policy: crate::domain::HiveExecutionPolicy::default(),
        qa_workers: Vec::new(),
        max_qa_iterations: test_default_max_qa_iterations(),
        qa_timeout_secs: 300,
        qa_inconclusive_at: None,
        auth_strategy: String::new(),
        worktree_path: None,
        worktree_branch: None,
        no_git: false,
    }
}

fn missing_temp_project() -> String {
    std::env::temp_dir()
        .join(format!("hive-fixture-project-{}", uuid::Uuid::new_v4()))
        .to_string_lossy()
        .to_string()
}

fn ids(report: &Value, key: &str) -> Vec<String> {
    report[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap().to_string())
        .collect()
}

/// #287: a freshly mounted pane can fetch the retained history, aligned to a parseable
/// boundary, with the absolute offsets it needs to splice in the live stream.
#[cfg(windows)]
#[tokio::test]
async fn pty_snapshot_route_returns_aligned_history_with_offsets() {
    use super::tests::{make_test_session_with_agents, register_live_pty, setup_test_app_full};
    use base64::Engine;
    use crate::pty::AgentRole;

    let (app, controller, _storage, state) = setup_test_app_full(isolated_storage_base()).await;
    let session_id = format!("snapshot-{}", uuid::Uuid::new_v4());
    let project = TempDir::new().unwrap();
    let agent_id = format!("{session_id}-worker-1");
    let session = make_test_session_with_agents(
        &session_id,
        project.path().to_str().unwrap(),
        &[&agent_id],
    );
    controller.write().insert_test_session(session);
    register_live_pty(&state, &agent_id, AgentRole::Worker { index: 1, parent: None });

    // Write a frame, then enough after it that ring eviction cuts three bytes into its
    // leading cursor-move sequence (the ring evicts from the front).
    let capacity = state.pty_manager.read().replay_capacity();
    let frame = b"\x1b[12;34Hafter\x1b[2Jtail";
    let padding = vec![b'x'; capacity - frame.len() + 3];
    state.pty_manager.read().record_output_for_test(&agent_id, frame);
    state.pty_manager.read().record_output_for_test(&agent_id, &padding);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/sessions/{session_id}/agents/{agent_id}/pty-snapshot"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;

    let data = base64::engine::general_purpose::STANDARD
        .decode(body["data"].as_str().unwrap())
        .unwrap();
    assert!(
        data.starts_with(b"\x1b[2Jtail"),
        "snapshot must start at the first intact ESC, got {:?}",
        String::from_utf8_lossy(&data[..data.len().min(24)])
    );
    let total = (padding.len() + frame.len()) as u64;
    assert_eq!(body["id"], agent_id);
    assert_eq!(body["offset_end"], total);
    assert_eq!(
        body["offset_end"].as_u64().unwrap() - body["offset_start"].as_u64().unwrap(),
        data.len() as u64
    );

    // The inject-facing tail view is unchanged: still the last 8 KB, lossily decoded.
    let tail = state.pty_manager.read().recent_output(&agent_id).unwrap();
    assert_eq!(tail.len(), 8 * 1024);
    assert!(tail.bytes().all(|byte| byte == b'x'));

    // An agent that is not in the session roster is a 404, not a leak of another PTY.
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/sessions/{session_id}/agents/{session_id}-worker-9/pty-snapshot"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// #288 review follow-up: purge and resume share the per-session lifecycle lock, so a
/// fixture cannot be loaded into the controller while its directory is being deleted.
#[tokio::test]
async fn purge_delete_waits_for_the_session_lifecycle_lock_and_keeps_live_sessions() {
    let (_app, controller, storage) =
        setup_test_app_with_controller_at(isolated_storage_base()).await;
    let id = format!("working-active-{}", uuid::Uuid::new_v4());
    storage
        .save_session(&persisted(&id, &missing_temp_project()))
        .unwrap();

    // Hold the lock the way an in-flight resume would.
    let lock = controller.read().session_lifecycle_lock(&id);
    let held = lock.lock();

    let worker = {
        let controller = controller.clone();
        let storage = storage.clone();
        let id = id.clone();
        std::thread::spawn(move || {
            controller
                .read()
                .purge_persisted_session_unless_live(&storage, &id)
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(!worker.is_finished(), "purge must wait for the lifecycle lock");
    assert!(storage.session_dir(&id).exists(), "nothing is deleted while the lock is held");

    drop(held);
    assert!(worker.join().unwrap().unwrap(), "released lock lets the purge proceed");
    assert!(!storage.session_dir(&id).exists());

    // A session that is live in the controller is kept, and the caller learns that.
    let live = format!("session-live-{}", uuid::Uuid::new_v4());
    let live_project = missing_temp_project();
    storage
        .save_session(&persisted(&live, &live_project))
        .unwrap();
    controller
        .write()
        .insert_test_session(make_test_session(&live, &live_project));
    assert!(!controller
        .read()
        .purge_persisted_session_unless_live(&storage, &live)
        .unwrap());
    assert!(storage.session_dir(&live).exists());
}

/// #288: the purge is a dry run by default, deletes only provable fixtures when applied,
/// and reports every suspicious session it deliberately kept.
#[tokio::test]
async fn purge_fixture_sessions_dry_run_then_apply_removes_only_leaked_fixtures() {
    let (app, controller, storage) =
        setup_test_app_with_controller_at(isolated_storage_base()).await;

    // Leaked fixture: non-UUID id, temp project path that no longer exists.
    let fixture_id = format!("working-active-{}", uuid::Uuid::new_v4());
    storage
        .save_session(&persisted(&fixture_id, &missing_temp_project()))
        .unwrap();
    // Leaked fixture created through the API: UUID id, but its TempDir project is gone.
    let temp_uuid_fixture = uuid::Uuid::new_v4().to_string();
    storage
        .save_session(&persisted(&temp_uuid_fixture, &missing_temp_project()))
        .unwrap();
    // Real session: UUID id and a project that exists.
    let real_id = uuid::Uuid::new_v4().to_string();
    let real_project = TempDir::new().unwrap();
    storage
        .save_session(&persisted(&real_id, real_project.path().to_str().unwrap()))
        .unwrap();
    // Real session whose project was moved: UUID id, missing path outside the temp dir.
    let moved_id = uuid::Uuid::new_v4().to_string();
    storage
        .save_session(&persisted(&moved_id, "Z:/definitely/not/here/hive-moved-project"))
        .unwrap();
    // Fixture-shaped id that the controller still holds live.
    let live_id = format!("session-live-{}", uuid::Uuid::new_v4());
    let live_project = missing_temp_project();
    storage
        .save_session(&persisted(&live_id, &live_project))
        .unwrap();
    controller
        .write()
        .insert_test_session(make_test_session(&live_id, &live_project));
    // Unreadable session.json: a UUID id is kept and reported, a fixture-shaped id is
    // a candidate (no real session can have a non-UUID id).
    let unreadable_real_id = uuid::Uuid::new_v4().to_string();
    let unreadable_fixture_id = format!("queen-working-{}", uuid::Uuid::new_v4());
    for id in [&unreadable_real_id, &unreadable_fixture_id] {
        let dir = storage.session_dir(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), b"{ not json").unwrap();
    }

    let dry_run = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/maintenance/purge-fixture-sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(dry_run.status(), StatusCode::OK);
    let report = json_body(dry_run).await;

    assert_eq!(report["applied"], false);
    assert_eq!(report["removed"], 0);
    assert_eq!(report["scanned"], 7);
    let mut candidates = ids(&report, "candidates");
    candidates.sort();
    let mut expected = vec![
        fixture_id.clone(),
        temp_uuid_fixture.clone(),
        unreadable_fixture_id.clone(),
    ];
    expected.sort();
    assert_eq!(candidates, expected);
    let skipped = ids(&report, "skipped");
    assert!(skipped.contains(&moved_id), "moved project must be kept: {report}");
    assert!(skipped.contains(&live_id), "live session must be kept: {report}");
    assert!(
        skipped.contains(&unreadable_real_id),
        "unreadable UUID session must be kept and reported: {report}"
    );
    assert!(!skipped.contains(&real_id), "healthy sessions are not even reported");
    for id in [
        &fixture_id,
        &temp_uuid_fixture,
        &unreadable_fixture_id,
        &real_id,
        &moved_id,
        &live_id,
        &unreadable_real_id,
    ] {
        assert!(storage.session_dir(id).exists(), "dry run must not delete {id}");
    }

    let applied = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/maintenance/purge-fixture-sessions?apply=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(applied.status(), StatusCode::OK);
    let report = json_body(applied).await;

    assert_eq!(report["applied"], true);
    assert_eq!(report["removed"], 3);
    assert_eq!(report["errors"].as_array().unwrap().len(), 0);
    assert!(!storage.session_dir(&fixture_id).exists());
    assert!(!storage.session_dir(&temp_uuid_fixture).exists());
    assert!(!storage.session_dir(&unreadable_fixture_id).exists());
    for id in [&real_id, &moved_id, &live_id, &unreadable_real_id] {
        assert!(storage.session_dir(id).exists(), "apply must keep {id}");
    }
}
