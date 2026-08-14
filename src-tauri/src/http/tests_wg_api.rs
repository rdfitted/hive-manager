//! Full-stack read-surface tests for work-graph observability (#227) and PTY evidence (#226).

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use parking_lot::RwLock;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use crate::coordination::{InjectionManager, QueueManager, StateManager};
use crate::domain::HiveExecutionPolicy;
use crate::events::EventBus;
use crate::http::routes::create_router;
use crate::http::state::AppState;
use crate::orchestrator::work_graph::archive::archive_completed_session;
use crate::orchestrator::work_graph::runtime::{
    record_graph_change, GraphMutationType,
};
use crate::orchestrator::work_graph::{
    BindingRef, EdgeKind, EdgeProvenance, NodeContract, NodeKind, NodeStatus, TaskGraph,
    WorkEdge, WorkNode,
};
use crate::pty::{AgentConfig, AgentRole, AgentStatus, PtyManager};
use crate::session::{
    AgentInfo, AuthStrategy, Session, SessionController, SessionState, SessionType,
};
use crate::storage::queue::{QueueRow, QueueStatus};
use crate::storage::{ApplicationStateDb, QueueRepo, SessionStorage};

struct TestApp {
    _storage_dir: TempDir,
    router: axum::Router,
    state: Arc<AppState>,
}

impl TestApp {
    fn storage(&self) -> &SessionStorage {
        &self.state.storage
    }
}

async fn test_app() -> TestApp {
    let storage_dir = TempDir::new().expect("temporary storage");
    let storage = Arc::new(
        SessionStorage::new_with_base(storage_dir.path().to_path_buf())
            .expect("session storage"),
    );
    let config = Arc::new(tokio::sync::RwLock::new(
        storage.load_config().expect("test config"),
    ));
    let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
    let session_controller = Arc::new(RwLock::new(SessionController::new(pty_manager.clone())));
    session_controller.write().set_storage(storage.clone());
    let injection_manager = Arc::new(RwLock::new(InjectionManager::new(
        pty_manager.clone(),
        SessionStorage::new_with_base(storage_dir.path().to_path_buf())
            .expect("injection storage"),
    )));
    let event_bus = EventBus::new(storage.base_dir().clone());
    let app_state_db = Arc::new(
        ApplicationStateDb::open(storage.base_dir()).expect("application state database"),
    );
    let queue_repo = Arc::new(QueueRepo::new(app_state_db.clone()));
    queue_repo.ensure_schema().expect("queue schema");
    let queue_manager = Arc::new(QueueManager::new(queue_repo, event_bus.clone()));
    let state = Arc::new(AppState::new(
        config,
        pty_manager,
        session_controller,
        injection_manager,
        storage,
        event_bus,
        app_state_db,
        queue_manager,
        None,
    ));
    state.set_registry(Arc::new(crate::actions::build_registry()));

    TestApp {
        _storage_dir: storage_dir,
        router: create_router(state.clone()),
        state,
    }
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, String, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response bytes");
    let body = String::from_utf8(bytes.to_vec()).expect("UTF-8 response");
    let json = serde_json::from_str(&body).unwrap_or(Value::Null);
    (status, body, json)
}

fn portable_graph_payload(html: &str) -> Value {
    const OPEN: &str = "<script id=\"graph-data\" type=\"application/json\">";
    const CLOSE: &str = "</script>";
    let start = html.find(OPEN).expect("embedded graph payload") + OPEN.len();
    let end = html[start..]
        .find(CLOSE)
        .map(|offset| start + offset)
        .expect("embedded graph payload terminator");
    serde_json::from_str(&html[start..end]).expect("valid embedded graph JSON")
}

fn node(
    id: &str,
    lane: BindingRef,
    status: NodeStatus,
    contract_marker: &str,
) -> WorkNode {
    WorkNode::new(
        id,
        NodeKind::Task,
        format!("task-body-title-{contract_marker}"),
        NodeContract {
            inputs: vec![format!("task-body-input-{contract_marker}")],
            outputs: vec![format!("task-body-output-{contract_marker}")],
            acceptance: vec![format!("task-body-acceptance-{contract_marker}")],
        },
        lane,
        status,
    )
}

