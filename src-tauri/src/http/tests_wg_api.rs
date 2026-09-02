//! Full-stack read-surface tests for work-graph observability (#227) and PTY evidence (#226).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Duration, Utc};
use parking_lot::RwLock;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use crate::coordination::{HierarchyNode, InjectionManager, QueueManager, StateManager};
use crate::domain::HiveExecutionPolicy;
use crate::events::EventBus;
use crate::http::routes::create_router;
use crate::http::state::AppState;
use crate::orchestrator::work_graph::archive::{
    archive_completed_session, WorkGraphArchive, WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
};
use crate::orchestrator::work_graph::divergence::DivergenceSummary;
use crate::orchestrator::work_graph::runtime::{
    record_graph_change, CompletionEvidenceClass, GraphMutationType, RuntimeOutcome,
    RuntimeOutcomeStatus,
};
use crate::orchestrator::work_graph::{
    BindingRef, CompositeExpansion, EdgeKind, EdgeProvenance, NodeContract, NodeKind, NodeStatus,
    TaskGraph, WorkEdge, WorkGraphOmission, WorkGraphOmissionReason, WorkNode,
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
        SessionStorage::new_with_base(storage_dir.path().to_path_buf()).expect("session storage"),
    );
    let config = Arc::new(tokio::sync::RwLock::new(
        storage.load_config().expect("test config"),
    ));
    let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
    let session_controller = Arc::new(RwLock::new(SessionController::new(pty_manager.clone())));
    session_controller.write().set_storage(storage.clone());
    let injection_manager = Arc::new(RwLock::new(InjectionManager::new(
        pty_manager.clone(),
        SessionStorage::new_with_base(storage_dir.path().to_path_buf()).expect("injection storage"),
    )));
    let event_bus = EventBus::new(storage.base_dir().clone());
    let app_state_db =
        Arc::new(ApplicationStateDb::open(storage.base_dir()).expect("application state database"));
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

