use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;
use crate::http::routes::create_router;
use crate::http::state::AppState;
use crate::storage::{SessionStorage, PersistedSession, SessionTypeInfo};
use crate::pty::PtyManager;
use crate::session::{Session, SessionController, SessionState, SessionType, AgentInfo, AuthStrategy};
use crate::pty::{AgentRole, AgentStatus, AgentConfig};
use crate::coordination::InjectionManager;
use crate::events::EventBus;
use parking_lot::RwLock;

async fn setup_test_app() -> axum::Router {
    let storage = Arc::new(SessionStorage::new().unwrap());
    let config = Arc::new(tokio::sync::RwLock::new(storage.load_config().unwrap()));
    let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
    let session_controller = Arc::new(RwLock::new(SessionController::new(pty_manager.clone())));
    session_controller.write().set_storage(storage.clone());
    let injection_manager = Arc::new(RwLock::new(InjectionManager::new(
        pty_manager.clone(),
        SessionStorage::new().unwrap(),
    )));
    let event_bus = EventBus::new(storage.base_dir().clone());
    let state = Arc::new(AppState::new(
        config,
        pty_manager,
        session_controller,
        injection_manager,
        storage,
        event_bus,
        None,
    ));

    create_router(state)
}

/// Setup test app and return both the router and session controller for inserting test sessions
async fn setup_test_app_with_controller() -> (axum::Router, Arc<RwLock<SessionController>>) {
    let storage = Arc::new(SessionStorage::new().unwrap());
    let config = Arc::new(tokio::sync::RwLock::new(storage.load_config().unwrap()));
    let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
    let session_controller = Arc::new(RwLock::new(SessionController::new(pty_manager.clone())));
    session_controller.write().set_storage(storage.clone());
    let injection_manager = Arc::new(RwLock::new(InjectionManager::new(
        pty_manager.clone(),
        SessionStorage::new().unwrap(),
    )));
    let event_bus = EventBus::new(storage.base_dir().clone());
    let state = Arc::new(AppState::new(
        config,
        pty_manager,
        session_controller.clone(),
        injection_manager,
        storage,
        event_bus,
        None,
    ));

    (create_router(state), session_controller)
}

fn make_test_session(id: &str, project_path: &str) -> Session {
    Session {
        id: id.to_string(),
        name: None,
        color: None,
        session_type: SessionType::Hive { worker_count: 1 },
        project_path: PathBuf::from(project_path),
        state: SessionState::Running,
        created_at: chrono::Utc::now(),
        agents: vec![],
        default_cli: "claude".to_string(),
        default_model: Some("opus-4-6".to_string()),
        max_qa_iterations: 3,
        qa_timeout_secs: 300,
        auth_strategy: AuthStrategy::default(),
    }
}

fn make_test_session_with_agents(id: &str, project_path: &str, agent_ids: &[&str]) -> Session {
    let agents: Vec<AgentInfo> = agent_ids
        .iter()
        .enumerate()
        .map(|(i, aid)| AgentInfo {
            id: (*aid).to_string(),
            role: AgentRole::Worker {
                index: (i + 1) as u8,
                parent: None,
            },
            status: AgentStatus::Running,
            config: AgentConfig::default(),
            parent_id: None,
        })
        .collect();
    Session {
        id: id.to_string(),
        name: None,
        color: None,
        session_type: SessionType::Hive { worker_count: 1 },
        project_path: PathBuf::from(project_path),
        state: SessionState::Running,
        created_at: chrono::Utc::now(),
        agents,
        default_cli: "claude".to_string(),
        default_model: Some("opus-4-6".to_string()),
        max_qa_iterations: 3,
        qa_timeout_secs: 300,
        auth_strategy: AuthStrategy::default(),
    }
}