fn queue_row(
    session_id: &str,
    id: &str,
    task_id: &str,
    status: QueueStatus,
    created_at: i64,
) -> QueueRow {
    QueueRow {
        id: id.to_string(),
        task_id: Some(task_id.to_string()),
        session_id: session_id.to_string(),
        worker_id: format!("worker-{task_id}"),
        role_type: "backend".to_string(),
        cli: "codex".to_string(),
        status,
        payload: json!({"task_id": task_id}),
        attempts: 1,
        continuation_count: 0,
        no_progress_count: 0,
        last_status: None,
        heartbeat_at: Some(created_at),
        assignment_id: created_at,
        blocked_reason: (status == QueueStatus::Blocked)
            .then(|| format!("blocked subtree at {task_id}")),
        created_at,
        updated_at: created_at,
    }
}

fn running_session_with_agent(session_id: &str, agent_id: &str) -> Session {
    let now = Utc::now();
    Session {
        id: session_id.to_string(),
        name: None,
        color: None,
        session_type: SessionType::Hive { worker_count: 0 },
        project_path: std::path::PathBuf::from("test-project"),
        state: SessionState::Running,
        created_at: now,
        last_activity_at: now,
        agents: vec![AgentInfo {
            id: agent_id.to_string(),
            role: AgentRole::Queen,
            status: AgentStatus::Running,
            config: AgentConfig::default(),
            parent_id: None,
            commit_sha: None,
            base_commit_sha: None,
            role_definition_id: None,
            role_definition_version: None,
        }],
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

#[tokio::test]
async fn plan_ready_graph_is_bounded_and_preserves_all_provenance_classes() {
    const SESSION_ID: &str = "wg-api-plan-ready";
    const SECRET: &str = "SECRET-FULL-TASK-BODY-MUST-NOT-CROSS-THE-WIRE";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let graph = TaskGraph::new(
        vec![
            node(
                "a",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Ready,
                SECRET,
            ),
            node(
                "b",
                BindingRef::Role("frontend".to_string()),
                NodeStatus::Pending,
                SECRET,
            ),
            node(
                "c",
                BindingRef::Zone("integration".to_string()),
                NodeStatus::Pending,
                SECRET,
            ),
            node(
                "d",
                BindingRef::Zone("docs".to_string()),
                NodeStatus::Ready,
                SECRET,
            ),
            node(
                "e",
                BindingRef::Role("reviewer".to_string()),
                NodeStatus::Pending,
                SECRET,
            ),
        ],
        vec![
            WorkEdge::new("a", "b", EdgeKind::DependsOn, EdgeProvenance::Planner),
            WorkEdge::new("b", "c", EdgeKind::DependsOn, EdgeProvenance::Codegraph),
            WorkEdge::new("a", "d", EdgeKind::Informs, EdgeProvenance::Knowledge),
            WorkEdge::new("d", "e", EdgeKind::DependsOn, EdgeProvenance::Runtime),
        ],
    );
    StateManager::new(session_dir)
        .write_work_graph(&graph)
        .expect("persisted plan graph");

    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=plan"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(response["source"], "live");
    assert_eq!(response["view"], "plan");
    assert_eq!(response["waves"], json!([["a", "d"], ["b", "e"], ["c"]]));
    assert_eq!(response["critical_path"], json!(["a", "b", "c"]));
    assert_eq!(
        response["lane_assignment"]["a"],
        json!({"kind": "role", "value": "backend"})
    );

    let provenance: BTreeSet<&str> = response["provenance_by_edge"]
        .as_array()
        .expect("provenance list")
        .iter()
        .filter_map(|edge| edge["provenance"].as_str())
        .collect();
    assert_eq!(
        provenance,
        BTreeSet::from(["planner", "codegraph", "knowledge", "runtime"]),
        "the four trust classes must remain distinct over HTTP"
    );

    let first_node = &response["nodes"][0];
    assert!(first_node.get("title").is_none());
    assert!(first_node.get("contract").is_none());
    assert_eq!(
        first_node["contract_summary"],
        json!({"input_count": 1, "output_count": 1, "acceptance_count": 1})
    );
    assert!(
        !body.contains(SECRET) && !body.contains("task-body-"),
        "bounded node projection leaked a full task body: {body}"
    );
}

#[tokio::test]
async fn runtime_graph_projects_a_blocked_subtree_from_the_durable_queue() {
    const SESSION_ID: &str = "wg-api-mid-flight";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let graph = TaskGraph::new(
        vec![
            node(
                "root",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Ready,
                "root",
            ),
            node(
                "blocked-child",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Pending,
                "child",
            ),
            node(
                "blocked-leaf",
                BindingRef::Role("reviewer".to_string()),
                NodeStatus::Pending,
                "leaf",
            ),
        ],
        vec![
            WorkEdge::new(
                "root",
                "blocked-child",
                EdgeKind::DependsOn,
                EdgeProvenance::Planner,
            ),
            WorkEdge::new(
                "blocked-child",
                "blocked-leaf",
                EdgeKind::DependsOn,
                EdgeProvenance::Runtime,
            ),
        ],
    );
    StateManager::new(session_dir)
        .write_work_graph(&graph)
        .expect("persisted live graph");
    let repo = app.state.queue_manager.repo();
    repo.enqueue(&queue_row(
        SESSION_ID,
        "run-root",
        "root",
        QueueStatus::Running,
        1,
    ))
    .expect("running queue row");
    repo.enqueue(&queue_row(
        SESSION_ID,
        "run-child",
        "blocked-child",
        QueueStatus::Blocked,
        2,
    ))
    .expect("blocked child queue row");
    repo.enqueue(&queue_row(
        SESSION_ID,
        "run-leaf",
        "blocked-leaf",
        QueueStatus::Blocked,
        3,
    ))
    .expect("blocked leaf queue row");

    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=runtime"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(response["source"], "live");
    assert_eq!(response["status_by_node"]["root"], "running");
    assert_eq!(response["status_by_node"]["blocked-child"], "blocked");
    assert_eq!(response["status_by_node"]["blocked-leaf"], "blocked");
    assert_eq!(response["critical_path"], json!(["root", "blocked-child", "blocked-leaf"]));
    assert_eq!(
        response["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .filter(|node| node["status"] == "blocked")
            .count(),
        2,
        "the dependent branch must remain visibly blocked"
    );
}

#[tokio::test]
async fn completed_session_falls_back_to_archive_with_divergence() {
    const SESSION_ID: &str = "wg-api-archived";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let plan = TaskGraph::new(
        vec![node(
            "planned",
            BindingRef::Role("backend".to_string()),
            NodeStatus::Ready,
            "planned",
        )],
        vec![],
    );
    let mut runtime = plan.clone();
    runtime.nodes.push(node(
        "runtime-review",
        BindingRef::Role("reviewer".to_string()),
        NodeStatus::Completed,
        "runtime",
    ));
    runtime.edges.push(WorkEdge::new(
        "planned",
        "runtime-review",
        EdgeKind::Reviews,
        EdgeProvenance::Runtime,
    ));
    StateManager::new(session_dir)
        .write_work_graph(&plan)
        .expect("persisted plan graph");
    record_graph_change(
        SESSION_ID,
        GraphMutationType::Other,
        &plan,
        &runtime,
        vec!["integration-test".to_string()],
    )
    .expect("recorded runtime mutation")
    .expect("non-empty runtime mutation");
    let completion = archive_completed_session(app.storage().base_dir(), None, SESSION_ID)
        .expect("completed archive");
    assert!(completion.created);

    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=divergence"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(response["source"], "archive");
    assert_eq!(response["view"], "divergence");
    assert_eq!(
        response["divergence"]["counts_by_mutation_type"]["node_added"],
        1
    );
    assert_eq!(
        response["divergence"]["recorded_runtime_mutations"]["other"],
        1
    );
    assert!(response["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .any(|node| node["id"] == "runtime-review"));
}

#[tokio::test]
async fn pty_buffer_exposes_unsubmitted_stub_content_and_rejects_traversal_ids() {
    const SESSION_ID: &str = "wg-api-pty";
    const AGENT_ID: &str = "wg-api-pty-queen";
    const PAYLOAD: &str = "UNSUBMITTED_SENTINEL_WITHOUT_ENTER";
    let app = test_app().await;
    app.storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    app.state
        .session_controller
        .read()
        .insert_test_session(running_session_with_agent(SESSION_ID, AGENT_ID));
    app.state
        .pty_manager
        .write()
        .create_session(
            AGENT_ID.to_string(),
            AgentRole::Queen,
            "cmd",
            &[],
            None,
            80,
            24,
        )
        .expect("stub PTY");
    app.state
        .pty_manager
        .read()
        .write(AGENT_ID, PAYLOAD.as_bytes())
        .expect("unsubmitted PTY write");

    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/agents/{AGENT_ID}/pty-buffer"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(response["session_id"], SESSION_ID);
    assert_eq!(response["agent_id"], AGENT_ID);
    assert_eq!(response["output"], PAYLOAD);
    assert_eq!(response["byte_count"], PAYLOAD.len());
    assert!(
        !response["output"].as_str().unwrap().contains('\r'),
        "the fixture must observe composer content before Enter is submitted"
    );

    let (bad_session_status, _, _) = get(
        &app.router,
        &format!("/api/sessions/bad..session/agents/{AGENT_ID}/pty-buffer"),
    )
    .await;
    assert_eq!(bad_session_status, StatusCode::BAD_REQUEST);

    let (bad_agent_status, _, _) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/agents/bad..agent/pty-buffer"),
    )
    .await;
    assert_eq!(bad_agent_status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn every_session_lifecycle_stage_emits_a_retrievable_graph() {
    const SESSION_ID: &str = "wg-api-all-lifecycle-stages";
    const AGENT_ID: &str = "wg-api-lifecycle-queen";
    const SECRET: &str = "LIFECYCLE-TASK-BODY-MUST-STAY-BOUNDED";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    StateManager::new(session_dir.clone())
        .write_work_graph(&TaskGraph::new(
            vec![node(
                "lifecycle-task",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Running,
                SECRET,
            )],
            vec![],
        ))
        .expect("persisted lifecycle graph");
    app.state
        .session_controller
        .read()
        .insert_test_session(running_session_with_agent(SESSION_ID, AGENT_ID));

    let lifecycle_states = [
        SessionState::Planning,
        SessionState::PlanReady,
        SessionState::Starting,
        SessionState::SpawningWorker(3),
        SessionState::WaitingForWorker(3),
        SessionState::SpawningPlanner(2),
        SessionState::WaitingForPlanner(2),
        SessionState::SpawningFusionVariant(1),
        SessionState::WaitingForFusionVariants,
        SessionState::SpawningDebateRound(1),
        SessionState::WaitingForDebateRound(1),
        SessionState::SpawningJudge,
        SessionState::Judging,
        SessionState::AwaitingVerdictSelection,
        SessionState::MergingWinner,
        SessionState::SpawningEvaluator,
        SessionState::QaInProgress { iteration: None },
        SessionState::QaPassed,
        SessionState::QaFailed { iteration: 1 },
        SessionState::QaMaxRetriesExceeded,
        SessionState::PrinceRemediation,
        SessionState::QaInconclusive,
        SessionState::Running,
        SessionState::Paused,
        SessionState::Completed,
        SessionState::Closing,
        SessionState::Closed,
        SessionState::Failed("simulated crash".to_string()),
    ];
    assert_eq!(lifecycle_states.len(), 28);

    for lifecycle_state in lifecycle_states {
        let expected_stage = format!("{:?}", lifecycle_state.kind());
        app.state
            .session_controller
            .read()
            .transition_test_session(SESSION_ID, lifecycle_state)
            .expect("transition through production lifecycle seam");

        let snapshot = StateManager::new(session_dir.clone())
            .read_work_graph_snapshot()
            .expect("snapshot read")
            .expect("transition emitted snapshot");
        assert_eq!(snapshot.lifecycle_stage, expected_stage);
        assert_eq!(snapshot.node_count, 1);

        let (status, body, response) = get(
            &app.router,
            &format!("/api/sessions/{SESSION_ID}/work-graph?view=runtime"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "stage {expected_stage}: {body}");
        assert_eq!(response["source"], "live");
        assert_eq!(response["nodes"][0]["id"], "lifecycle-task");
        assert!(
            !body.contains(SECRET),
            "stage {expected_stage} leaked the task body"
        );
    }

    let artifact = session_dir.join("work-graph.html");
    let html = std::fs::read_to_string(artifact).expect("crash-stage portable artifact");
    assert!(html.contains("\"lifecycle_stage\":\"Failed\""));
    assert!(html.contains("lifecycle-task"));
    assert!(!html.contains(SECRET));
}

#[tokio::test]
async fn completion_persistence_rollback_refreshes_the_graph_artifact() {
    const SESSION_ID: &str = "wg-api-completion-rollback";
    const AGENT_ID: &str = "wg-api-rollback-queen";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    StateManager::new(session_dir.clone())
        .write_work_graph(&TaskGraph::new(
            vec![node(
                "rollback-task",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Running,
                "rollback",
            )],
            vec![],
        ))
        .expect("persisted rollback graph");
    let mut session = running_session_with_agent(SESSION_ID, AGENT_ID);
    session.last_activity_at = Utc::now() - Duration::minutes(11);
    app.state
        .session_controller
        .read()
        .insert_test_session(session);
    app.state
        .session_controller
        .read()
        .transition_test_session(SESSION_ID, SessionState::Running)
        .expect("baseline lifecycle snapshot");

    // A directory at the metadata file path deterministically rejects the atomic
    // session.json replace while leaving state/work-graph.json writable.
    std::fs::create_dir(session_dir.join("session.json"))
        .expect("session metadata failure fixture");
    app.state
        .session_controller
        .read()
        .mark_session_completed(SESSION_ID)
        .expect_err("completion persistence must fail and roll back");

    let restored = app
        .state
        .session_controller
        .read()
        .get_session(SESSION_ID)
        .expect("restored session");
    assert_eq!(restored.state, SessionState::Running);
    let snapshot = StateManager::new(session_dir.clone())
        .read_work_graph_snapshot()
        .expect("snapshot read")
        .expect("rollback refreshed snapshot");
    assert_eq!(snapshot.lifecycle_stage, "Running");
    let html = std::fs::read_to_string(session_dir.join("work-graph.html"))
        .expect("rollback artifact");
    assert_eq!(portable_graph_payload(&html)["lifecycle_stage"], "Running");
}

#[test]
fn every_production_state_rollback_refreshes_lifecycle_evidence() {
    let source = include_str!("../session/controller.rs").replace("\r\n", "\n");
    let production = source
        .split("\n#[cfg(test)]\nmod tests {")
        .next()
        .expect("production controller source");
    let forbidden_direct_mutations = [
        ("completion rollback", "session.state = previous_session_state;"),
        ("QA-timeout forward", "session.state = SessionState::QaInconclusive;"),
        ("QA-timeout rollback", "session.state = previous_state;"),
    ];
    for (label, needle) in forbidden_direct_mutations {
        assert!(
            !production.contains(needle),
            "{label} bypasses the snapshot-emitting transition helper"
        );
    }

    let whole_session_rollbacks = production
        .match_indices("*session = previous_session;")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(
        whole_session_rollbacks.len(),
        4,
        "whole-session rollback audit changed; inspect every restore before updating this gate"
    );
    for index in whole_session_rollbacks {
        let end = production.len().min(index + 900);
        assert!(
            production[index..end].contains("emit_work_graph_snapshot"),
            "whole-session rollback at byte {index} restores state without refreshing graph evidence"
        );
    }
}

#[tokio::test]
async fn simulated_crash_after_plan_ready_leaves_a_bounded_standalone_graph() {
    const SESSION_ID: &str = "wg-api-crash-artifact";
    const AGENT_ID: &str = "wg-api-crash-queen";
    const SECRET: &str = "CRASH-TASK-BODY-MUST-STAY-IN-AUTHORITATIVE-STATE";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let manager = StateManager::new(session_dir.clone());
    manager
        .write_work_graph(&TaskGraph::new(
            vec![node(
                "crashed-task",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Running,
                SECRET,
            )],
            vec![],
        ))
        .expect("persisted graph");
    app.state
        .session_controller
        .read()
        .insert_test_session(running_session_with_agent(SESSION_ID, AGENT_ID));

    // The process is considered to disappear immediately after this ordinary
    // lifecycle transition: no archival or read endpoint is invoked to create evidence.
    app.state
        .session_controller
        .read()
        .transition_test_session(SESSION_ID, SessionState::PlanReady)
        .expect("production PlanReady transition");
    let artifact = session_dir.join("work-graph.html");
    let snapshot = manager
        .read_work_graph_snapshot()
        .expect("snapshot read")
        .expect("snapshot exists");
    let html = std::fs::read_to_string(&artifact).expect("portable graph HTML");
    let payload = portable_graph_payload(&html);

    assert_eq!(artifact, session_dir.join("work-graph.html"));
    assert_eq!(snapshot.lifecycle_stage, "PlanReady");
    assert_eq!(snapshot.node_count, 1);
    assert_eq!(snapshot.artifact, "work-graph.html");
    assert!(html.starts_with("<!doctype html>"));
    assert_eq!(payload["lifecycle_stage"], "PlanReady");
    assert_eq!(payload["nodes"][0]["id"], "crashed-task");
    assert!(payload["nodes"][0].get("title").is_none());
    assert!(!html.contains(SECRET));
    assert!(!html.contains("http://") && !html.contains("https://"));
}

#[test]
fn clean_archival_writes_and_backfills_the_portable_graph() {
    const SESSION_ID: &str = "wg-api-clean-artifact";
    let temp = TempDir::new().expect("temporary storage");
    let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).expect("storage");
    let session_dir = storage
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    StateManager::new(session_dir.clone())
        .write_work_graph(&TaskGraph::new(
            vec![node(
                "completed-task",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Completed,
                "clean-archive",
            )],
            vec![],
        ))
        .expect("persisted graph");

    let completion = archive_completed_session(temp.path(), None, SESSION_ID)
        .expect("clean archival");
    let artifact = session_dir.join("work-graph.html");
    assert!(completion.created);
    assert!(artifact.is_file());
    let html = std::fs::read_to_string(&artifact).expect("portable graph HTML");
    let payload = portable_graph_payload(&html);
    assert_eq!(payload["lifecycle_stage"], "archived");
    assert_eq!(payload["nodes"][0]["id"], "completed-task");
    assert!(payload["divergence"].is_object());

    std::fs::remove_file(&artifact).expect("remove artifact to exercise upgrade backfill");
    let repeated = archive_completed_session(temp.path(), None, SESSION_ID)
        .expect("idempotent archival backfills HTML");
    assert!(!repeated.created);
    assert_eq!(repeated.path, completion.path);
    assert!(
        artifact.is_file(),
        "an existing JSON archive must not skip the portable artifact"
    );
}