fn node(id: &str, lane: BindingRef, status: NodeStatus, contract_marker: &str) -> WorkNode {
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
async fn plan_ready_graph_preserves_node_contract_and_all_provenance_classes() {
    const SESSION_ID: &str = "wg-api-plan-ready";
    const CONTRACT_MARKER: &str = "contract-text-must-cross-the-wire";
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
                CONTRACT_MARKER,
            ),
            node(
                "b",
                BindingRef::Role("frontend".to_string()),
                NodeStatus::Pending,
                CONTRACT_MARKER,
            ),
            node(
                "c",
                BindingRef::Zone("integration".to_string()),
                NodeStatus::Pending,
                CONTRACT_MARKER,
            ),
            node(
                "d",
                BindingRef::Zone("docs".to_string()),
                NodeStatus::Ready,
                CONTRACT_MARKER,
            ),
            node(
                "e",
                BindingRef::Role("reviewer".to_string()),
                NodeStatus::Pending,
                CONTRACT_MARKER,
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
    assert_eq!(
        first_node["title"],
        format!("task-body-title-{CONTRACT_MARKER}")
    );
    assert_eq!(first_node["kind"], "task");
    assert_eq!(
        first_node["contract"],
        json!({
            "inputs": [format!("task-body-input-{CONTRACT_MARKER}")],
            "outputs": [format!("task-body-output-{CONTRACT_MARKER}")],
            "acceptance": [format!("task-body-acceptance-{CONTRACT_MARKER}")],
        })
    );
    assert_eq!(first_node["expansion"], Value::Null);
    assert_eq!(
        first_node["contract_summary"],
        json!({"input_count": 1, "output_count": 1, "acceptance_count": 1})
    );
    assert!(
        body.contains(CONTRACT_MARKER),
        "the full node contract must be observable over HTTP: {body}"
    );
}

#[tokio::test]
async fn node_payload_shape_is_identical_across_live_views_and_archive() {
    const SESSION_ID: &str = "wg-api-node-shape";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let mut expanded = node(
        "expanded-task",
        BindingRef::Role("backend".to_string()),
        NodeStatus::Ready,
        "expanded",
    );
    expanded.expansion = Some(CompositeExpansion {
        template: "review-template".to_string(),
        parameters: BTreeMap::from([("target".to_string(), "expanded-task".to_string())]),
    });
    let graph = TaskGraph::new(vec![expanded], vec![]);
    StateManager::new(session_dir)
        .write_work_graph(&graph)
        .expect("persisted plan graph");

    let mut live_nodes = Vec::new();
    for view in ["plan", "runtime", "divergence"] {
        let (status, body, response) = get(
            &app.router,
            &format!("/api/sessions/{SESSION_ID}/work-graph?view={view}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(response["source"], "live");
        let payload = response["nodes"][0].clone();
        assert_eq!(payload["title"], "task-body-title-expanded");
        assert_eq!(payload["kind"], "task");
        assert_eq!(
            payload["contract"],
            json!({
                "inputs": ["task-body-input-expanded"],
                "outputs": ["task-body-output-expanded"],
                "acceptance": ["task-body-acceptance-expanded"],
            })
        );
        assert_eq!(
            payload["contract_summary"],
            json!({"input_count": 1, "output_count": 1, "acceptance_count": 1})
        );
        assert_eq!(
            payload["expansion"],
            json!({
                "template": "review-template",
                "parameters": {"target": "expanded-task"},
            })
        );
        live_nodes.push(payload);
    }

    assert_eq!(live_nodes[0], live_nodes[1]);
    assert_eq!(live_nodes[1], live_nodes[2]);

    archive_completed_session(app.storage().base_dir(), None, SESSION_ID)
        .expect("completed archive");
    let (status, body, archived) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=plan&source=archive"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(archived["source"], "archive");
    assert_eq!(archived["nodes"][0], live_nodes[0]);
}

#[tokio::test]
async fn source_selectors_choose_live_archive_and_auto_tracks_session_lifecycle() {
    const SESSION_ID: &str = "wg-api-source-selector";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let archived_graph = TaskGraph::new(
        vec![node(
            "archived-node",
            BindingRef::Role("backend".to_string()),
            NodeStatus::Ready,
            "archived",
        )],
        vec![],
    );
    let state = StateManager::new(session_dir.clone());
    state
        .write_work_graph(&archived_graph)
        .expect("persisted archive source graph");
    archive_completed_session(app.storage().base_dir(), None, SESSION_ID)
        .expect("completed archive");

    let live_graph = TaskGraph::new(
        vec![node(
            "live-node",
            BindingRef::Role("frontend".to_string()),
            NodeStatus::Ready,
            "live",
        )],
        vec![],
    );
    state
        .write_work_graph(&live_graph)
        .expect("persisted newer live graph");
    app.state
        .session_controller
        .read()
        .insert_test_session(running_session_with_agent(SESSION_ID, "selector-queen"));

    for (selector, expected_source, expected_node) in [
        ("source=live", "live", "live-node"),
        ("source=archive", "archive", "archived-node"),
        ("source=auto", "live", "live-node"),
        ("", "live", "live-node"),
    ] {
        let separator = if selector.is_empty() { "" } else { "&" };
        let (status, body, response) = get(
            &app.router,
            &format!("/api/sessions/{SESSION_ID}/work-graph?view=plan{separator}{selector}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{selector}: {body}");
        assert_eq!(response["source"], expected_source, "{selector}: {body}");
        assert_eq!(
            response["nodes"][0]["id"], expected_node,
            "{selector}: {body}"
        );
    }

    let mut terminal = app
        .state
        .session_controller
        .read()
        .get_session(SESSION_ID)
        .expect("running selector session");
    terminal.state = SessionState::Completed;
    app.state
        .session_controller
        .read()
        .insert_test_session(terminal);
    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=plan&source=auto"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(response["source"], "archive");
    assert_eq!(response["nodes"][0]["id"], "archived-node");

    std::fs::remove_file(session_dir.join("state").join("work-graph.json"))
        .expect("remove only the temporary live graph fixture");
    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=plan&source=auto"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(response["source"], "archive");
    assert_eq!(response["nodes"][0]["id"], "archived-node");
}

#[tokio::test]
async fn terminal_session_without_archive_serves_live_with_typed_omission() {
    const SESSION_ID: &str = "wg-api-terminal-missing-archive";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    StateManager::new(session_dir)
        .write_work_graph(&TaskGraph::new(
            vec![node(
                "live-terminal-node",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Completed,
                "live-terminal",
            )],
            vec![],
        ))
        .expect("persisted live terminal graph");
    let mut session = running_session_with_agent(SESSION_ID, "terminal-queen");
    session.state = SessionState::Failed("expected test failure".to_string());
    app.state
        .session_controller
        .read()
        .insert_test_session(session);

    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=plan&source=auto"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(response["source"], "live");
    assert_eq!(response["nodes"][0]["id"], "live-terminal-node");
    assert!(response["omissions"]
        .as_array()
        .expect("typed omissions")
        .iter()
        .any(|omission| {
            omission["reason"] == "source_unreadable"
                && omission["examples"] == json!(["archive:missing"])
        }));
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
    assert_eq!(
        response["critical_path"],
        json!(["root", "blocked-child", "blocked-leaf"])
    );
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
async fn live_view_preserves_unbacked_terminal_status_and_reports_provenance() {
    const SESSION_ID: &str = "wg-api-view-provenance";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let graph = TaskGraph::new(
        vec![
            node(
                "plan-completed",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Completed,
                "plan-completed",
            ),
            node(
                "queue-completed",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Pending,
                "queue-completed",
            ),
            node(
                "dependent",
                BindingRef::Role("reviewer".to_string()),
                NodeStatus::Pending,
                "dependent",
            ),
        ],
        vec![WorkEdge::new(
            "plan-completed",
            "dependent",
            EdgeKind::DependsOn,
            EdgeProvenance::Planner,
        )],
    );
    let state_manager = StateManager::new(session_dir);
    state_manager
        .write_work_graph(&graph)
        .expect("persisted live graph");
    state_manager
        .update_hierarchy(&[
            HierarchyNode {
                id: "worker-queue-completed".to_string(),
                role: "Worker-1".to_string(),
                principal: Some("backend".to_string()),
                parent_id: Some("view-queen".to_string()),
                children: Vec::new(),
            },
            HierarchyNode {
                id: "view-queen".to_string(),
                role: "Queen".to_string(),
                principal: None,
                parent_id: None,
                children: vec!["worker-queue-completed".to_string()],
            },
        ])
        .expect("persisted observed principal mapping");
    app.state
        .queue_manager
        .repo()
        .enqueue(&queue_row(
            SESSION_ID,
            "run-queue-completed",
            "queue-completed",
            QueueStatus::Finalized,
            1,
        ))
        .expect("finalized queue evidence");

    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=runtime&source=live"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(response["status_by_node"]["plan-completed"], "completed");
    assert_eq!(response["status_by_node"]["queue-completed"], "completed");
    assert_eq!(
        response["status_by_node"]["dependent"], "pending",
        "the view projection must not feed persisted completion into readiness promotion"
    );
    assert_eq!(response["completion_provenance"]["plan-completed"], "plan");
    assert_eq!(response["completion_provenance"]["queue-completed"], "queue");
    assert_eq!(
        response["lane_assignment"]["queue-completed"],
        json!({"kind":"role","value":"worker-queue-completed"})
    );
    assert_eq!(
        response["agents_by_lane"]["backend"],
        json!(["worker-queue-completed"])
    );
    assert!(response["omissions"]
        .as_array()
        .expect("typed omissions")
        .iter()
        .any(|omission| {
            omission["reason"] == "resolution_incomplete"
                && omission["examples"]
                    .as_array()
                    .is_some_and(|examples| examples.iter().any(|example| example == "queue:plan-completed"))
        }));
}

#[tokio::test]
async fn runtime_and_divergence_progress_preserve_queue_evidence_and_null_timing() {
    const SESSION_ID: &str = "wg-api-live-progress";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let graph = TaskGraph::new(
        vec![
            node(
                "frozen",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Ready,
                "frozen",
            ),
            node(
                "healthy",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Ready,
                "healthy",
            ),
        ],
        vec![],
    );
    StateManager::new(session_dir)
        .write_work_graph(&graph)
        .expect("persisted live graph");

    let mut frozen = queue_row(
        SESSION_ID,
        "run-frozen",
        "frozen",
        QueueStatus::Running,
        1_000,
    );
    frozen.worker_id = "agent-frozen".to_string();
    frozen.attempts = 3;
    frozen.heartbeat_at = Some(1_000);
    let mut healthy = queue_row(
        SESSION_ID,
        "run-healthy",
        "healthy",
        QueueStatus::Running,
        9_000,
    );
    healthy.worker_id = "agent-healthy".to_string();
    healthy.attempts = 1;
    healthy.heartbeat_at = Some(9_000);
    let repo = app.state.queue_manager.repo();
    repo.enqueue(&frozen).expect("frozen queue row");
    repo.enqueue(&healthy).expect("healthy queue row");

    let (status, body, plan) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=plan&source=live"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(plan["nodes"]
        .as_array()
        .expect("plan nodes")
        .iter()
        .all(|node| node.get("progress").is_none()));

    for view in ["runtime", "divergence"] {
        let (status, body, response) = get(
            &app.router,
            &format!("/api/sessions/{SESSION_ID}/work-graph?view={view}&source=live"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let nodes = response["nodes"].as_array().expect("runtime nodes");
        let frozen_progress = &nodes
            .iter()
            .find(|node| node["id"] == "frozen")
            .expect("frozen node")["progress"];
        let healthy_progress = &nodes
            .iter()
            .find(|node| node["id"] == "healthy")
            .expect("healthy node")["progress"];
        assert_eq!(frozen_progress["started_at"], Value::Null);
        assert_eq!(frozen_progress["finished_at"], Value::Null);
        assert_eq!(frozen_progress["attempts"], 3);
        assert_eq!(frozen_progress["agent_id"], "agent-frozen");
        assert_eq!(
            frozen_progress["last_heartbeat_at"],
            json!(DateTime::<Utc>::from_timestamp_millis(1_000).expect("valid timestamp"))
        );
        assert_eq!(healthy_progress["attempts"], 1);
        assert_eq!(healthy_progress["agent_id"], "agent-healthy");
        assert_eq!(
            healthy_progress["last_heartbeat_at"],
            json!(DateTime::<Utc>::from_timestamp_millis(9_000).expect("valid timestamp"))
        );
        assert_ne!(
            frozen_progress["last_heartbeat_at"], healthy_progress["last_heartbeat_at"],
            "a frozen heartbeat must remain distinguishable from recent progress"
        );
    }
}

#[tokio::test]
async fn archived_runtime_progress_uses_the_same_nullable_object_shape() {
    const SESSION_ID: &str = "wg-api-archive-progress";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let plan = TaskGraph::new(
        vec![node(
            "archived-task",
            BindingRef::Role("backend".to_string()),
            NodeStatus::Ready,
            "archived-progress",
        )],
        vec![],
    );
    let mut runtime = plan.clone();
    runtime.nodes[0].status = NodeStatus::Completed;
    StateManager::new(session_dir)
        .write_work_graph(&plan)
        .expect("persisted archive plan");
    record_graph_change(
        SESSION_ID,
        GraphMutationType::Other,
        &plan,
        &runtime,
        vec!["archive-progress-test".to_string()],
    )
    .expect("recorded status mutation")
    .expect("non-empty status mutation");
    archive_completed_session(app.storage().base_dir(), None, SESSION_ID)
        .expect("completed archive");

    let (status, body, plan_response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=plan&source=archive"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(plan_response["nodes"][0].get("progress").is_none());

    for view in ["runtime", "divergence"] {
        let (status, body, response) = get(
            &app.router,
            &format!("/api/sessions/{SESSION_ID}/work-graph?view={view}&source=archive"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let progress = &response["nodes"][0]["progress"];
        let keys = progress
            .as_object()
            .expect("archive progress object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from([
                "agent_id",
                "attempts",
                "finished_at",
                "last_heartbeat_at",
                "started_at",
            ])
        );
        assert_eq!(progress["started_at"], Value::Null);
        assert!(progress["finished_at"].is_string());
        assert_eq!(progress["attempts"], 1);
        assert_eq!(progress["agent_id"], Value::Null);
        assert_eq!(progress["last_heartbeat_at"], Value::Null);
    }
}

#[tokio::test]
async fn archived_progress_keeps_target_and_expansion_outcomes_distinct_in_any_order() {
    let app = test_app().await;
    let mut review_node = node(
        "review-a",
        BindingRef::Role("reviewer".to_string()),
        NodeStatus::Completed,
        "review-outcome",
    );
    review_node.kind = NodeKind::Review;
    review_node.expansion = Some(CompositeExpansion {
        template: "review".to_string(),
        parameters: BTreeMap::from([("target".to_string(), "task-a".to_string())]),
    });
    let runtime_graph = TaskGraph::new(
        vec![
            node(
                "task-a",
                BindingRef::Role("backend".to_string()),
                NodeStatus::Completed,
                "task-outcome",
            ),
            review_node,
        ],
        vec![],
    );
    let review_started = DateTime::<Utc>::from_timestamp(10, 0).expect("review start");
    let review_finished = DateTime::<Utc>::from_timestamp(20, 0).expect("review finish");
    let task_started = DateTime::<Utc>::from_timestamp(30, 0).expect("task start");
    let task_finished = DateTime::<Utc>::from_timestamp(40, 0).expect("task finish");
    let review_outcome = RuntimeOutcome {
        subject_id: "review-a".to_string(),
        task_id: Some("task-a".to_string()),
        agent_ids: vec!["agent-review".to_string()],
        status: RuntimeOutcomeStatus::Completed,
        started_at: Some(review_started),
        finished_at: Some(review_finished),
        attempt_count: 1,
        effects: Vec::new(),
        source_refs: vec!["event:review-a".to_string()],
        completion_evidence: Some(CompletionEvidenceClass::Inferred),
    };
    let task_outcome = RuntimeOutcome {
        subject_id: "task-a".to_string(),
        task_id: Some("task-a".to_string()),
        agent_ids: vec!["agent-task".to_string()],
        status: RuntimeOutcomeStatus::Completed,
        started_at: Some(task_started),
        finished_at: Some(task_finished),
        attempt_count: 3,
        effects: Vec::new(),
        source_refs: vec!["event:task-a".to_string()],
        completion_evidence: Some(CompletionEvidenceClass::Observed),
    };
    let fixture_orders = [
        (
            "wg-api-progress-review-first",
            vec![review_outcome.clone(), task_outcome.clone()],
        ),
        (
            "wg-api-progress-task-first",
            vec![task_outcome, review_outcome],
        ),
    ];
    let mut projected_nodes = Vec::new();

    for (session_id, outcomes) in fixture_orders {
        let session_dir = app
            .storage()
            .create_session_dir(session_id)
            .expect("session directory");
        let archive = WorkGraphArchive {
            schema_version: WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
            archive_id: format!("archive-{session_id}"),
            session_id: session_id.to_string(),
            archived_at: Utc::now(),
            plan_graph: Some(runtime_graph.clone()),
            runtime_graph: runtime_graph.clone(),
            deltas: Vec::new(),
            outcomes,
            divergence: DivergenceSummary::default(),
            sources: Vec::new(),
        };
        let archive_dir = session_dir.join("archive").join("work-graphs");
        std::fs::create_dir_all(&archive_dir).expect("archive directory");
        std::fs::write(
            archive_dir.join(format!("{}.json", archive.archive_id)),
            serde_json::to_vec_pretty(&archive).expect("archive JSON"),
        )
        .expect("archive fixture");

        let (status, body, response) = get(
            &app.router,
            &format!("/api/sessions/{session_id}/work-graph?view=runtime&source=archive"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let nodes = response["nodes"].as_array().expect("archive nodes");
        let task_progress = &nodes
            .iter()
            .find(|node| node["id"] == "task-a")
            .expect("task node")["progress"];
        let review_progress = &nodes
            .iter()
            .find(|node| node["id"] == "review-a")
            .expect("review node")["progress"];
        assert_eq!(task_progress["attempts"], 3, "{body}");
        assert_eq!(response["completion_provenance"]["task-a"], "observed");
        assert_eq!(response["completion_provenance"]["review-a"], "inferred");
        assert_eq!(
            response["completion_source_refs"]["task-a"],
            json!(["event:task-a"])
        );
        assert_eq!(
            response["completion_source_refs"]["review-a"],
            json!(["event:review-a"])
        );
        assert_eq!(task_progress["agent_id"], "agent-task", "{body}");
        assert_eq!(task_progress["started_at"], json!(task_started), "{body}");
        assert_eq!(task_progress["finished_at"], json!(task_finished), "{body}");
        assert_eq!(review_progress["attempts"], 1, "{body}");
        assert_eq!(review_progress["agent_id"], "agent-review", "{body}");
        assert_eq!(
            review_progress["started_at"],
            json!(review_started),
            "{body}"
        );
        assert_eq!(
            review_progress["finished_at"],
            json!(review_finished),
            "{body}"
        );
        projected_nodes.push(response["nodes"].clone());
    }

    assert_eq!(
        projected_nodes[0], projected_nodes[1],
        "archive progress must not depend on outcome ordering"
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
    let mut session = running_session_with_agent(SESSION_ID, "archived-queen");
    session.state = SessionState::Completed;
    app.state
        .session_controller
        .read()
        .insert_test_session(session);

    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=divergence&source=auto"),
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
async fn live_divergence_reports_recorded_runtime_retry_mutation() {
    const SESSION_ID: &str = "wg-api-live-divergence-retry";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let plan = TaskGraph::new(
        vec![node(
            "retry-task",
            BindingRef::Role("backend".to_string()),
            NodeStatus::Ready,
            "retry",
        )],
        vec![],
    );
    let mut retried = plan.clone();
    retried.nodes[0].status = NodeStatus::Running;
    StateManager::new(session_dir)
        .write_work_graph(&plan)
        .expect("persisted plan graph");
    record_graph_change(
        SESSION_ID,
        GraphMutationType::RemediationDetour,
        &plan,
        &retried,
        vec!["retry:test".to_string()],
    )
    .expect("recorded retry mutation")
    .expect("non-empty retry mutation");

    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=divergence&source=live"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(response["source"], "live");
    assert_eq!(
        response["divergence"]["recorded_runtime_mutations"]["remediation_detour"],
        1
    );
}

#[tokio::test]
async fn live_divergence_reports_typed_omission_when_mutation_log_is_untracked() {
    const SESSION_ID: &str = "wg-api-live-divergence-untracked";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let plan = TaskGraph::new(
        vec![node(
            "untracked-task",
            BindingRef::Role("backend".to_string()),
            NodeStatus::Ready,
            "untracked",
        )],
        vec![],
    );
    StateManager::new(session_dir)
        .write_work_graph(&plan)
        .expect("persisted plan graph");

    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=divergence&source=live"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(response["divergence"]["recorded_runtime_mutations"]
        .as_object()
        .is_some_and(serde_json::Map::is_empty));
    assert_eq!(response["omissions"].as_array().map(Vec::len), Some(1));
    let omission = &response["omissions"][0];
    assert_eq!(omission["reason"], "resolution_incomplete");
    assert_eq!(omission["count"], 1);
    assert_eq!(
        omission["detail"],
        "the process did not observe a mutation boundary for this session; zero deltas cannot prove that no earlier structural mutations occurred"
    );
    assert_eq!(
        omission["examples"],
        json!(["mutation-log:not-observed-in-this-process"])
    );
}

#[tokio::test]
async fn live_divergence_combines_completion_and_projection_omissions() {
    const SESSION_ID: &str = "wg-api-live-completion-omission";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let mut graph = TaskGraph::new(
        vec![node(
            "unresolved-task",
            BindingRef::Role("backend".to_string()),
            NodeStatus::Ready,
            "completion-omission",
        )],
        vec![],
    );
    graph.omissions.push(WorkGraphOmission::new(
        WorkGraphOmissionReason::CompletionUnresolved,
        1,
        vec!["event:unmapped-completion:agent:worker-7".to_string()],
    ));
    StateManager::new(session_dir)
        .write_work_graph(&graph)
        .expect("persisted graph with completion omission");

    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=divergence&source=live"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let omissions = response["omissions"]
        .as_array()
        .expect("combined omission evidence");
    assert_eq!(omissions.len(), 2, "{body}");
    assert!(omissions.iter().any(|omission| {
        omission["reason"] == "completion_unresolved"
            && omission["count"] == 1
            && omission["examples"] == json!(["event:unmapped-completion:agent:worker-7"])
    }));
    assert!(omissions
        .iter()
        .any(|omission| omission["reason"] == "resolution_incomplete"));
}

#[tokio::test]
async fn archived_runtime_serializes_completion_unresolved_graph_omission() {
    const SESSION_ID: &str = "wg-api-archive-completion-omission";
    let app = test_app().await;
    let session_dir = app
        .storage()
        .create_session_dir(SESSION_ID)
        .expect("session directory");
    let mut graph = TaskGraph::new(
        vec![node(
            "archived-unresolved-task",
            BindingRef::Role("backend".to_string()),
            NodeStatus::Ready,
            "archived-completion-omission",
        )],
        vec![],
    );
    graph.omissions.push(WorkGraphOmission::new(
        WorkGraphOmissionReason::CompletionUnresolved,
        1,
        vec!["event:archived-unmapped-completion:agent:worker-8".to_string()],
    ));
    StateManager::new(session_dir)
        .write_work_graph(&graph)
        .expect("persisted graph with completion omission");
    archive_completed_session(app.storage().base_dir(), None, SESSION_ID)
        .expect("completed archive");

    let (status, body, response) = get(
        &app.router,
        &format!("/api/sessions/{SESSION_ID}/work-graph?view=runtime&source=archive"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(response["omissions"]
        .as_array()
        .expect("archive graph omissions")
        .iter()
        .any(|omission| {
            omission["reason"] == "completion_unresolved"
                && omission["examples"]
                    == json!(["event:archived-unmapped-completion:agent:worker-8"])
        }));
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
    const CONTRACT_MARKER: &str = "LIFECYCLE-CONTRACT-MUST-CROSS-THE-API";
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
                CONTRACT_MARKER,
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
            body.contains(CONTRACT_MARKER),
            "stage {expected_stage} omitted the task contract"
        );
    }

    let artifact = session_dir.join("work-graph.html");
    let html = std::fs::read_to_string(artifact).expect("crash-stage portable artifact");
    assert!(html.contains("\"lifecycle_stage\":\"Failed\""));
    assert!(html.contains("lifecycle-task"));
    assert!(!html.contains(CONTRACT_MARKER));
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
    let html =
        std::fs::read_to_string(session_dir.join("work-graph.html")).expect("rollback artifact");
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
        (
            "completion rollback",
            "session.state = previous_session_state;",
        ),
        (
            "QA-timeout forward",
            "session.state = SessionState::QaInconclusive;",
        ),
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

    let completion =
        archive_completed_session(temp.path(), None, SESSION_ID).expect("clean archival");
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
