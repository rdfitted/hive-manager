use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;
use crate::http::routes::create_router;
use crate::http::state::AppState;
use crate::storage::SessionStorage;
use crate::pty::PtyManager;
use crate::session::{Session, SessionController, SessionState, SessionType};
use crate::coordination::InjectionManager;
use parking_lot::RwLock;

async fn setup_test_app() -> axum::Router {
    let storage = Arc::new(SessionStorage::new().unwrap());
    let config = Arc::new(tokio::sync::RwLock::new(storage.load_config().unwrap()));
    let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
    let session_controller = Arc::new(RwLock::new(SessionController::new(pty_manager.clone())));
    let injection_manager = Arc::new(RwLock::new(InjectionManager::new(
        pty_manager.clone(),
        SessionStorage::new().unwrap(),
    )));

    let state = Arc::new(AppState::new(
        config,
        pty_manager,
        session_controller,
        injection_manager,
        storage,
    ));

    create_router(state)
}

/// Setup test app and return both the router and session controller for inserting test sessions
async fn setup_test_app_with_controller() -> (axum::Router, Arc<RwLock<SessionController>>) {
    let storage = Arc::new(SessionStorage::new().unwrap());
    let config = Arc::new(tokio::sync::RwLock::new(storage.load_config().unwrap()));
    let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
    let session_controller = Arc::new(RwLock::new(SessionController::new(pty_manager.clone())));
    let injection_manager = Arc::new(RwLock::new(InjectionManager::new(
        pty_manager.clone(),
        SessionStorage::new().unwrap(),
    )));

    let state = Arc::new(AppState::new(
        config,
        pty_manager,
        session_controller.clone(),
        injection_manager,
        storage,
    ));

    (create_router(state), session_controller)
}

fn make_test_session(id: &str, project_path: &str) -> Session {
    Session {
        id: id.to_string(),
        session_type: SessionType::Hive { worker_count: 1 },
        project_path: PathBuf::from(project_path),
        state: SessionState::Running,
        created_at: chrono::Utc::now(),
        agents: vec![],
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
