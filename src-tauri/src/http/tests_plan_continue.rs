//! Full-stack tests for leaving the planning phase over HTTP (#371):
//! `POST /api/sessions/{id}/continue` and `POST /api/sessions/{id}/plan-ready`.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chrono::Utc;
use parking_lot::{Mutex, RwLock};
use schemars::schema::RootSchema;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use crate::actions::session::{dispatch_continue_after_planning, dispatch_mark_plan_ready};
use crate::actions::{Action, ActionContext, ActionError, ActionRegistry, Caller};
use crate::coordination::{InjectionManager, QueueManager, StateManager};
use crate::domain::HiveExecutionPolicy;
use crate::events::EventBus;
use crate::http::routes::create_router;
use crate::http::state::AppState;
use crate::pty::{AgentConfig, AgentRole, PtyManager};
use crate::session::{AuthStrategy, Session, SessionController, SessionState, SessionType};
use crate::storage::{ApplicationStateDb, QueueRepo, SessionStorage};

const VALID_PLAN: &str = r#"# Continue over HTTP

## Tasks
- [ ] T1: Establish the root
- [ ] T2: Build on the root (deps: T1)
"#;

const CYCLIC_PLAN: &str = r#"# Cyclic plan

## Tasks
- [ ] T1: First task (deps: T2)
- [ ] T2: Second task (deps: T1)
"#;

struct TestApp {
    _storage_dir: TempDir,
    _project_dir: TempDir,
    router: axum::Router,
    state: Arc<AppState>,
}

impl TestApp {
    fn project_path(&self) -> &Path {
        self._project_dir.path()
    }

    fn session(&self, session_id: &str) -> Session {
        self.state
            .session_controller
            .read()
            .get_session(session_id)
            .expect("session remains queryable")
    }

    fn pty_ids(&self) -> Vec<String> {
        self.state
            .pty_manager
            .read()
            .list_sessions()
            .into_iter()
            .map(|(id, _, _)| id)
            .collect()
    }
}

fn app_with_registry(registry: ActionRegistry) -> TestApp {
    let storage_dir = TempDir::new().expect("temporary storage");
    let project_dir = TempDir::new().expect("temporary project");
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
    state.set_registry(Arc::new(registry));

    TestApp {
        _storage_dir: storage_dir,
        _project_dir: project_dir,
        router: create_router(state.clone()),
        state,
    }
}

fn test_app() -> TestApp {
    app_with_registry(crate::actions::build_registry())
}

/// Seed a session the way a `with_planning` Hive launch leaves it: plan.md and
/// pending-config.json under `.hive-manager/{id}`, a Queen worktree path, and a
/// session state directory in storage.
fn seed_session(
    app: &TestApp,
    session_id: &str,
    session_type: SessionType,
    state: SessionState,
    plan: &str,
) {
    let project_path = app.project_path().to_path_buf();
    let session_dir = project_path.join(".hive-manager").join(session_id);
    std::fs::create_dir_all(&session_dir).expect("project session directory");
    std::fs::write(session_dir.join("plan.md"), plan).expect("plan fixture");
    let queen_config = AgentConfig {
        cli: "claude".to_string(),
        label: Some("Queen".to_string()),
        ..AgentConfig::default()
    };
    let pending_config = json!({
        "project_path": project_path.to_string_lossy(),
        "queen_config": queen_config,
        "workers": [],
        "prompt": "Ship #371",
        "with_planning": true,
    });
    std::fs::write(
        session_dir.join("pending-config.json"),
        serde_json::to_string_pretty(&pending_config).expect("pending config JSON"),
    )
    .expect("pending config fixture");
    let worktree_path = project_path.join("queen-worktree");
    std::fs::create_dir_all(&worktree_path).expect("queen worktree directory");

    app.state
        .storage
        .create_session_dir(session_id)
        .expect("session state directory");
    app.state.session_controller.read().insert_test_session(Session {
        id: session_id.to_string(),
        name: None,
        color: None,
        session_type,
        project_path,
        state,
        created_at: Utc::now(),
        last_activity_at: Utc::now(),
        agents: Vec::new(),
        default_cli: "claude".to_string(),
        default_model: None,
        default_principal_cli: None,
        default_principal_model: None,
        default_principal_flags: Vec::new(),
        execution_policy: HiveExecutionPolicy::default(),
        qa_workers: Vec::new(),
        max_qa_iterations: 3,
        qa_timeout_secs: 300,
        qa_inconclusive_at: None,
        auth_strategy: AuthStrategy::default(),
        worktree_path: Some(worktree_path.to_string_lossy().to_string()),
        worktree_branch: Some(format!("hive/{session_id}/queen")),
        no_git: true,
        resume_report: None,
    });
}

