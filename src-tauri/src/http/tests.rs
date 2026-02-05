use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::Arc;
use tower::ServiceExt;
use crate::http::routes::create_router;
use crate::http::state::AppState;
use crate::storage::SessionStorage;
use crate::pty::PtyManager;
use crate::session::SessionController;
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
