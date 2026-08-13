//! Readiness-queue tests for issue #212, owned by WS-4.

use std::path::Path;
use std::sync::{Arc, Barrier};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use parking_lot::RwLock;
use serde_json::json;
use tower::ServiceExt;

use crate::coordination::queue_manager::{ClaimOutcome, QueueManager};
use crate::coordination::{InjectionManager, StateManager};
use crate::domain::event::{EventType, Severity};
use crate::domain::HiveExecutionPolicy;
use crate::events::EventBus;
use crate::http::handlers::workers::AddWorkerRequest;
use crate::http::routes::create_router;
use crate::http::state::AppState;
use crate::orchestrator::work_graph::review::checkpoint_aware_claimable_nodes;
use crate::orchestrator::work_graph::{
    BindingRef, EdgeKind, EdgeProvenance, NodeContract, NodeKind, NodeStatus, TaskGraph,
    WorkEdge, WorkNode,
};
use crate::pty::PtyManager;
use crate::session::{
    AuthStrategy, Session, SessionController, SessionState, SessionType,
};
use crate::storage::queue::{QueueRow, QueueStatus};
use crate::storage::{ApplicationStateDb, QueueRepo, SessionStorage};

const SESSION_ID: &str = "readiness-session";

fn queue_repo() -> QueueRepo {
    let db = Arc::new(ApplicationStateDb::open_in_memory().expect("in-memory queue database"));
    let repo = QueueRepo::new(db);
    repo.ensure_schema().expect("queue schema");
    repo
}

fn queued_row(id: &str, worker_id: &str, task_id: Option<&str>, created_at: i64) -> QueueRow {
    queue_row(
        id,
        worker_id,
        task_id,
        QueueStatus::Queued,
        created_at,
    )
}

fn queue_row(
    id: &str,
    worker_id: &str,
    task_id: Option<&str>,
    status: QueueStatus,
    created_at: i64,
) -> QueueRow {
    QueueRow {
        id: id.to_string(),
        task_id: task_id.map(str::to_string),
        session_id: SESSION_ID.to_string(),
        worker_id: worker_id.to_string(),
        role_type: "backend".to_string(),
        cli: "codex".to_string(),
        status,
        payload: json!({"prompt": id}),
        attempts: 0,
        continuation_count: 0,
        no_progress_count: 0,
        last_status: None,
        heartbeat_at: None,
        blocked_reason: None,
        created_at,
        updated_at: created_at,
    }
}

fn enqueue_diamond(repo: &QueueRepo) {
    repo.enqueue_with_dependencies(&queued_row("run-a", "worker-a", Some("A"), 1), &[])
        .unwrap();
    repo.enqueue_with_dependencies(
        &queued_row("run-b", "worker-b", Some("B"), 2),
        &["A".to_string()],
    )
    .unwrap();
    repo.enqueue_with_dependencies(
        &queued_row("run-c", "worker-c", Some("C"), 3),
        &["A".to_string()],
    )
    .unwrap();
    repo.enqueue_with_dependencies(
        &queued_row("run-d", "worker-d", Some("D"), 4),
        &["B".to_string(), "C".to_string()],
    )
    .unwrap();
}