/// POST with the empty JSON body `hive.py continue` sends.
async fn post_empty(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response bytes");
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn error_message(body: &Value) -> String {
    body["error"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn http_continue_moves_planning_hive_to_running_and_spawns_queen() {
    let app = test_app();
    let session_id = "continue-http";
    seed_session(
        &app,
        session_id,
        SessionType::Hive { worker_count: 0 },
        SessionState::Planning,
        VALID_PLAN,
    );

    let (status, body) =
        post_empty(&app.router, &format!("/api/sessions/{session_id}/continue")).await;

    assert_eq!(status, StatusCode::OK, "unexpected continue response: {body}");
    assert_eq!(body["id"], session_id);
    assert_eq!(body["state"], "Running");
    let queen_id = format!("{session_id}-queen");
    let agents = body["agents"].as_array().expect("session agents");
    assert!(
        agents
            .iter()
            .any(|agent| agent["id"] == queen_id.as_str() && agent["role"] == "Queen"),
        "the response must carry the spawned Queen: {body}"
    );

    // The same transition the button produces: state, Queen PTY, persisted
    // plan graph, and the consumed pending config.
    let session = app.session(session_id);
    assert_eq!(session.state, SessionState::Running);
    assert!(session
        .agents
        .iter()
        .any(|agent| agent.id == queen_id && matches!(agent.role, AgentRole::Queen)));
    assert!(app.pty_ids().contains(&queen_id), "Queen PTY was not spawned");
    let graph = StateManager::new(app.state.storage.session_dir(session_id))
        .read_work_graph()
        .expect("graph state is readable")
        .expect("continue persisted the plan graph");
    assert_eq!(graph.nodes.len(), 2);
    assert!(!app
        .project_path()
        .join(".hive-manager")
        .join(session_id)
        .join("pending-config.json")
        .exists());

    // A second press is a wrong-state conflict, not a second Queen.
    let (status, body) =
        post_empty(&app.router, &format!("/api/sessions/{session_id}/continue")).await;
    assert_eq!(status, StatusCode::CONFLICT, "unexpected retry response: {body}");
    assert!(error_message(&body).contains("not in planning phase"), "{body}");
}

#[tokio::test]
async fn http_continue_rejects_wrong_state_with_conflict() {
    let app = test_app();
    let session_id = "continue-running";
    seed_session(
        &app,
        session_id,
        SessionType::Hive { worker_count: 0 },
        SessionState::Running,
        VALID_PLAN,
    );

    let (status, body) =
        post_empty(&app.router, &format!("/api/sessions/{session_id}/continue")).await;

    assert_eq!(status, StatusCode::CONFLICT, "unexpected response: {body}");
    assert!(
        error_message(&body).contains("Session is not in planning phase: Running"),
        "{body}"
    );
    assert_eq!(app.session(session_id).state, SessionState::Running);
    assert!(app.pty_ids().is_empty(), "a rejected continue spawned an agent");
}

#[tokio::test]
async fn http_continue_rejects_solo_and_leaves_session_planning() {
    let app = test_app();
    let session_id = "continue-solo";
    seed_session(
        &app,
        session_id,
        SessionType::Solo {
            cli: "codex".to_string(),
            model: None,
        },
        SessionState::Planning,
        VALID_PLAN,
    );

    let (status, body) =
        post_empty(&app.router, &format!("/api/sessions/{session_id}/continue")).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "unexpected response: {body}");
    assert!(
        error_message(&body).contains("Solo sessions do not support planning continuation"),
        "{body}"
    );
    assert_eq!(app.session(session_id).state, SessionState::Planning);
    assert!(app.pty_ids().is_empty(), "a rejected continue spawned an agent");
}

#[tokio::test]
async fn http_continue_rejects_invalid_plan_graph_and_leaves_session_planning() {
    let app = test_app();
    let session_id = "continue-cycle";
    seed_session(
        &app,
        session_id,
        SessionType::Hive { worker_count: 0 },
        SessionState::Planning,
        CYCLIC_PLAN,
    );

    let (status, body) =
        post_empty(&app.router, &format!("/api/sessions/{session_id}/continue")).await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unexpected response: {body}"
    );
    let message = error_message(&body);
    assert!(
        message.contains("T1") && message.contains("T2"),
        "the validation message must name the cycle: {body}"
    );
    assert_eq!(app.session(session_id).state, SessionState::Planning);
    assert!(app.pty_ids().is_empty(), "a rejected continue spawned the Queen");
    assert!(
        app.project_path()
            .join(".hive-manager")
            .join(session_id)
            .join("pending-config.json")
            .exists(),
        "a rejected continue must keep the pending config for the retry"
    );

    // The same failure through the validate-only route.
    let (status, body) =
        post_empty(&app.router, &format!("/api/sessions/{session_id}/plan-ready")).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unexpected response: {body}"
    );
    assert_eq!(app.session(session_id).state, SessionState::Planning);
}

