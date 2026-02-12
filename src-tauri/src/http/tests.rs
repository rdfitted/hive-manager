use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;
use crate::http::routes::create_router;
use crate::http::state::AppState;
use crate::storage::{SessionStorage, PersistedSession, SessionTypeInfo};
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
        default_cli: "claude".to_string(),
        default_model: Some("opus-4-6".to_string()),
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

// --- PersistedSession serde tests ---

#[test]
fn test_persisted_session_serializes_default_cli() {
    let session = PersistedSession {
        id: "test-session".to_string(),
        session_type: SessionTypeInfo::Hive { worker_count: 2 },
        project_path: "/tmp/test".to_string(),
        created_at: chrono::Utc::now(),
        agents: vec![],
        state: "Running".to_string(),
        default_cli: "gemini".to_string(),
        default_model: Some("gemini-2.5-pro".to_string()),
    };

    let json = serde_json::to_string(&session).unwrap();
    let deserialized: PersistedSession = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.default_cli, "gemini");
    assert_eq!(deserialized.default_model, Some("gemini-2.5-pro".to_string()));
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
