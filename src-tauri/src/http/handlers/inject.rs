use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use super::{validate_agent_id, validate_session_id};
use crate::coordination::InjectionError;
use crate::http::error::ApiError;
use crate::http::state::AppState;

fn default_submit() -> bool {
    true
}

#[derive(Deserialize)]
pub struct OperatorInjectRequest {
    pub target_agent_id: String,
    pub message: String,
    #[serde(default = "default_submit")]
    pub submit: bool,
}

#[derive(Deserialize)]
pub struct QueenInjectRequest {
    pub queen_id: String,
    pub target_worker_id: String,
    pub message: String,
    #[serde(default = "default_submit")]
    pub submit: bool,
}

#[derive(Deserialize)]
pub struct EvaluatorInjectRequest {
    pub evaluator_id: String,
    pub target_agent_id: String,
    pub message: String,
    #[serde(default = "default_submit")]
    pub submit: bool,
}

pub async fn operator_inject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<OperatorInjectRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&id)?;
    validate_agent_id(&payload.target_agent_id)?;

    let manager = Arc::clone(&state.injection_manager);
    let injection_session_id = id.clone();
    let injection_result = tokio::task::spawn_blocking(move || {
        manager.read().operator_inject(
            &injection_session_id,
            &payload.target_agent_id,
            &payload.message,
            payload.submit,
        )
    })
    .await
    .map_err(|error| ApiError::internal(format!("Injection task failed: {error}")))?;
    injection_result.map_err(|error| ApiError::internal(error.to_string()))?;

    Ok(Json(json!({
        "status": "success",
        "message": format!("Operator injection sent to session {}", id)
    })))
}

pub async fn queen_inject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<QueenInjectRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&id)?;
    validate_agent_id(&payload.queen_id)?;
    validate_agent_id(&payload.target_worker_id)?;

    let manager = Arc::clone(&state.injection_manager);
    let injection_session_id = id.clone();
    let injection_result = tokio::task::spawn_blocking(move || {
        manager.read().queen_inject(
            &injection_session_id,
            &payload.queen_id,
            &payload.target_worker_id,
            &payload.message,
            payload.submit,
        )
    })
    .await
    .map_err(|error| ApiError::internal(format!("Injection task failed: {error}")))?;
    injection_result.map_err(map_injection_error)?;

    Ok(Json(json!({
        "status": "success",
        "message": format!("Queen injection sent to session {}", id)
    })))
}

pub async fn evaluator_inject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<EvaluatorInjectRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&id)?;
    validate_agent_id(&payload.evaluator_id)?;
    validate_agent_id(&payload.target_agent_id)?;

    let manager = Arc::clone(&state.injection_manager);
    let injection_session_id = id.clone();
    let injection_result = tokio::task::spawn_blocking(move || {
        manager.read().evaluator_inject(
            &injection_session_id,
            &payload.evaluator_id,
            &payload.target_agent_id,
            &payload.message,
            payload.submit,
        )
    })
    .await
    .map_err(|error| ApiError::internal(format!("Injection task failed: {error}")))?;
    injection_result.map_err(map_injection_error)?;

    Ok(Json(json!({
        "status": "success",
        "message": format!("Evaluator injection sent to session {}", id)
    })))
}

fn map_injection_error(error: InjectionError) -> ApiError {
    match error {
        InjectionError::NotAuthorized(message) => {
            ApiError::new(axum::http::StatusCode::FORBIDDEN, message)
        }
        InjectionError::AgentNotFound(message) | InjectionError::SessionNotFound(message) => {
            ApiError::not_found(message)
        }
        other => ApiError::internal(other.to_string()),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::coordination::{InjectionManager, QueueManager};
    use crate::events::EventBus;
    use crate::pty::{AgentRole, PtyManager};
    use crate::session::SessionController;
    use crate::storage::{ApplicationStateDb, QueueRepo, SessionStorage};
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::post,
        Router,
    };
    use parking_lot::RwLock;
    use serde_json::{json, Value};
    use tempfile::TempDir;
    use tower::ServiceExt;

    const AGENT_ID: &str = "inject-test-worker-1";

    fn setup_test_app() -> (TempDir, Router, Arc<AppState>) {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(
            SessionStorage::new_with_base(temp_dir.path().to_path_buf()).unwrap(),
        );
        let config = Arc::new(tokio::sync::RwLock::new(storage.load_config().unwrap()));
        let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
        pty_manager
            .write()
            .create_session(
                AGENT_ID.to_string(),
                AgentRole::Worker {
                    index: 1,
                    parent: None,
                },
                "claude",
                &[],
                None,
                80,
                24,
            )
            .unwrap();
        let session_controller = Arc::new(RwLock::new(SessionController::new(
            pty_manager.clone(),
        )));
        session_controller.write().set_storage(storage.clone());
        let injection_manager = Arc::new(RwLock::new(InjectionManager::new(
            pty_manager.clone(),
            SessionStorage::new_with_base(temp_dir.path().to_path_buf()).unwrap(),
        )));
        let event_bus = EventBus::new(storage.base_dir().clone());
        let app_state_db = Arc::new(ApplicationStateDb::open_in_memory().unwrap());
        let queue_repo = Arc::new(QueueRepo::new(app_state_db.clone()));
        queue_repo.ensure_schema().unwrap();
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
        let app = Router::new()
            .route("/api/sessions/{id}/inject", post(operator_inject))
            .with_state(state.clone());

        (temp_dir, app, state)
    }

    async fn post_operator_inject(app: Router, body: Value) -> StatusCode {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sessions/inject-test/inject")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
    }

    #[tokio::test]
    async fn operator_inject_omitted_submit_defaults_to_true() {
        let (_temp_dir, app, state) = setup_test_app();

        let status = post_operator_inject(
            app,
            json!({ "target_agent_id": AGENT_ID, "message": "hello\r\n" }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            state
                .pty_manager
                .read()
                .write_records_for_test(AGENT_ID)
                .unwrap(),
            vec![b"hello".to_vec(), b"\r".to_vec()]
        );
    }

    #[tokio::test]
    async fn operator_inject_submit_false_writes_payload_without_enter() {
        let (_temp_dir, app, state) = setup_test_app();

        let status = post_operator_inject(
            app,
            json!({
                "target_agent_id": AGENT_ID,
                "message": "draft\r\n",
                "submit": false
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            state
                .pty_manager
                .read()
                .write_records_for_test(AGENT_ID)
                .unwrap(),
            vec![b"draft".to_vec()]
        );
    }

    #[tokio::test]
    async fn operator_inject_multiline_preserves_newlines_and_submits_once() {
        let (_temp_dir, app, state) = setup_test_app();

        let status = post_operator_inject(
            app,
            json!({
                "target_agent_id": AGENT_ID,
                "message": "line one\nline two\r\n"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let writes = state
            .pty_manager
            .read()
            .write_records_for_test(AGENT_ID)
            .unwrap();
        assert_eq!(writes, vec![b"line one\nline two".to_vec(), b"\r".to_vec()]);
        assert_eq!(writes.iter().filter(|write| write.as_slice() == b"\r").count(), 1);
        assert!(!writes[1].contains(&b'\n'));
    }

    #[tokio::test]
    async fn operator_inject_blocking_path_preserves_pty_error_status() {
        let (_temp_dir, app, _state) = setup_test_app();

        let status = post_operator_inject(
            app,
            json!({ "target_agent_id": "missing-agent", "message": "hello" }),
        )
        .await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