#[tokio::test]
async fn http_plan_ready_validates_without_spawning_the_queen_then_continue_runs() {
    let app = test_app();
    let session_id = "plan-ready-http";
    seed_session(
        &app,
        session_id,
        SessionType::Hive { worker_count: 0 },
        SessionState::Planning,
        VALID_PLAN,
    );

    let (status, body) =
        post_empty(&app.router, &format!("/api/sessions/{session_id}/plan-ready")).await;

    assert_eq!(status, StatusCode::OK, "unexpected plan-ready response: {body}");
    assert_eq!(body["state"], "PlanReady");
    assert_eq!(app.session(session_id).state, SessionState::PlanReady);
    assert!(app.pty_ids().is_empty(), "plan-ready must not spawn the Queen");

    // plan-ready is Planning-only; a second call is a wrong-state conflict.
    let (status, _) =
        post_empty(&app.router, &format!("/api/sessions/{session_id}/plan-ready")).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // continue accepts PlanReady and finishes the transition.
    let (status, body) =
        post_empty(&app.router, &format!("/api/sessions/{session_id}/continue")).await;
    assert_eq!(status, StatusCode::OK, "unexpected continue response: {body}");
    assert_eq!(body["state"], "Running");
    assert!(app.pty_ids().contains(&format!("{session_id}-queen")));
}

#[tokio::test]
async fn http_continue_rejects_unknown_and_traversal_session_ids() {
    let app = test_app();

    let (status, body) = post_empty(&app.router, "/api/sessions/missing-session/continue").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "unexpected response: {body}");

    for route in ["continue", "plan-ready"] {
        let (status, body) =
            post_empty(&app.router, &format!("/api/sessions/bad..id/{route}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "unexpected {route} response: {body}");
    }
}

#[tokio::test]
async fn planning_actions_are_listed_for_outside_orchestrators() {
    let app = test_app();
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/actions")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response bytes");
    let listing = String::from_utf8(bytes.to_vec()).expect("UTF-8 listing");
    assert!(listing.contains("session.continue_after_planning"));
    assert!(listing.contains("session.mark_plan_ready"));
}

/// Stands in for a planning action and records every dispatch, so a surface that
/// calls the controller directly (bypassing the registry) never reaches it.
struct PlanningActionSpy {
    name: &'static str,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

#[async_trait]
impl Action for PlanningActionSpy {
    fn name(&self) -> &'static str {
        self.name
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(Value)
    }