#[tokio::test]
async fn test_health_check() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_list_sessions_empty() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_get_nonexistent_session() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_patch_session_updates_name_and_color() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-patch-session");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller
        .read()
        .insert_test_session(make_test_session("session-patch", temp_dir.to_str().unwrap()));

    let body = serde_json::json!({
        "name": "Alpha Session",
        "color": "#7aa2f7"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/sessions/session-patch")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(response_json.get("name").unwrap().as_str().unwrap(), "Alpha Session");
    assert_eq!(response_json.get("color").unwrap().as_str().unwrap(), "#7aa2f7");

    let session = controller.read().get_session("session-patch").unwrap();
    assert_eq!(session.name.as_deref(), Some("Alpha Session"));
    assert_eq!(session.color.as_deref(), Some("#7aa2f7"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_patch_session_omitted_field_preserves_existing_value() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-patch-preserve-color");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(Session {
        id: "session-preserve".to_string(),
        name: Some("Original".to_string()),
        color: Some("#7aa2f7".to_string()),
        session_type: SessionType::Hive { worker_count: 1 },
        project_path: temp_dir.clone(),
        state: SessionState::Running,
        created_at: chrono::Utc::now(),
        agents: vec![],
        default_cli: "claude".to_string(),
        default_model: Some("opus-4-6".to_string()),
        max_qa_iterations: 3,
        qa_timeout_secs: 300,
        auth_strategy: AuthStrategy::default(),
    });

    let body = serde_json::json!({
        "name": "Renamed"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/sessions/session-preserve")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let session = controller.read().get_session("session-preserve").unwrap();
    assert_eq!(session.name.as_deref(), Some("Renamed"));
    assert_eq!(session.color.as_deref(), Some("#7aa2f7"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_patch_session_null_clears_field() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-patch-clear-color");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(Session {
        id: "session-clear".to_string(),
        name: Some("Original".to_string()),
        color: Some("#7aa2f7".to_string()),
        session_type: SessionType::Hive { worker_count: 1 },
        project_path: temp_dir.clone(),
        state: SessionState::Running,
        created_at: chrono::Utc::now(),
        agents: vec![],
        default_cli: "claude".to_string(),
        default_model: Some("opus-4-6".to_string()),
        max_qa_iterations: 3,
        qa_timeout_secs: 300,
        auth_strategy: AuthStrategy::default(),
    });

    let body = serde_json::json!({
        "color": null
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/sessions/session-clear")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let session = controller.read().get_session("session-clear").unwrap();
    assert_eq!(session.name.as_deref(), Some("Original"));
    assert_eq!(session.color, None);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_patch_session_rejects_invalid_color() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-patch-invalid-color");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller
        .read()
        .insert_test_session(make_test_session("session-patch-color", temp_dir.to_str().unwrap()));

    let body = serde_json::json!({
        "name": "Alpha Session",
        "color": "#ffffff"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/sessions/session-patch-color")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_patch_session_rejects_whitespace_name() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-patch-whitespace-name");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller
        .read()
        .insert_test_session(make_test_session("session-patch-whitespace", temp_dir.to_str().unwrap()));

    let body = serde_json::json!({
        "name": "   ",
        "color": "#7aa2f7"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/sessions/session-patch-whitespace")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_patch_session_rejects_invalid_name() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-patch-invalid-name");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller
        .read()
        .insert_test_session(make_test_session("session-patch-name", temp_dir.to_str().unwrap()));

    let body = serde_json::json!({
        "name": "../escape",
        "color": "#7aa2f7"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/sessions/session-patch-name")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_patch_session_updates_persisted_session_not_loaded_in_memory() {
    let (app, _controller) = setup_test_app_with_controller().await;
    let storage = SessionStorage::new().unwrap();
    let session_id = format!("persisted-patch-{}", uuid::Uuid::new_v4());
    let session_dir = storage.session_dir(&session_id);
    let _ = std::fs::remove_dir_all(&session_dir);

    let persisted = PersistedSession {
        id: session_id.clone(),
        name: Some("Stored".to_string()),
        color: Some("#7aa2f7".to_string()),
        session_type: SessionTypeInfo::Hive { worker_count: 1 },
        project_path: std::env::temp_dir().join("hive-test-persisted-update").to_string_lossy().to_string(),
        created_at: chrono::Utc::now(),
        agents: vec![],
        state: "Completed".to_string(),
        default_cli: "claude".to_string(),
        default_model: Some("opus-4-6".to_string()),
        max_qa_iterations: 3,
        qa_timeout_secs: 300,
        auth_strategy: String::new(),
    };
    storage.save_session(&persisted).unwrap();

    let body = serde_json::json!({
        "name": "Persisted Rename"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/sessions/{}", session_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let updated = storage.load_session(&session_id).unwrap();
    assert_eq!(updated.name.as_deref(), Some("Persisted Rename"));
    assert_eq!(updated.color.as_deref(), Some("#7aa2f7"));

    let _ = std::fs::remove_dir_all(storage.session_dir(&session_id));
}

// --- Session-scoped learnings endpoint tests ---

#[tokio::test]
async fn test_session_scoped_list_learnings_returns_404_for_nonexistent_session() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/nonexistent-id/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_session_scoped_project_dna_returns_404_for_nonexistent_session() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/nonexistent-id/project-dna")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_session_scoped_list_learnings_for_valid_session() {
    let (app, controller) = setup_test_app_with_controller().await;

    // Insert a test session with a temp dir as project path
    let temp_dir = std::env::temp_dir().join("hive-test-session-a");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-a", temp_dir.to_str().unwrap())
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-a/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_project_dna_for_valid_session() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-session-dna");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-dna", temp_dir.to_str().unwrap())
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-dna/project-dna")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_legacy_learnings_works_with_single_session() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-single-session");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-single", temp_dir.to_str().unwrap())
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Old endpoint should work when only one project is active
    assert_eq!(response.status(), StatusCode::OK);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_legacy_learnings_returns_error_with_multiple_projects() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir_a = std::env::temp_dir().join("hive-test-multi-a");
    let temp_dir_b = std::env::temp_dir().join("hive-test-multi-b");
    let _ = std::fs::create_dir_all(&temp_dir_a);
    let _ = std::fs::create_dir_all(&temp_dir_b);

    // Insert two sessions with different project paths
    controller.read().insert_test_session(
        make_test_session("session-multi-a", temp_dir_a.to_str().unwrap())
    );
    controller.read().insert_test_session(
        make_test_session("session-multi-b", temp_dir_b.to_str().unwrap())
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Old endpoint should return 400 with helpful error when multiple projects active
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir_a);
    let _ = std::fs::remove_dir_all(&temp_dir_b);
}

#[tokio::test]
async fn test_session_scoped_learnings_work_with_multiple_projects() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir_a = std::env::temp_dir().join("hive-test-scoped-a");
    let temp_dir_b = std::env::temp_dir().join("hive-test-scoped-b");
    let _ = std::fs::create_dir_all(&temp_dir_a);
    let _ = std::fs::create_dir_all(&temp_dir_b);

    controller.read().insert_test_session(
        make_test_session("scoped-a", temp_dir_a.to_str().unwrap())
    );
    controller.read().insert_test_session(
        make_test_session("scoped-b", temp_dir_b.to_str().unwrap())
    );

    // Session-scoped endpoint should work even with multiple projects
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/scoped-a/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let _ = std::fs::remove_dir_all(&temp_dir_a);
    let _ = std::fs::remove_dir_all(&temp_dir_b);
}

#[tokio::test]
async fn test_session_scoped_submit_learning_validates_input() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-submit-validation");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-submit", temp_dir.to_str().unwrap())
    );

    // Submit with empty insight should fail
    let body = serde_json::json!({
        "session": "session-submit",
        "task": "test task",
        "outcome": "success",
        "keywords": [],
        "insight": "",
        "files_touched": []
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-submit/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_submit_learning_rejects_path_traversal() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-path-traversal");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-traversal", temp_dir.to_str().unwrap())
    );

    // Submit with path traversal in files_touched should fail
    let body = serde_json::json!({
        "session": "session-traversal",
        "task": "test task",
        "outcome": "success",
        "keywords": ["test"],
        "insight": "A valid insight",
        "files_touched": ["../../etc/passwd"]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-traversal/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_submit_learning_success() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-submit-success");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-ok", temp_dir.to_str().unwrap())
    );

    let body = serde_json::json!({
        "session": "session-ok",
        "task": "implement feature X",
        "outcome": "success",
        "keywords": ["feature", "api"],
        "insight": "Using session-scoped endpoints prevents multi-project conflicts",
        "files_touched": ["src/handlers/learnings.rs"]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-ok/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_submit_learning_validates_empty_session() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-empty-session");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-empty-session", temp_dir.to_str().unwrap())
    );

    let body = serde_json::json!({
        "session": "",
        "task": "test task",
        "outcome": "success",
        "keywords": [],
        "insight": "A valid insight",
        "files_touched": []
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-empty-session/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_submit_learning_validates_empty_task() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-empty-task");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-empty-task", temp_dir.to_str().unwrap())
    );

    let body = serde_json::json!({
        "session": "session-empty-task",
        "task": "",
        "outcome": "success",
        "keywords": [],
        "insight": "A valid insight",
        "files_touched": []
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-empty-task/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_submit_learning_validates_invalid_outcome() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-invalid-outcome");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-invalid-outcome", temp_dir.to_str().unwrap())
    );

    let body = serde_json::json!({
        "session": "session-invalid-outcome",
        "task": "test task",
        "outcome": "invalid",
        "keywords": [],
        "insight": "A valid insight",
        "files_touched": []
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-invalid-outcome/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_submit_learning_validates_all_outcomes() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-all-outcomes");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-outcomes", temp_dir.to_str().unwrap())
    );

    for outcome in ["success", "partial", "failed"] {
        let body = serde_json::json!({
            "session": "session-outcomes",
            "task": format!("test task {}", outcome),
            "outcome": outcome,
            "keywords": [],
            "insight": "A valid insight",
            "files_touched": []
        });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions/session-outcomes/learnings")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_submit_learning_rejects_absolute_paths() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-absolute-path");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-absolute", temp_dir.to_str().unwrap())
    );

    // Test absolute Unix path
    let body = serde_json::json!({
        "session": "session-absolute",
        "task": "test task",
        "outcome": "success",
        "keywords": [],
        "insight": "A valid insight",
        "files_touched": ["/etc/passwd"]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-absolute/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Test Windows absolute path
    let body = serde_json::json!({
        "session": "session-absolute",
        "task": "test task",
        "outcome": "success",
        "keywords": [],
        "insight": "A valid insight",
        "files_touched": ["C:\\Windows\\System32"]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-absolute/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_submit_learning_returns_learning_id() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-learning-id");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-learning-id", temp_dir.to_str().unwrap())
    );

    let body = serde_json::json!({
        "session": "session-learning-id",
        "task": "test task",
        "outcome": "success",
        "keywords": ["test"],
        "insight": "A valid insight",
        "files_touched": ["src/file.rs"]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-learning-id/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    assert!(response_json.get("learning_id").is_some());
    assert_eq!(response_json.get("message").unwrap().as_str().unwrap(), "Learning submitted successfully");
    assert!(!response_json.get("learning_id").unwrap().as_str().unwrap().is_empty());

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_list_learnings_with_filtering_by_category() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-filter-category");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-filter", temp_dir.to_str().unwrap())
    );

    // Submit multiple learnings with different outcomes
    for (i, outcome) in ["success", "partial", "failed", "success"].iter().enumerate() {
        let body = serde_json::json!({
            "session": "session-filter",
            "task": format!("task {}", i),
            "outcome": outcome,
            "keywords": [],
            "insight": format!("insight {}", i),
            "files_touched": []
        });

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions/session-filter/learnings")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    // Filter by success
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-filter/learnings?category=success")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings.len(), 2);
    assert!(learnings.iter().all(|l| l.get("outcome").unwrap().as_str().unwrap() == "success"));

    // Filter by failed
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-filter/learnings?category=failed")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings.len(), 1);
    assert_eq!(learnings[0].get("outcome").unwrap().as_str().unwrap(), "failed");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_list_learnings_with_filtering_by_keywords() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-filter-keywords");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-filter-kw", temp_dir.to_str().unwrap())
    );

    // Submit learnings with different keywords
    let test_cases = vec![
        (vec!["rust", "api"], "task 1"),
        (vec!["api", "test"], "task 2"),
        (vec!["rust"], "task 3"),
        (vec!["frontend"], "task 4"),
    ];

    for (keywords, task) in test_cases {
        let body = serde_json::json!({
            "session": "session-filter-kw",
            "task": task,
            "outcome": "success",
            "keywords": keywords,
            "insight": "insight",
            "files_touched": []
        });

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions/session-filter-kw/learnings")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    // Filter by "rust" - should match 2 learnings
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-filter-kw/learnings?keywords=rust")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings.len(), 2);
    assert!(learnings.iter().any(|l| l.get("task").unwrap().as_str().unwrap() == "task 1"));
    assert!(learnings.iter().any(|l| l.get("task").unwrap().as_str().unwrap() == "task 3"));

    // Filter by multiple keywords (comma-separated)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-filter-kw/learnings?keywords=api,test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    // Should match learnings that have either "api" or "test"
    assert!(learnings.len() >= 2);

    // Filter by non-existent keyword
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-filter-kw/learnings?keywords=nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings.len(), 0);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_list_learnings_with_combined_filters() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-combined-filters");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-combined", temp_dir.to_str().unwrap())
    );

    // Submit learnings with different combinations
    let test_cases = vec![
        ("success", vec!["rust"], "task 1"),
        ("success", vec!["api"], "task 2"),
        ("failed", vec!["rust"], "task 3"),
        ("partial", vec!["api"], "task 4"),
    ];

    for (outcome, keywords, task) in test_cases {
        let body = serde_json::json!({
            "session": "session-combined",
            "task": task,
            "outcome": outcome,
            "keywords": keywords,
            "insight": "insight",
            "files_touched": []
        });

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions/session-combined/learnings")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    // Filter by category=success AND keywords=rust
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-combined/learnings?category=success&keywords=rust")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings.len(), 1);
    assert_eq!(learnings[0].get("task").unwrap().as_str().unwrap(), "task 1");
    assert_eq!(learnings[0].get("outcome").unwrap().as_str().unwrap(), "success");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_list_learnings_returns_correct_structure() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-structure");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-structure", temp_dir.to_str().unwrap())
    );

    // Submit a learning
    let body = serde_json::json!({
        "session": "session-structure",
        "task": "test task",
        "outcome": "success",
        "keywords": ["test", "api"],
        "insight": "test insight",
        "files_touched": ["src/file.rs", "tests/file.rs"]
    });

    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-structure/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // List learnings and verify structure
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-structure/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    assert!(response_json.get("learnings").is_some());
    assert!(response_json.get("count").is_some());

    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings.len(), 1);
    assert_eq!(response_json.get("count").unwrap().as_u64().unwrap(), 1);

    let learning = &learnings[0];
    assert!(learning.get("id").is_some());
    assert!(learning.get("date").is_some());
    assert_eq!(learning.get("session").unwrap().as_str().unwrap(), "session-structure");
    assert_eq!(learning.get("task").unwrap().as_str().unwrap(), "test task");
    assert_eq!(learning.get("outcome").unwrap().as_str().unwrap(), "success");
    assert_eq!(learning.get("insight").unwrap().as_str().unwrap(), "test insight");
    
    let keywords = learning.get("keywords").unwrap().as_array().unwrap();
    assert_eq!(keywords.len(), 2);
    assert!(keywords.iter().any(|k| k.as_str().unwrap() == "test"));
    assert!(keywords.iter().any(|k| k.as_str().unwrap() == "api"));

    let files = learning.get("files_touched").unwrap().as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|f| f.as_str().unwrap() == "src/file.rs"));
    assert!(files.iter().any(|f| f.as_str().unwrap() == "tests/file.rs"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_delete_learning_success() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-delete-success");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-delete", temp_dir.to_str().unwrap())
    );

    // Submit a learning
    let body = serde_json::json!({
        "session": "session-delete",
        "task": "test task",
        "outcome": "success",
        "keywords": [],
        "insight": "test insight",
        "files_touched": []
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-delete/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let learning_id = response_json.get("learning_id").unwrap().as_str().unwrap();

    // Delete the learning
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/sessions/session-delete/learnings/{}", learning_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify it's deleted by listing learnings
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-delete/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings.len(), 0);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_delete_learning_not_found() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-delete-notfound");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-delete-nf", temp_dir.to_str().unwrap())
    );

    // Try to delete a non-existent learning
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/sessions/session-delete-nf/learnings/nonexistent-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_session_scoped_delete_learning_returns_404_for_nonexistent_session() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/sessions/nonexistent-session/learnings/some-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_session_scoped_delete_learning_preserves_other_learnings() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-delete-preserve");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-delete-preserve", temp_dir.to_str().unwrap())
    );

    // Submit two learnings
    let mut learning_ids = Vec::new();
    for i in 0..2 {
        let body = serde_json::json!({
            "session": "session-delete-preserve",
            "task": format!("task {}", i),
            "outcome": "success",
            "keywords": [],
            "insight": format!("insight {}", i),
            "files_touched": []
        });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions/session-delete-preserve/learnings")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let learning_id = response_json.get("learning_id").unwrap().as_str().unwrap().to_string();
        learning_ids.push(learning_id);
    }

    // Delete the first learning
    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/sessions/session-delete-preserve/learnings/{}", learning_ids[0]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    // Verify only one learning remains
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-delete-preserve/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings.len(), 1);
    assert_eq!(learnings[0].get("task").unwrap().as_str().unwrap(), "task 1");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_legacy_submit_learning_validates_input() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-legacy-submit");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-legacy", temp_dir.to_str().unwrap())
    );

    // Test empty session
    let body = serde_json::json!({
        "session": "",
        "task": "test task",
        "outcome": "success",
        "keywords": [],
        "insight": "valid insight",
        "files_touched": []
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Test invalid outcome
    let body = serde_json::json!({
        "session": "session-legacy",
        "task": "test task",
        "outcome": "invalid",
        "keywords": [],
        "insight": "valid insight",
        "files_touched": []
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_legacy_list_learnings_with_filtering() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-legacy-filter");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-legacy-filter", temp_dir.to_str().unwrap())
    );

    // Submit learnings via legacy endpoint
    for (i, outcome) in ["success", "failed"].iter().enumerate() {
        let body = serde_json::json!({
            "session": "session-legacy-filter",
            "task": format!("task {}", i),
            "outcome": outcome,
            "keywords": ["legacy"],
            "insight": format!("insight {}", i),
            "files_touched": []
        });

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/learnings")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    // Filter by category
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/learnings?category=success")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings.len(), 1);
    assert_eq!(learnings[0].get("outcome").unwrap().as_str().unwrap(), "success");

    // Filter by keywords
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/learnings?keywords=legacy")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings.len(), 2);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// --- E2E: Session Isolation ---

#[tokio::test]
async fn test_e2e_session_isolation() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir_a = std::env::temp_dir().join("hive-test-isolation-a");
    let temp_dir_b = std::env::temp_dir().join("hive-test-isolation-b");
    let _ = std::fs::create_dir_all(&temp_dir_a);
    let _ = std::fs::create_dir_all(&temp_dir_b);

    controller.read().insert_test_session(
        make_test_session("iso-session-a", temp_dir_a.to_str().unwrap())
    );
    controller.read().insert_test_session(
        make_test_session("iso-session-b", temp_dir_b.to_str().unwrap())
    );

    // POST a learning to session A only
    let body = serde_json::json!({
        "session": "iso-session-a",
        "task": "implement isolation feature",
        "outcome": "success",
        "keywords": ["isolation", "test"],
        "insight": "Session-scoped storage prevents cross-contamination",
        "files_touched": ["src/storage/mod.rs"]
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/iso-session-a/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    // Verify session A has the learning
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sessions/iso-session-a/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let learnings_a = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings_a.len(), 1);
    assert_eq!(learnings_a[0].get("task").unwrap().as_str().unwrap(), "implement isolation feature");

    // Verify session B has NO learnings (isolation)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sessions/iso-session-b/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let learnings_b = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings_b.len(), 0, "Session B should have no learnings - session isolation violated");

    // Now POST to session B and verify both are independent
    let body_b = serde_json::json!({
        "session": "iso-session-b",
        "task": "separate task for B",
        "outcome": "partial",
        "keywords": ["different"],
        "insight": "B has its own learnings",
        "files_touched": []
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/iso-session-b/learnings")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body_b).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    // Re-check session A still has only 1 learning
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sessions/iso-session-a/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let learnings_a = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings_a.len(), 1, "Session A should still have exactly 1 learning after B got a new one");

    // Check session B has exactly 1 learning
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/iso-session-b/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let learnings_b = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings_b.len(), 1);
    assert_eq!(learnings_b[0].get("outcome").unwrap().as_str().unwrap(), "partial");

    let _ = std::fs::remove_dir_all(&temp_dir_a);
    let _ = std::fs::remove_dir_all(&temp_dir_b);
}

#[tokio::test]
async fn test_e2e_delete_does_not_affect_other_sessions() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir_a = std::env::temp_dir().join("hive-test-delete-iso-a");
    let temp_dir_b = std::env::temp_dir().join("hive-test-delete-iso-b");
    let _ = std::fs::create_dir_all(&temp_dir_a);
    let _ = std::fs::create_dir_all(&temp_dir_b);

    controller.read().insert_test_session(
        make_test_session("del-iso-a", temp_dir_a.to_str().unwrap())
    );
    controller.read().insert_test_session(
        make_test_session("del-iso-b", temp_dir_b.to_str().unwrap())
    );

    // POST learnings to both sessions
    for (session_id, task) in [("del-iso-a", "task A"), ("del-iso-b", "task B")] {
        let body = serde_json::json!({
            "session": session_id,
            "task": task,
            "outcome": "success",
            "keywords": [],
            "insight": "test insight",
            "files_touched": []
        });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{}/learnings", session_id))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    // Get learning_id from session A
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sessions/del-iso-a/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    let learning_id = learnings[0].get("id").unwrap().as_str().unwrap();

    // DELETE from session A
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/sessions/del-iso-a/learnings/{}", learning_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify session A is now empty
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sessions/del-iso-a/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings.len(), 0);

    // Verify session B is UNAFFECTED - still has its learning
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/del-iso-b/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let learnings = response_json.get("learnings").unwrap().as_array().unwrap();
    assert_eq!(learnings.len(), 1, "Session B should still have its learning after deleting from session A");
    assert_eq!(learnings[0].get("task").unwrap().as_str().unwrap(), "task B");

    let _ = std::fs::remove_dir_all(&temp_dir_a);
    let _ = std::fs::remove_dir_all(&temp_dir_b);
}

#[tokio::test]
async fn test_legacy_list_learnings_returns_error_with_no_sessions() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/learnings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// --- Worker/Planner CLI validation and session_id validation tests ---

#[tokio::test]
async fn test_add_worker_rejects_invalid_cli() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-invalid-cli");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session("session-cli-test", temp_dir.to_str().unwrap())
    );

    let body = serde_json::json!({
        "role_type": "backend",
        "cli": "invalid-command"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-cli-test/workers")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let error_msg = response_json.get("error").unwrap().as_str().unwrap();
    assert!(error_msg.contains("Invalid CLI"), "Error should mention invalid CLI: {}", error_msg);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_add_worker_rejects_path_traversal_session_id() {
    let app = setup_test_app().await;

    let body = serde_json::json!({
        "role_type": "backend"
    });

    // Use "..evil" which contains ".." and triggers validate_session_id rejection
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/..evil/workers")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let error_msg = response_json.get("error").unwrap().as_str().unwrap();
    assert!(error_msg.contains("Invalid session ID"), "Error should mention invalid session ID: {}", error_msg);
}

#[tokio::test]
async fn test_add_worker_explicit_cli_overrides_session_default() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-cli-override");
    let _ = std::fs::create_dir_all(&temp_dir);

    // Create a session with default_cli = "gemini" (not "claude")
    let mut session = make_test_session("session-override", temp_dir.to_str().unwrap());
    session.default_cli = "gemini".to_string();
    controller.read().insert_test_session(session);

    // POST with explicit cli: "droid" - should pass CLI validation (not 400)
    // The handler may fail downstream at PTY spawn (500), but should NOT reject
    // the CLI itself since "droid" is in the allowlist.
    let body = serde_json::json!({
        "role_type": "backend",
        "cli": "droid"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-override/workers")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should NOT be 400 - "droid" is a valid CLI that overrides session default "gemini"
    assert_ne!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Explicit CLI 'droid' should override session default 'gemini' and pass validation"
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_add_worker_request_accepts_name_and_description_fields() {
    let request: crate::http::handlers::workers::AddWorkerRequest = serde_json::from_value(
        serde_json::json!({
            "role_type": "frontend",
            "cli": "codex",
            "name": "Worker 2 (Frontend)",
            "description": "SSE resync + chat/timeline event handling",
            "initial_task": "Handle SSE lagged events"
        }),
    )
    .unwrap();

    assert_eq!(request.name.as_deref(), Some("Worker 2 (Frontend)"));
    assert_eq!(
        request.description.as_deref(),
        Some("SSE resync + chat/timeline event handling")
    );
}

#[test]
fn test_add_worker_request_blank_name_deserializes_to_none() {
    for raw_name in ["", "   "] {
        let request: crate::http::handlers::workers::AddWorkerRequest = serde_json::from_value(
            serde_json::json!({
                "role_type": "frontend",
                "cli": "codex",
                "name": raw_name,
                "description": "SSE resync + chat/timeline event handling",
                "initial_task": "Handle SSE lagged events"
            }),
        )
        .unwrap();

        assert!(
            request.name.is_none(),
            "expected blank name {:?} to deserialize as None",
            raw_name
        );
    }
}

#[test]
fn test_persisted_agent_config_round_trips_name_and_description_fields() {
    let config = crate::storage::PersistedAgentConfig {
        cli: "codex".to_string(),
        model: Some("gpt-5.4".to_string()),
        flags: vec![],
        label: Some("Worker 2 (Frontend) — SSE resync + chat/timeline event handling".to_string()),
        name: Some("Worker 2 (Frontend)".to_string()),
        description: Some("SSE resync + chat/timeline event handling".to_string()),
        role_type: Some("frontend".to_string()),
        initial_prompt: Some("Handle SSE lagged events".to_string()),
    };

    let encoded = serde_json::to_string(&config).unwrap();
    let decoded: crate::storage::PersistedAgentConfig = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded.name.as_deref(), Some("Worker 2 (Frontend)"));
    assert_eq!(
        decoded.description.as_deref(),
        Some("SSE resync + chat/timeline event handling")
    );
    assert_eq!(
        decoded.label.as_deref(),
        Some("Worker 2 (Frontend) — SSE resync + chat/timeline event handling")
    );
}

#[test]
fn test_persisted_agent_config_blank_name_round_trip_uses_indexed_default_behavior() {
    for raw_name in ["", "   "] {
        let config = crate::storage::PersistedAgentConfig {
            cli: "codex".to_string(),
            model: Some("gpt-5.4".to_string()),
            flags: vec![],
            label: Some("Worker 2 (Frontend) — SSE resync + chat/timeline event handling".to_string()),
            name: Some(raw_name.to_string()),
            description: Some("SSE resync + chat/timeline event handling".to_string()),
            role_type: Some("frontend".to_string()),
            initial_prompt: Some("Handle SSE lagged events".to_string()),
        };

        let encoded = serde_json::to_string(&config).unwrap();
        let decoded: crate::storage::PersistedAgentConfig = serde_json::from_str(&encoded).unwrap();

        assert!(
            decoded.name.is_none(),
            "expected blank persisted name {:?} to deserialize as None",
            raw_name
        );
        assert_eq!(
            decoded.description.as_deref(),
            Some("SSE resync + chat/timeline event handling")
        );
    }
}

#[tokio::test]
async fn test_add_qa_worker_rejects_invalid_cli() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-invalid-qa-cli");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller
        .read()
        .insert_test_session(make_test_session("session-qa-cli-test", temp_dir.to_str().unwrap()));

    let body = serde_json::json!({
        "specialization": "ui",
        "cli": "invalid-command"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-qa-cli-test/qa-workers")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let error_msg = response_json.get("error").unwrap().as_str().unwrap();
    assert!(error_msg.contains("Invalid CLI"), "Error should mention invalid CLI: {}", error_msg);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_add_qa_worker_rejects_invalid_specialization() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-invalid-qa-specialization");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller
        .read()
        .insert_test_session(make_test_session("session-qa-specialization", temp_dir.to_str().unwrap()));

    let body = serde_json::json!({
        "specialization": "performance"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-qa-specialization/qa-workers")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let error_msg = response_json.get("error").unwrap().as_str().unwrap();
    assert!(error_msg.contains("Invalid QA specialization"), "Error should mention invalid specialization: {}", error_msg);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_add_qa_worker_rejects_path_traversal_session_id() {
    let app = setup_test_app().await;

    let body = serde_json::json!({
        "specialization": "ui"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/..evil/qa-workers")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let error_msg = response_json.get("error").unwrap().as_str().unwrap();
    assert!(error_msg.contains("Invalid session ID"), "Error should mention invalid session ID: {}", error_msg);
}

#[tokio::test]
async fn test_add_qa_worker_valid_request_reaches_controller() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-valid-qa-worker");
    let _ = std::fs::create_dir_all(&temp_dir);

    let mut session = make_test_session("session-qa-valid", temp_dir.to_str().unwrap());
    session.agents.push(AgentInfo {
        id: "session-qa-valid-evaluator".to_string(),
        role: AgentRole::Evaluator,
        status: AgentStatus::Running,
        config: AgentConfig::default(),
        parent_id: None,
    });
    controller.read().insert_test_session(session);

    let body = serde_json::json!({
        "specialization": "a11y",
        "cli": "droid"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-qa-valid/qa-workers")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Valid QA worker request should pass handler validation and reach controller logic"
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// --- PersistedSession serde tests ---

#[test]
fn test_persisted_session_serializes_default_cli() {
    let session = PersistedSession {
        id: "test-session".to_string(),
        name: Some("Test Session".to_string()),
        color: Some("#7aa2f7".to_string()),
        session_type: SessionTypeInfo::Hive { worker_count: 2 },
        project_path: "/tmp/test".to_string(),
        created_at: chrono::Utc::now(),
        agents: vec![],
        state: "Running".to_string(),
        default_cli: "gemini".to_string(),
        default_model: Some("gemini-2.5-pro".to_string()),
        max_qa_iterations: 3,
        qa_timeout_secs: 300,
        auth_strategy: String::new(),
    };

    let json = serde_json::to_string(&session).unwrap();
    let deserialized: PersistedSession = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.default_cli, "gemini");
    assert_eq!(deserialized.default_model, Some("gemini-2.5-pro".to_string()));
    assert_eq!(deserialized.name, Some("Test Session".to_string()));
    assert_eq!(deserialized.color, Some("#7aa2f7".to_string()));
}

#[test]
fn test_persisted_session_legacy_json_defaults_to_claude() {
    // Legacy JSON without default_cli or default_model fields
    // should deserialize with serde(default) fallbacks
    let json = r#"{
        "id": "legacy-session",
        "session_type": {"Hive": {"worker_count": 3}},
        "project_path": "/tmp/legacy",
        "created_at": "2024-01-01T00:00:00Z",
        "agents": [],
        "state": "Completed"
    }"#;

    let session: PersistedSession = serde_json::from_str(json).unwrap();

    assert_eq!(
        session.default_cli, "claude",
        "Legacy sessions without default_cli should fallback to 'claude'"
    );
    assert_eq!(
        session.default_model, None,
        "Legacy sessions without default_model should fallback to None"
    );
    assert_eq!(session.name, None);
    assert_eq!(session.color, None);
    assert_eq!(session.max_qa_iterations, 3);
    assert_eq!(session.qa_timeout_secs, 300);
}

// --- Fusion mode smoke tests ---

#[tokio::test]
async fn test_launch_fusion_success() {
    let app = setup_test_app().await;

    let body = serde_json::json!({
        "project_path": std::env::temp_dir().to_str().unwrap(),
        "task_description": "Implement feature X",
        "variants": [
            { "name": "variant-a" },
            { "name": "variant-b" }
        ]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/fusion")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // May be 201 (success) or 500 (PTY spawn fails in test env), but NOT 400
    assert_ne!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_launch_fusion_empty_variants() {
    let app = setup_test_app().await;

    let body = serde_json::json!({
        "project_path": "/tmp/test",
        "task_description": "Implement feature X",
        "variants": []
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/fusion")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_launch_fusion_empty_task() {
    let app = setup_test_app().await;

    let body = serde_json::json!({
        "project_path": "/tmp/test",
        "task_description": "   ",
        "variants": [{ "name": "v1" }]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/fusion")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_launch_fusion_invalid_cli() {
    let app = setup_test_app().await;

    let body = serde_json::json!({
        "project_path": "/tmp/test",
        "task_description": "Implement feature",
        "variants": [{ "name": "v1" }],
        "default_cli": "evil-cli"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/fusion")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_launch_fusion_invalid_judge_cli() {
    let app = setup_test_app().await;

    let body = serde_json::json!({
        "project_path": "/tmp/test",
        "task_description": "Implement feature",
        "variants": [{ "name": "v1" }],
        "judge_cli": "malicious-judge"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/fusion")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_get_fusion_status_not_found() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/nonexistent/fusion/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_fusion_evaluation_not_found() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/nonexistent/fusion/evaluation")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_select_fusion_winner_not_found() {
    let app = setup_test_app().await;

    let body = serde_json::json!({ "variant": "variant-a" });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/nonexistent/fusion/select-winner")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should be 404 or 500 (session not found), not 200
    assert_ne!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_select_fusion_winner_empty_variant() {
    let app = setup_test_app().await;

    let body = serde_json::json!({ "variant": "  " });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/nonexistent/fusion/select-winner")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_fusion_status_path_traversal() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/..evil/fusion/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_fusion_evaluation_path_traversal() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/..evil/fusion/evaluation")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_select_winner_path_traversal() {
    let app = setup_test_app().await;

    let body = serde_json::json!({ "variant": "v1" });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/..evil/fusion/select-winner")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// --- Conversations API tests ---

#[tokio::test]
async fn test_append_conversation_and_verify_file_content() {
    let (app, controller) = setup_test_app_with_controller().await;
    let storage = SessionStorage::new().unwrap();
    let session_id = format!("conv-append-{}", uuid::Uuid::new_v4());

    let temp_dir = std::env::temp_dir().join(format!("hive-test-{}", session_id));
    let _ = std::fs::create_dir_all(&temp_dir);
    controller
        .read()
        .insert_test_session(make_test_session(&session_id, temp_dir.to_str().unwrap()));

    let body = serde_json::json!({
        "from": "worker-1",
        "content": "First conversation message"
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{}/conversations/worker-1/append", session_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let conversation_file = storage
        .session_dir(&session_id)
        .join("conversations")
        .join("worker-1.md");
    let file_content = std::fs::read_to_string(conversation_file).unwrap();
    assert!(file_content.contains("from @worker-1"));
    assert!(file_content.contains("First conversation message"));

    let _ = std::fs::remove_dir_all(storage.session_dir(&session_id));
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_read_conversation_since_filter() {
    let (app, controller) = setup_test_app_with_controller().await;
    let session_id = format!("conv-since-{}", uuid::Uuid::new_v4());

    let temp_dir = std::env::temp_dir().join(format!("hive-test-{}", session_id));
    let _ = std::fs::create_dir_all(&temp_dir);
    controller
        .read()
        .insert_test_session(make_test_session(&session_id, temp_dir.to_str().unwrap()));

    let body_1 = serde_json::json!({
        "from": "queen",
        "content": "Before marker"
    });
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{}/conversations/shared/append", session_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body_1).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let marker = chrono::Utc::now().to_rfc3339();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let body_2 = serde_json::json!({
        "from": "worker-1",
        "content": "After marker"
    });
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{}/conversations/shared/append", session_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body_2).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/sessions/{}/conversations/shared?since={}",
                    session_id, marker
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let messages = response_json.get("messages").unwrap().as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].get("from").unwrap().as_str().unwrap(), "worker-1");
    assert_eq!(
        messages[0].get("content").unwrap().as_str().unwrap(),
        "After marker"
    );

    let storage = SessionStorage::new().unwrap();
    let _ = std::fs::remove_dir_all(storage.session_dir(&session_id));
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_conversation_rejects_path_traversal_and_invalid_agent_id() {
    let app = setup_test_app().await;
    let body = serde_json::json!({
        "from": "worker-1",
        "content": "test"
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/..evil/conversations/worker-1/append")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-safe/conversations/worker 1/append")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_conversation_concurrent_appends() {
    let (app, controller) = setup_test_app_with_controller().await;
    let session_id = format!("conv-concurrent-{}", uuid::Uuid::new_v4());

    let temp_dir = std::env::temp_dir().join(format!("hive-test-{}", session_id));
    let _ = std::fs::create_dir_all(&temp_dir);
    controller
        .read()
        .insert_test_session(make_test_session(&session_id, temp_dir.to_str().unwrap()));

    let mut handles = Vec::new();
    for i in 0..5 {
        let app_clone = app.clone();
        let session_id_clone = session_id.clone();
        handles.push(tokio::spawn(async move {
            let body = serde_json::json!({
                "from": format!("worker-{}", i + 1),
                "content": format!("message-{}", i + 1)
            });
            app_clone
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/sessions/{}/conversations/shared/append", session_id_clone))
                        .header("content-type", "application/json")
                        .body(Body::from(serde_json::to_string(&body).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }));
    }

    for handle in handles {
        let status = handle.await.unwrap();
        assert_eq!(status, StatusCode::CREATED);
    }

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/sessions/{}/conversations/shared", session_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let messages = response_json.get("messages").unwrap().as_array().unwrap();
    assert_eq!(messages.len(), 5);

    let storage = SessionStorage::new().unwrap();
    let _ = std::fs::remove_dir_all(storage.session_dir(&session_id));
    let _ = std::fs::remove_dir_all(&temp_dir);
}

struct TestPathCleanup {
    paths: Vec<PathBuf>,
}

impl TestPathCleanup {
    fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }
}

impl Drop for TestPathCleanup {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[tokio::test]
async fn test_list_cells_returns_primary_cell_for_hive_session() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-cells-primary");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session_with_agents("session-cells", temp_dir.to_str().unwrap(), &["session-cells-queen", "session-cells-worker-1"]),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-cells/cells")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let cells = response_json.as_array().unwrap();
    assert_eq!(cells.len(), 1);
    assert_eq!(cells[0].get("id").unwrap().as_str().unwrap(), "primary");
    assert_eq!(cells[0].get("session_id").unwrap().as_str().unwrap(), "session-cells");
    assert_eq!(cells[0].get("status").unwrap().as_str().unwrap(), "running");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_get_cell_rejects_invalid_cell_id() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-safe/cells/../bad")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_stop_cell_returns_bad_request() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-stop-cell");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller
        .read()
        .insert_test_session(make_test_session("session-stop-cell", temp_dir.to_str().unwrap()));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/sessions/session-stop-cell/cells/primary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let session = controller.read().get_session("session-stop-cell").unwrap();
    assert!(matches!(session.state, SessionState::Running));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_list_agents_in_cell_returns_session_agents() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-cell-agents");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session_with_agents("session-cell-agents", temp_dir.to_str().unwrap(), &["worker-1", "worker-2"]),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-cell-agents/cells/primary/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let agents = response_json.as_array().unwrap();
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0].get("cell_id").unwrap().as_str().unwrap(), "primary");
    assert_eq!(agents[0].get("status").unwrap().as_str().unwrap(), "running");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_list_agents_in_cell_rejects_invalid_cell_id() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-safe/cells/../bad/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_list_artifacts_returns_empty_for_synthetic_cell() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-cell-artifacts");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller
        .read()
        .insert_test_session(make_test_session("session-cell-artifacts", temp_dir.to_str().unwrap()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-cell-artifacts/cells/primary/artifacts")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(response_json.as_array().unwrap().len(), 0);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_list_artifacts_uses_persisted_session_fallback() {
    let app = setup_test_app().await;
    let session_id = format!("persisted-artifacts-{}", uuid::Uuid::new_v4());
    let temp_dir = TempDir::new().unwrap();
    let storage = SessionStorage::new().unwrap();
    let _cleanup = TestPathCleanup::new(vec![storage.session_dir(&session_id)]);

    storage
        .save_session(&PersistedSession {
            id: session_id.clone(),
            name: Some("Persisted Session".to_string()),
            color: None,
            session_type: SessionTypeInfo::Hive { worker_count: 1 },
            project_path: temp_dir.path().to_string_lossy().to_string(),
            created_at: chrono::Utc::now(),
            agents: vec![],
            state: "Completed".to_string(),
            default_cli: "claude".to_string(),
            default_model: Some("opus-4-6".to_string()),
            max_qa_iterations: 3,
            qa_timeout_secs: 300,
            auth_strategy: String::new(),
        })
        .unwrap();
    storage
        .save_artifact(
            &session_id,
            "primary",
            &crate::domain::ArtifactBundle {
                summary: Some("Persisted artifact".to_string()),
                changed_files: vec!["src/main.rs".to_string()],
                commits: vec!["abc123 persisted".to_string()],
                branch: "feature/persisted".to_string(),
                test_results: None,
                diff_summary: None,
                unresolved_issues: vec![],
                confidence: None,
                recommended_next_step: None,
            },
        )
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/sessions/{}/cells/primary/artifacts", session_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let artifacts = response_json.as_array().unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(
        artifacts[0].get("branch").unwrap().as_str().unwrap(),
        "feature/persisted"
    );
}

#[tokio::test]
async fn test_list_artifacts_rejects_invalid_cell_id() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/session-safe/cells/../bad/artifacts")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_post_artifact_round_trip_and_cell_projection() {
    let (app, controller) = setup_test_app_with_controller().await;
    let session_id = format!("artifact-roundtrip-{}", uuid::Uuid::new_v4());
    let temp_dir = TempDir::new().unwrap();
    let storage = SessionStorage::new().unwrap();
    let _cleanup = TestPathCleanup::new(vec![storage.session_dir(&session_id)]);

    controller
        .read()
        .insert_test_session(make_test_session(
            &session_id,
            temp_dir.path().to_str().unwrap(),
        ));

    let artifact = serde_json::json!({
        "artifact": {
            "summary": "Primary cell summary",
            "changed_files": ["src/main.rs"],
            "commits": ["abc123 add resolver endpoint"],
            "branch": "feature/artifacts",
            "test_results": { "passed": 3, "failed": 0 },
            "diff_summary": "1 file changed",
            "unresolved_issues": [],
            "confidence": 0.82,
            "recommended_next_step": "Open comparison view"
        }
    });

    let post_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{}/cells/primary/artifacts", session_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&artifact).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(post_response.status(), StatusCode::CREATED);

    let get_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/sessions/{}/cells/primary/artifacts", session_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(get_response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(get_response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let artifacts = response_json.as_array().unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].get("branch").unwrap().as_str().unwrap(), "feature/artifacts");

    let cell_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/sessions/{}/cells/primary", session_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(cell_response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(cell_response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        response_json
            .get("artifacts")
            .unwrap()
            .get("summary")
            .unwrap()
            .as_str()
            .unwrap(),
        "Primary cell summary"
    );
}

#[tokio::test]
async fn test_get_resolver_output_endpoint_returns_persisted_output() {
    let (app, controller) = setup_test_app_with_controller().await;
    let session_id = format!("resolver-output-{}", uuid::Uuid::new_v4());
    let temp_dir = TempDir::new().unwrap();
    let storage = SessionStorage::new().unwrap();
    let _cleanup = TestPathCleanup::new(vec![storage.session_dir(&session_id)]);

    controller
        .read()
        .insert_test_session(make_test_session(
            &session_id,
            temp_dir.path().to_str().unwrap(),
        ));

    storage
        .save_resolver_output(
            &session_id,
            &crate::domain::ResolverOutput {
                selected_candidate: "variant-b".to_string(),
                rationale: "Better test coverage".to_string(),
                tradeoffs: vec!["More files changed".to_string()],
                hybrid_integration_plan: None,
                final_recommendation: Some("Merge variant-b".to_string()),
            },
        )
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/sessions/{}/resolver", session_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        response_json
            .get("selected_candidate")
            .unwrap()
            .as_str()
            .unwrap(),
        "variant-b"
    );
}

#[tokio::test]
async fn test_get_resolver_output_returns_internal_error_for_invalid_persisted_session() {
    let app = setup_test_app().await;
    let session_id = format!("resolver-invalid-session-{}", uuid::Uuid::new_v4());
    let storage = SessionStorage::new().unwrap();
    let _cleanup = TestPathCleanup::new(vec![storage.session_dir(&session_id)]);

    std::fs::create_dir_all(storage.session_dir(&session_id)).unwrap();
    std::fs::write(
        storage.session_dir(&session_id).join("session.json"),
        "{ invalid json",
    )
    .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/sessions/{}/resolver", session_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn test_template_crud_endpoints() {
    let app = setup_test_app().await;
    let template_id = format!("user-template-{}", uuid::Uuid::new_v4().simple());

    let template = serde_json::json!({
        "id": template_id,
        "name": "Custom Fusion",
        "description": "User-defined template",
        "mode": "fusion",
        "cells": [
            {
                "role": "candidate-a",
                "cli": "codex",
                "model": "gpt-5.4",
                "prompt_template": "fusion-worker"
            }
        ],
        "workspace_strategy": "isolated_cell",
        "is_builtin": false
    });

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/templates")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&template).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_response.status(), StatusCode::CREATED);

    let get_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/templates/{}", template_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);

    let list_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/templates")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(list_response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(response_json
        .get("templates")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item.get("id").unwrap().as_str().unwrap() == template_id));
    assert!(response_json
        .get("role_packs")
        .unwrap()
        .as_array()
        .unwrap()
        .len()
        >= 4);

    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/templates/{}", template_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    let get_deleted_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/templates/{}", template_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_deleted_response.status(), StatusCode::NOT_FOUND);

    let storage = SessionStorage::new().unwrap();
    let _ = storage.delete_user_template(&template_id);
}

#[tokio::test]
async fn test_send_agent_input_rejects_empty_input() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-agent-input");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session_with_agents("session-agent-input", temp_dir.to_str().unwrap(), &["worker-1"]),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-agent-input/agents/worker-1/input")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"input":"   "}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_send_agent_input_rejects_invalid_agent_id() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-safe/agents/../bad/input")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"input":"hello"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_create_session_rejects_invalid_mode() {
    let app = setup_test_app().await;
    let temp_dir = std::env::temp_dir().join("hive-test-create-session-invalid-mode");
    let _ = std::fs::create_dir_all(&temp_dir);

    let body = serde_json::json!({
        "project_path": temp_dir.to_str().unwrap(),
        "mode": "solo",
        "objective": "test objective"
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_launch_session_returns_not_implemented() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-launch/launch")
                .header("content-type", "application/json")
                .body(Body::from(r#"{}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// --- Heartbeat endpoint tests ---

#[tokio::test]
async fn test_post_heartbeat_updates_timestamp() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-heartbeat");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session_with_agents("session-hb", temp_dir.to_str().unwrap(), &["worker-1"]),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-hb/heartbeat")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"agent_id":"worker-1","status":"working","summary":"3/5 files done"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_post_heartbeat_rejects_invalid_status() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-heartbeat-invalid");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session_with_agents("session-hb-inv", temp_dir.to_str().unwrap(), &["worker-1"]),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-hb-inv/heartbeat")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"agent_id":"worker-1","status":"invalid","summary":"x"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_get_active_sessions_returns_only_running() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-active");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session_with_agents("session-running", temp_dir.to_str().unwrap(), &["worker-1"]),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/active")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let sessions = response_json.get("sessions").unwrap().as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].get("id").unwrap().as_str().unwrap(), "session-running");
    let agents = sessions[0].get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents.len(), 1);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_get_active_sessions_includes_heartbeat_after_post() {
    let (app, controller) = setup_test_app_with_controller().await;

    let temp_dir = std::env::temp_dir().join("hive-test-active-hb");
    let _ = std::fs::create_dir_all(&temp_dir);

    controller.read().insert_test_session(
        make_test_session_with_agents("session-active-hb", temp_dir.to_str().unwrap(), &["worker-1"]),
    );

    // POST heartbeat first
    let _ = app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/session-active-hb/heartbeat")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"agent_id":"worker-1","status":"working","summary":"2/3 done"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    // GET active sessions
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/active")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let sessions = response_json.get("sessions").unwrap().as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    let agents = sessions[0].get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents.len(), 1);
    assert!(agents[0].get("last_activity").unwrap().as_str().is_some());
    assert_eq!(agents[0].get("status").unwrap().as_str().unwrap(), "working");
    assert_eq!(agents[0].get("summary").unwrap().as_str().unwrap(), "2/3 done");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// --- SSE Event Endpoint Tests ---

#[tokio::test]
async fn test_get_events_returns_empty_for_new_session() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/new-session/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return OK with empty array for non-existent session
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let response_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let events = response_json.as_array().unwrap();
    assert_eq!(events.len(), 0);
}

#[tokio::test]
async fn test_get_events_rejects_path_traversal_session_id() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/../../../etc/passwd/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_stream_events_endpoint_exists() {
    let app = setup_test_app().await;

    // Just verify the endpoint exists and returns SSE content type
    // Note: Testing actual SSE streaming in unit tests is complex
    // This test verifies the endpoint is accessible
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/test-session/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // SSE endpoint should return OK (it will keep connection open)
    // The response should have content-type: text/event-stream
    assert_eq!(response.status(), StatusCode::OK);
    
    let content_type = response.headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(content_type.contains("text/event-stream") || content_type.contains("event-stream"));
}

#[tokio::test]
async fn test_stream_events_rejects_path_traversal_session_id() {
    let app = setup_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/sessions/../../etc/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ── Resolver launch endpoint tests ──────────────────────────────────────

fn make_fusion_session(id: &str, project_path: &str) -> Session {
    Session {
        id: id.to_string(),
        name: None,
        color: None,
        session_type: SessionType::Fusion { variants: vec!["variant-a".to_string(), "variant-b".to_string()] },
        project_path: PathBuf::from(project_path),
        state: SessionState::Running,
        created_at: chrono::Utc::now(),
        agents: vec![],
        default_cli: "claude".to_string(),
        default_model: Some("opus-4-6".to_string()),
        max_qa_iterations: 3,
        qa_timeout_secs: 300,
        auth_strategy: AuthStrategy::default(),
    }
}

#[tokio::test]
async fn test_resolver_launch_success_with_artifacts() {
    let (app, controller) = setup_test_app_with_controller().await;
    let storage = SessionStorage::new().unwrap();
    let session_id = format!("resolver-launch-{}", uuid::Uuid::new_v4());

    // Create session dir and artifacts
    storage.create_session_dir(&session_id).unwrap();
    storage
        .save_artifact(
            &session_id,
            "variant-a",
            &crate::domain::ArtifactBundle {
                summary: Some("Variant A impl".to_string()),
                changed_files: vec!["src/a.rs".to_string()],
                commits: vec!["a1".to_string()],
                branch: "fusion/a".to_string(),
                test_results: None,
                diff_summary: None,
                unresolved_issues: vec![],
                confidence: Some(0.9),
                recommended_next_step: None,
            },
        )
        .unwrap();
    storage
        .save_artifact(
            &session_id,
            "variant-b",
            &crate::domain::ArtifactBundle {
                summary: Some("Variant B impl".to_string()),
                changed_files: vec!["src/b.rs".to_string()],
                commits: vec!["b1".to_string()],
                branch: "fusion/b".to_string(),
                test_results: None,
                diff_summary: None,
                unresolved_issues: vec!["todo".to_string()],
                confidence: Some(0.5),
                recommended_next_step: None,
            },
        )
        .unwrap();

    // Register Fusion session in controller
    let session = make_fusion_session(&session_id, &std::env::temp_dir().to_string_lossy());
    controller.write().insert_test_session(session);

    let body = serde_json::json!({
        "candidate_ids": ["variant-a", "variant-b"]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{}/resolver/launch", session_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let output: crate::domain::ResolverOutput = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(output.selected_candidate, "variant-a");
    assert!(!output.rationale.is_empty());

    let session = controller.read().get_session(&session_id).unwrap();
    assert_eq!(session.state, SessionState::Completed);

    // Verify output was persisted
    let persisted_output = storage.load_resolver_output(&session_id).unwrap();
    assert!(persisted_output.is_some());

    let _ = std::fs::remove_dir_all(storage.session_dir(&session_id));
}

#[tokio::test]
async fn test_resolver_launch_missing_session_returns_404() {
    let app = setup_test_app().await;

    let body = serde_json::json!({
        "candidate_ids": ["variant-a"]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/nonexistent-session-id/resolver/launch")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_resolver_launch_empty_candidates_returns_400() {
    let (app, controller) = setup_test_app_with_controller().await;
    let session_id = format!("resolver-empty-{}", uuid::Uuid::new_v4());

    let session = make_fusion_session(&session_id, &std::env::temp_dir().to_string_lossy());
    controller.write().insert_test_session(session);

    let body = serde_json::json!({
        "candidate_ids": []
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{}/resolver/launch", session_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_resolver_launch_rejects_duplicate_candidate_ids() {
    let (app, controller) = setup_test_app_with_controller().await;
    let session_id = format!("resolver-duplicate-{}", uuid::Uuid::new_v4());

    let session = make_fusion_session(&session_id, &std::env::temp_dir().to_string_lossy());
    controller.write().insert_test_session(session);

    let body = serde_json::json!({
        "candidate_ids": ["variant-a", "variant-a"]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{}/resolver/launch", session_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_resolver_launch_rejects_unknown_candidate_ids() {
    let (app, controller) = setup_test_app_with_controller().await;
    let session_id = format!("resolver-unknown-{}", uuid::Uuid::new_v4());

    let session = make_fusion_session(&session_id, &std::env::temp_dir().to_string_lossy());
    controller.write().insert_test_session(session);

    let body = serde_json::json!({
        "candidate_ids": ["variant-a", "variant-c"]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{}/resolver/launch", session_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_resolver_launch_non_fusion_session_returns_400() {
    let (app, controller) = setup_test_app_with_controller().await;
    let session_id = format!("resolver-hive-{}", uuid::Uuid::new_v4());

    let session = make_test_session(&session_id, &std::env::temp_dir().to_string_lossy());
    controller.write().insert_test_session(session);

    let body = serde_json::json!({
        "candidate_ids": ["variant-a"]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/sessions/{}/resolver/launch", session_id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