fn race_claims(repo: &QueueRepo, ids: &[&str], now_ms: i64) -> Vec<Option<i64>> {
    let barrier = Arc::new(Barrier::new(ids.len() + 1));
    let handles = ids
        .iter()
        .map(|id| {
            let repo = repo.clone();
            let barrier = Arc::clone(&barrier);
            let id = (*id).to_string();
            std::thread::spawn(move || {
                barrier.wait();
                repo.try_claim(&id, now_ms - 90_000, now_ms).unwrap()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    handles
        .into_iter()
        .map(|handle| handle.join().expect("claim thread"))
        .collect()
}

fn race_bound_claims(
    repo: &QueueRepo,
    ids: &[&str],
    worker_id: &str,
    now_ms: i64,
) -> Vec<Option<i64>> {
    let barrier = Arc::new(Barrier::new(ids.len() + 1));
    let handles = ids
        .iter()
        .map(|id| {
            let repo = repo.clone();
            let barrier = Arc::clone(&barrier);
            let id = (*id).to_string();
            let worker_id = worker_id.to_string();
            std::thread::spawn(move || {
                barrier.wait();
                repo.try_claim_for_worker(
                    &id,
                    Some(&worker_id),
                    now_ms - 90_000,
                    now_ms,
                )
                .unwrap()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    handles
        .into_iter()
        .map(|handle| handle.join().expect("bound claim thread"))
        .collect()
}

#[test]
fn diamond_claims_are_dependency_gated_atomic_and_eventually_unblock() {
    let repo = queue_repo();
    enqueue_diamond(&repo);

    let premature = race_claims(&repo, &["run-b", "run-c"], 10);
    assert_eq!(
        premature,
        vec![None, None],
        "B and C must remain queued until A finalizes"
    );

    let a_epoch = repo
        .try_claim("run-a", -90_000, 20)
        .unwrap()
        .expect("root A is ready");
    assert_eq!(a_epoch, 1, "attempts remains the fencing epoch");
    let claimed_a = repo.get_row("run-a").unwrap().unwrap();
    assert_eq!(claimed_a.heartbeat_at, Some(20), "claim stamps heartbeat");
    repo.record_heartbeat(SESSION_ID, "worker-a", "completed", 30)
        .unwrap();

    let siblings = race_claims(&repo, &["run-b", "run-c"], 40);
    assert!(
        siblings.iter().all(Option::is_some),
        "B and C must both become independently claimable after A finalizes: {siblings:?}"
    );
    assert_eq!(repo.try_claim("run-d", -90_000, 50).unwrap(), None);

    repo.record_heartbeat(SESSION_ID, "worker-b", "completed", 60)
        .unwrap();
    assert_eq!(
        repo.try_claim("run-d", -90_000, 70).unwrap(),
        None,
        "D still waits for C"
    );
    repo.record_heartbeat(SESSION_ID, "worker-c", "completed", 80)
        .unwrap();

    let same_ready_row = race_claims(&repo, &["run-d", "run-d"], 90);
    assert_eq!(
        same_ready_row.iter().filter(|epoch| epoch.is_some()).count(),
        1,
        "exactly one concurrent claimer may win the same ready row: {same_ready_row:?}"
    );
    let claimed_d = repo.get_row("run-d").unwrap().unwrap();
    assert_eq!(claimed_d.attempts, 1);
    assert_eq!(claimed_d.heartbeat_at, Some(90));
}

#[test]
fn terminal_failure_blocks_only_transitive_descendants_with_reason() {
    let repo = queue_repo();
    enqueue_diamond(&repo);
    repo.enqueue(&queued_row("run-e", "worker-e", Some("E"), 5))
        .unwrap();

    let epoch = repo.try_claim("run-a", -90_000, 10).unwrap().unwrap();
    assert!(repo.fail_claimed("run-a", epoch, 20).unwrap());

    let snapshot = repo.snapshot(SESSION_ID).unwrap();
    assert_eq!(snapshot.failed, 1);
    assert_eq!(snapshot.blocked, 3);
    assert_eq!(snapshot.queued, 1);
    for id in ["run-b", "run-c", "run-d"] {
        let row = repo.get_row(id).unwrap().unwrap();
        assert_eq!(row.status, QueueStatus::Blocked);
        assert!(
            row.blocked_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("A") && reason.contains("failed")),
            "{id} omitted its failed ancestor: {:?}",
            row.blocked_reason
        );
        assert_eq!(repo.try_claim(id, -90_000, 30).unwrap(), None);
    }
    assert!(repo.try_claim("run-e", -90_000, 30).unwrap().is_some());
}

#[test]
fn cancellation_blocks_root_and_transitive_descendants_with_reason() {
    let repo = queue_repo();
    enqueue_diamond(&repo);
    repo.enqueue(&queued_row("run-e", "worker-e", Some("E"), 5))
        .unwrap();

    let blocked = repo
        .cancel_task_and_descendants(SESSION_ID, "A", "operator cancelled", 10)
        .unwrap();
    assert_eq!(blocked, vec!["run-a", "run-b", "run-c", "run-d"]);
    for id in &blocked {
        let row = repo.get_row(id).unwrap().unwrap();
        assert_eq!(row.status, QueueStatus::Blocked);
        assert!(
            row.blocked_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("A") && reason.contains("cancelled"))
        );
        assert_eq!(repo.try_claim(id, -90_000, 20).unwrap(), None);
    }
    assert!(repo.try_claim("run-e", -90_000, 20).unwrap().is_some());
}

#[test]
fn edgeless_and_legacy_rows_keep_existing_claim_behavior_and_order() {
    let repo = queue_repo();
    repo.ensure_schema().expect("idempotent additive upgrade");
    repo.enqueue(&queued_row("run-b", "worker-b", Some("B"), 1))
        .unwrap();
    repo.enqueue(&queued_row("run-a", "worker-a", None, 1))
        .unwrap();

    let claims = race_claims(&repo, &["run-a", "run-b"], 10);
    assert!(claims.iter().all(Option::is_some));
    assert!(race_claims(&repo, &["run-a", "run-b"], 20)
        .iter()
        .all(Option::is_none));
    let snapshot = repo.snapshot(SESSION_ID).unwrap();
    assert_eq!(
        snapshot
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        vec!["run-a", "run-b"],
        "legacy snapshot ordering remains created_at then id"
    );
}

#[test]
fn task_requeue_releases_worker_binding_before_slot_reuse() {
    let repo = queue_repo();
    repo.enqueue(&queued_row("run-b", "pending:run-b", Some("B"), 1))
        .unwrap();
    repo.enqueue(&queued_row("run-c", "pending:run-c", Some("C"), 2))
        .unwrap();

    let b_epoch = repo
        .try_claim_for_worker("run-b", Some("worker-1"), -90_000, 10)
        .unwrap()
        .unwrap();
    assert!(repo.requeue_claimed("run-b", b_epoch, 20).unwrap());
    let requeued_b = repo.get_row("run-b").unwrap().unwrap();
    assert_eq!(requeued_b.status, QueueStatus::Queued);
    assert_eq!(requeued_b.worker_id, "pending:run-b");

    repo.try_claim_for_worker("run-c", Some("worker-1"), -90_000, 30)
        .unwrap()
        .expect("C reuses the released slot");
    repo.record_heartbeat(SESSION_ID, "worker-1", "completed", 40)
        .unwrap();

    let b_after_c = repo.get_row("run-b").unwrap().unwrap();
    let c_after_completion = repo.get_row("run-c").unwrap().unwrap();
    assert_eq!(b_after_c.status, QueueStatus::Queued);
    assert_eq!(b_after_c.worker_id, "pending:run-b");
    assert_eq!(c_after_completion.status, QueueStatus::Finalized);
}

#[test]
fn distinct_tasks_cannot_concurrently_claim_the_same_worker_slot() {
    let repo = queue_repo();
    repo.enqueue(&queued_row("run-b", "pending:run-b", Some("B"), 1))
        .unwrap();
    repo.enqueue(&queued_row("run-c", "pending:run-c", Some("C"), 2))
        .unwrap();

    let claims = race_bound_claims(&repo, &["run-b", "run-c"], "worker-1", 10);
    assert_eq!(
        claims.iter().filter(|claim| claim.is_some()).count(),
        1,
        "one roster slot cannot back two running task rows: {claims:?}"
    );
    let rows = repo.snapshot(SESSION_ID).unwrap().rows;
    assert_eq!(
        rows.iter()
            .filter(|row| row.status == QueueStatus::Running && row.worker_id == "worker-1")
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.status == QueueStatus::Queued)
            .count(),
        1
    );
}

#[tokio::test]
async fn recovery_prioritizes_live_claim_over_older_terminal_slot_history() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(queue_repo());
    repo.enqueue(&queue_row(
        "old-finalized",
        "worker-1",
        Some("A"),
        QueueStatus::Finalized,
        1,
    ))
    .unwrap();
    repo.enqueue(&queue_row(
        "old-failed",
        "worker-1",
        Some("B"),
        QueueStatus::Failed,
        2,
    ))
    .unwrap();
    repo.enqueue(&queue_row(
        "live-running",
        "worker-1",
        Some("C"),
        QueueStatus::Running,
        3,
    ))
    .unwrap();
    let manager = QueueManager::new(
        Arc::clone(&repo),
        EventBus::new(temp.path().to_path_buf()),
    );

    assert_eq!(
        manager.release_claim(SESSION_ID, "worker-1").await.unwrap(),
        crate::coordination::ReleaseOutcome::Released {
            previous: QueueStatus::Running
        }
    );
    assert_eq!(repo.get_row("live-running").unwrap().unwrap().status, QueueStatus::Queued);
    assert_eq!(repo.get_row("old-failed").unwrap().unwrap().status, QueueStatus::Failed);
    assert_eq!(repo.get_row("old-finalized").unwrap().unwrap().status, QueueStatus::Finalized);

    assert_eq!(
        manager.release_claim(SESSION_ID, "worker-1").await.unwrap(),
        crate::coordination::ReleaseOutcome::Released {
            previous: QueueStatus::Failed
        }
    );
    assert_eq!(repo.get_row("old-failed").unwrap().unwrap().status, QueueStatus::Queued);
    assert_eq!(
        manager.release_claim(SESSION_ID, "worker-1").await.unwrap(),
        crate::coordination::ReleaseOutcome::Terminal {
            status: QueueStatus::Finalized
        }
    );
}

#[tokio::test]
async fn typed_pending_outcome_retries_to_real_claim_and_events_keep_task_id() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(queue_repo());
    let event_bus = EventBus::new(temp.path().to_path_buf());
    let mut events = event_bus.subscribe();
    let manager = QueueManager::new(Arc::clone(&repo), event_bus);

    manager
        .enqueue_worker_with_dependencies(
            "run-a",
            SESSION_ID,
            "worker-a",
            "backend",
            "codex",
            json!({}),
            Some("A".to_string()),
            &[],
        )
        .await
        .unwrap();
    manager
        .enqueue_worker_with_dependencies(
            "run-b",
            SESSION_ID,
            "worker-b",
            "backend",
            "codex",
            json!({}),
            Some("B".to_string()),
            &["A".to_string()],
        )
        .await
        .unwrap();

    assert_eq!(
        manager
            .claim_and_spawn("run-b", SESSION_ID, "worker-b")
            .await
            .unwrap(),
        ClaimOutcome::DependenciesPending {
            task_ids: vec!["A".to_string()]
        }
    );
    assert!(matches!(
        manager
            .claim_and_spawn("run-a", SESSION_ID, "worker-a")
            .await
            .unwrap(),
        ClaimOutcome::Claimed { epoch: 1 }
    ));
    manager
        .record_heartbeat(SESSION_ID, "worker-a", "completed")
        .await
        .unwrap();
    assert!(matches!(
        manager
            .claim_and_spawn("run-b", SESSION_ID, "worker-b")
            .await
            .unwrap(),
        ClaimOutcome::Claimed { epoch: 1 }
    ));
    assert_eq!(repo.get_row("run-b").unwrap().unwrap().task_id.as_deref(), Some("B"));

    let mut observed = Vec::new();
    for _ in 0..6 {
        observed.push(events.recv().await.unwrap());
    }
    let pending = observed
        .iter()
        .find(|event| {
            event.event_type == EventType::WorkerClaimFailed
                && event.agent_id.as_deref() == Some("worker-b")
        })
        .expect("normal dependency wait event");
    assert_eq!(pending.severity, Severity::Info);
    assert_eq!(pending.payload["task_id"], "B");
    assert!(observed.iter().all(|event| event.payload.get("task_id").is_some()));
}

#[test]
fn checkpoint_projection_and_queue_sql_agree_on_claimability() {
    fn node(id: &str, status: NodeStatus) -> WorkNode {
        WorkNode::new(
            id,
            if id == "gate" {
                NodeKind::Checkpoint
            } else {
                NodeKind::Task
            },
            id,
            NodeContract::default(),
            BindingRef::Role("worker".to_string()),
            status,
        )
    }

    let mut graph = TaskGraph::new(
        vec![
            node("A", NodeStatus::Completed),
            node("gate", NodeStatus::Ready),
            node("B", NodeStatus::Ready),
        ],
        vec![
            WorkEdge::new("A", "gate", EdgeKind::DependsOn, EdgeProvenance::Planner),
            WorkEdge::new("gate", "B", EdgeKind::DependsOn, EdgeProvenance::Planner),
        ],
    );
    assert_eq!(checkpoint_aware_claimable_nodes(&graph), vec!["gate"]);

    let repo = queue_repo();
    repo.enqueue(&queue_row(
        "run-a",
        "worker-a",
        Some("A"),
        QueueStatus::Finalized,
        1,
    ))
    .unwrap();
    repo.enqueue_with_dependencies(
        &queued_row("run-gate", "worker-gate", Some("gate"), 2),
        &["A".to_string()],
    )
    .unwrap();
    repo.enqueue_with_dependencies(
        &queued_row("run-b", "worker-b", Some("B"), 3),
        &["gate".to_string()],
    )
    .unwrap();
    assert!(repo.try_claim("run-gate", -90_000, 10).unwrap().is_some());
    assert_eq!(repo.try_claim("run-b", -90_000, 10).unwrap(), None);

    repo.record_heartbeat(SESSION_ID, "worker-gate", "completed", 20)
        .unwrap();
    graph
        .nodes
        .iter_mut()
        .find(|node| node.id == "gate")
        .unwrap()
        .status = NodeStatus::Completed;
    assert_eq!(checkpoint_aware_claimable_nodes(&graph), vec!["B"]);
    assert!(repo.try_claim("run-b", -90_000, 30).unwrap().is_some());
}

#[test]
fn add_worker_request_task_id_is_explicit_or_null_and_backward_compatible() {
    let legacy: AddWorkerRequest = serde_json::from_value(json!({
        "role_type": "backend",
        "initial_task": "Implement T7, but never infer from this prose"
    }))
    .unwrap();
    assert_eq!(legacy.task_id, None);

    let linked: AddWorkerRequest = serde_json::from_value(json!({
        "role_type": "backend",
        "initial_task": "arbitrary prose",
        "task_id": "T7"
    }))
    .unwrap();
    assert_eq!(linked.task_id.as_deref(), Some("T7"));
}

fn dependency_http_fixture(
    temp: &tempfile::TempDir,
) -> (axum::Router, Arc<AppState>, Arc<RwLock<SessionController>>) {
    let base = temp.path().join("http-storage");
    let storage = Arc::new(SessionStorage::new_with_base(base.clone()).unwrap());
    let config = Arc::new(tokio::sync::RwLock::new(storage.load_config().unwrap()));
    let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
    let controller = Arc::new(RwLock::new(SessionController::new(Arc::clone(
        &pty_manager,
    ))));
    controller.write().set_storage(Arc::clone(&storage));
    let injection = Arc::new(RwLock::new(InjectionManager::new(
        Arc::clone(&pty_manager),
        SessionStorage::new_with_base(base).unwrap(),
    )));
    let event_bus = EventBus::new(storage.base_dir().clone());
    let db = Arc::new(ApplicationStateDb::open(storage.base_dir()).unwrap());
    let repo = Arc::new(QueueRepo::new(Arc::clone(&db)));
    repo.ensure_schema().unwrap();
    let queue_manager = Arc::new(QueueManager::new(repo, Arc::clone(&event_bus)));
    let state = Arc::new(AppState::new(
        config,
        pty_manager,
        Arc::clone(&controller),
        injection,
        Arc::clone(&storage),
        event_bus,
        db,
        queue_manager,
        None,
    ));
    state.set_registry(Arc::new(crate::actions::build_registry()));
    (create_router(Arc::clone(&state)), state, controller)
}

async fn post_task_worker(app: &axum::Router, task_id: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/http-dependency-session/workers")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "role_type": "researcher",
                        "cli": "codex",
                        "task_id": task_id,
                        "initial_task": format!("Execute {task_id}")
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn http_retry_spawns_dependent_without_reusing_another_tasks_queue_identity() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    state.storage.create_session_dir(session_id).unwrap();
    controller
        .read()
        .insert_test_session(quiet_hive_session(session_id, &project));

    let graph = TaskGraph::new(
        vec![
            queue_test_node("A", NodeStatus::Ready),
            queue_test_node("B", NodeStatus::Ready),
            queue_test_node("C", NodeStatus::Ready),
        ],
        vec![WorkEdge::new(
            "A",
            "B",
            EdgeKind::DependsOn,
            EdgeProvenance::Planner,
        )],
    );
    StateManager::new(state.storage.session_dir(session_id))
        .write_work_graph(&graph)
        .unwrap();

    let waiting_b = post_task_worker(&app, "B").await;
    let waiting_status = waiting_b.status();
    let waiting_body: serde_json::Value = serde_json::from_slice(
        &to_bytes(waiting_b.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    assert_eq!(
        waiting_status,
        StatusCode::CONFLICT,
        "unexpected dependency wait response: {waiting_body}"
    );
    assert_eq!(waiting_body["reason"], "dependencies_pending");
    assert_eq!(controller.read().get_session(session_id).unwrap().agents.len(), 0);

    // C is ready and must not collide with B's dependency-pending queue intent even though
    // both requests initially reserve worker-1.
    let spawned_c = post_task_worker(&app, "C").await;
    assert_eq!(spawned_c.status(), StatusCode::CREATED);
    assert_eq!(controller.read().get_session(session_id).unwrap().agents.len(), 1);
    let after_c = state.queue_manager.queue_snapshot(session_id).unwrap();
    assert!(after_c
        .rows
        .iter()
        .find(|row| row.task_id.as_deref() == Some("B"))
        .unwrap()
        .worker_id
        .starts_with("pending:task:"));
    assert!(after_c
        .rows
        .iter()
        .find(|row| row.task_id.as_deref() == Some("C"))
        .unwrap()
        .worker_id
        .ends_with("worker-1"));

    state
        .queue_manager
        .enqueue_worker(
            "run-a",
            session_id,
            "prerequisite-a",
            "backend",
            "codex",
            json!({}),
            Some("A".to_string()),
        )
        .await
        .unwrap();
    assert!(matches!(
        state
            .queue_manager
            .claim_and_spawn("run-a", session_id, "prerequisite-a")
            .await
            .unwrap(),
        ClaimOutcome::Claimed { .. }
    ));
    state
        .queue_manager
        .record_heartbeat(session_id, "prerequisite-a", "completed")
        .await
        .unwrap();

    // Retrying the same task request now claims its preserved row, atomically rebinds it to
    // the roster's current worker-2 slot, and reaches the real controller/PTy spawn path.
    let spawned_b = post_task_worker(&app, "B").await;
    assert_eq!(spawned_b.status(), StatusCode::CREATED);
    let session = controller.read().get_session(session_id).unwrap();
    assert_eq!(session.agents.len(), 2);
    assert!(session.agents.iter().any(|agent| agent.id.ends_with("worker-2")));

    let snapshot = state.queue_manager.queue_snapshot(session_id).unwrap();
    let b = snapshot
        .rows
        .iter()
        .find(|row| row.task_id.as_deref() == Some("B"))
        .unwrap();
    let c = snapshot
        .rows
        .iter()
        .find(|row| row.task_id.as_deref() == Some("C"))
        .unwrap();
    assert!(b.worker_id.ends_with("worker-2"));
    assert!(c.worker_id.ends_with("worker-1"));
    assert_ne!(b.id, c.id, "stable task queue identities must not collide");
}

#[tokio::test]
async fn unreadable_authoritative_graph_fails_closed_before_enqueue() {
    let temp = tempfile::tempdir().unwrap();
    let (app, state, controller) = dependency_http_fixture(&temp);
    let session_id = "http-dependency-session";
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let session_dir = state.storage.create_session_dir(session_id).unwrap();
    controller
        .read()
        .insert_test_session(quiet_hive_session(session_id, &project));
    std::fs::write(session_dir.join("state").join("work-graph.json"), "not-json").unwrap();

    let response = post_task_worker(&app, "B").await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(state
        .queue_manager
        .queue_snapshot(session_id)
        .unwrap()
        .rows
        .is_empty());
}

fn queue_test_node(id: &str, status: NodeStatus) -> WorkNode {
    WorkNode::new(
        id,
        NodeKind::Task,
        id,
        NodeContract::default(),
        BindingRef::Role("worker".to_string()),
        status,
    )
}

fn quiet_hive_session(session_id: &str, project_path: &Path) -> Session {
    let mut session = quiet_fusion_session(session_id, project_path);
    session.session_type = SessionType::Hive { worker_count: 0 };
    session.last_activity_at = Utc::now();
    session
}

fn quiet_fusion_session(session_id: &str, project_path: &Path) -> Session {
    Session {
        id: session_id.to_string(),
        name: None,
        color: None,
        session_type: SessionType::Fusion {
            variants: vec!["alpha".to_string()],
        },
        project_path: project_path.to_path_buf(),
        state: SessionState::Running,
        created_at: Utc::now() - Duration::minutes(12),
        last_activity_at: Utc::now() - Duration::minutes(11),
        agents: Vec::new(),
        default_cli: "codex".to_string(),
        default_model: None,
        default_principal_cli: None,
        default_principal_model: None,
        default_principal_flags: Vec::new(),
        execution_policy: HiveExecutionPolicy::default(),
        qa_workers: Vec::new(),
        max_qa_iterations: 3,
        qa_timeout_secs: 300,
        auth_strategy: AuthStrategy::default(),
        worktree_path: None,
        worktree_branch: None,
        no_git: true,
        resume_report: None,
    }
}

#[test]
fn mark_session_completed_schedules_archive_after_persistence() {
    let temp = tempfile::tempdir().unwrap();
    let session_id = "completion-archive";
    let storage = Arc::new(
        SessionStorage::new_with_base(temp.path().join("storage")).unwrap(),
    );
    let session_dir = storage.create_session_dir(session_id).unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let mut controller = SessionController::new(Arc::new(RwLock::new(PtyManager::new())));
    controller.set_storage(Arc::clone(&storage));
    controller.insert_test_session(quiet_fusion_session(session_id, &project));
    controller
        .mark_session_completed(session_id)
        .expect("completion succeeds independently of archive scheduling");
    assert_eq!(
        controller.get_session(session_id).unwrap().state,
        SessionState::Completed
    );
    assert!(
        storage
            .load_session(session_id)
            .unwrap()
            .state
            .contains("Completed"),
        "session must be durable before the archive hook runs"
    );

    let archive_dir = session_dir.join("archive").join("work-graphs");
    let archive = (0..200)
        .find_map(|_| {
            let found = std::fs::read_dir(&archive_dir)
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"));
            if found.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            found
        })
        .expect("completion schedules the idempotent work-graph archive");
    assert!(archive.is_file());
}