    async fn run(&self, ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let caller = serde_json::to_value(ctx.caller)
            .expect("caller serializes")
            .as_str()
            .expect("caller is a string")
            .to_string();
        self.calls.lock().push((caller, input));
        Ok(json!({ "spy": self.name }))
    }
}

#[tokio::test]
async fn tauri_commands_and_http_routes_dispatch_the_same_planning_actions() {
    let calls: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ActionRegistry::new();
    for name in ["session.continue_after_planning", "session.mark_plan_ready"] {
        registry.register(Box::new(PlanningActionSpy {
            name,
            calls: Arc::clone(&calls),
        }));
    }
    let app = app_with_registry(registry);
    // A real Planning session: a bypassing surface would transition it.
    seed_session(
        &app,
        "shared-path",
        SessionType::Hive { worker_count: 0 },
        SessionState::Planning,
        VALID_PLAN,
    );

    let (status, body) = post_empty(&app.router, "/api/sessions/shared-path/continue").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "spy": "session.continue_after_planning" }));
    let (status, body) = post_empty(&app.router, "/api/sessions/shared-path/plan-ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "spy": "session.mark_plan_ready" }));

    // What the Tauri commands run (pinned by the source test below).
    let frontend = ActionContext::new(Caller::Frontend, Arc::clone(&app.state));
    let output = dispatch_continue_after_planning(app.state.registry(), &frontend, "shared-path")
        .await
        .expect("Tauri continue dispatch");
    assert_eq!(output, json!({ "spy": "session.continue_after_planning" }));
    let output = dispatch_mark_plan_ready(app.state.registry(), &frontend, "shared-path")
        .await
        .expect("Tauri plan-ready dispatch");
    assert_eq!(output, json!({ "spy": "session.mark_plan_ready" }));

    assert_eq!(
        *calls.lock(),
        vec![
            ("http".to_string(), json!({ "id": "shared-path" })),
            ("http".to_string(), json!({ "id": "shared-path" })),
            ("frontend".to_string(), json!({ "id": "shared-path" })),
            ("frontend".to_string(), json!({ "id": "shared-path" })),
        ]
    );
    assert_eq!(
        app.session("shared-path").state,
        SessionState::Planning,
        "a surface reached the controller without going through the action"
    );
    assert!(app.pty_ids().is_empty());
}

/// Return the body of `pub async fn {name}(` that follows `marker` in `source`.
fn function_body<'a>(source: &'a str, marker: &str, name: &str) -> &'a str {
    let signature = format!("{marker}pub async fn {name}(");
    let start = source
        .find(&signature)
        .unwrap_or_else(|| panic!("missing {marker}{name}"));
    let body = &source[start..];
    &body[..body.find("\n}\n").expect("function body end")]
}

/// The `#[tauri::command]` wrappers are compiled out of the test build and need
/// a running Tauri app to invoke, so pin both surfaces' bodies to the shared
/// dispatch helpers exercised above.
#[test]
fn tauri_commands_and_http_handlers_delegate_to_the_shared_dispatch_helpers() {
    let commands = include_str!("../commands/session_commands.rs").replace("\r\n", "\n");
    let handlers = include_str!("handlers/sessions.rs").replace("\r\n", "\n");
    for (command, helper) in [
        ("continue_after_planning", "dispatch_continue_after_planning"),
        ("mark_plan_ready", "dispatch_mark_plan_ready"),
    ] {
        for (surface, body) in [
            (
                "Tauri command",
                function_body(&commands, "#[tauri::command]\n", command),
            ),
            ("HTTP handler", function_body(&handlers, "", command)),
        ] {
            assert!(
                body.contains(&format!("{helper}(")),
                "{surface} {command} must dispatch through {helper}"
            );
            assert!(
                !body.contains("controller"),
                "{surface} {command} must not call the SessionController directly"
            );
        }
    }
}
