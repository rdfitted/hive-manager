use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{validate_agent_id, validate_session_id};
use crate::coordination::{InjectionError, InjectionReceipt};
use crate::http::error::ApiError;
use crate::http::state::AppState;

fn default_submit() -> bool {
    true
}

const INJECTION_EVIDENCE_SCOPE: &str = "These facts prove only that bytes were written and the PTY output ring was observed for a bounded window; they do not prove that the agent took a turn. submit_confirmed is heuristic: true means sustained post-submit ring activity consistent with the composer accepting Enter, false means Enter produced no observable reaction within the confirmation window, null means unknown or not applicable. An agent that was already streaming output can produce a false positive.";
const INJECTION_ACTIVITY_OBSERVATION_WINDOW: Duration = Duration::from_millis(250);
const INJECTION_ACTIVITY_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// How long the sender watches the ring for delivery evidence after the Enter write (#256).
const SUBMIT_CONFIRMATION_WINDOW: Duration = Duration::from_millis(1500);
/// Minimum spread between the first and last observed ring change for the activity to
/// count as sustained rather than a single composer repaint.
const SUBMIT_CONFIRMATION_MIN_SPAN: Duration = Duration::from_millis(250);

struct InjectionObservation {
    pty_observation_available: bool,
    pty_output_bytes_after: Option<usize>,
    pty_activity_observed: Option<bool>,
    observation_window_ms: u64,
    observation_elapsed_ms: u64,
    submit_confirmed: Option<bool>,
    submit_confirmation_basis: &'static str,
    submit_confirmation_window_ms: u64,
    submit_confirmation_elapsed_ms: u64,
}

impl InjectionObservation {
    fn unobserved(
        pty_observation_available: bool,
        pty_output_bytes_after: Option<usize>,
        submit_confirmation_basis: &'static str,
    ) -> Self {
        Self {
            pty_observation_available,
            pty_output_bytes_after,
            pty_activity_observed: None,
            observation_window_ms: 0,
            observation_elapsed_ms: 0,
            submit_confirmed: None,
            submit_confirmation_basis,
            submit_confirmation_window_ms: 0,
            submit_confirmation_elapsed_ms: 0,
        }
    }
}

/// Classify post-Enter ring behaviour into the tri-state delivery signal (#256).
///
/// `change_offsets` holds the elapsed time of every poll at which the ring content
/// differed from the previous poll. Changes spread over at least
/// [`SUBMIT_CONFIRMATION_MIN_SPAN`] are the signature of a composer that accepted Enter
/// and started a turn. A short isolated burst is ambiguous — a swallowed Enter also
/// repaints the composer once. Zero changes mean the Enter provably produced no visible
/// reaction, which a live TUI never does for an accepted submit.
fn classify_submit_confirmation(change_offsets: &[Duration]) -> (Option<bool>, &'static str) {
    match change_offsets {
        [] => (Some(false), "no-post-submit-activity"),
        [first, .., last]
            if last.saturating_sub(*first) >= SUBMIT_CONFIRMATION_MIN_SPAN =>
        {
            (Some(true), "sustained-post-submit-activity")
        }
        _ => (None, "ambiguous-post-submit-activity"),
    }
}

async fn observe_injection(
    state: &AppState,
    agent_id: &str,
    receipt: &InjectionReceipt,
) -> InjectionObservation {
    if !receipt.submit_keystroke_issued {
        return InjectionObservation::unobserved(
            receipt.pty_output_after_write.is_some(),
            receipt.pty_output_after_write.as_ref().map(String::len),
            "not-submitted",
        );
    }

    let Some(baseline) = receipt.pty_output_after_write.as_ref() else {
        return InjectionObservation::unobserved(false, None, "pty-unobservable");
    };

    let observation_started = Instant::now();
    let confirmation_deadline = observation_started + SUBMIT_CONFIRMATION_WINDOW;
    let mut previous = baseline.clone();
    let mut output_after = Some(baseline.clone());
    let mut first_activity_elapsed: Option<Duration> = None;
    let mut change_offsets: Vec<Duration> = Vec::new();
    let mut ring_lost = false;

    loop {
        let remaining = confirmation_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(INJECTION_ACTIVITY_POLL_INTERVAL.min(remaining)).await;
        let current = state.pty_manager.read().recent_output(agent_id);
        let Some(current) = current else {
            output_after = None;
            ring_lost = true;
            break;
        };
        let elapsed = observation_started.elapsed();
        if current != previous {
            change_offsets.push(elapsed);
            if first_activity_elapsed.is_none() {
                first_activity_elapsed = Some(elapsed);
            }
            previous = current.clone();
        }
        output_after = Some(current);
        // Once the sustained-activity criterion is met, the verdict cannot change.
        if classify_submit_confirmation(&change_offsets).0 == Some(true) {
            break;
        }
    }

    let (submit_confirmed, submit_confirmation_basis) = if ring_lost {
        (None, "pty-unobservable")
    } else {
        classify_submit_confirmation(&change_offsets)
    };

    // The legacy activity fields keep their pre-#256 meaning: did the ring change within
    // the first 250 ms, and when was that change first seen.
    let activity_within_window = first_activity_elapsed
        .is_some_and(|elapsed| elapsed <= INJECTION_ACTIVITY_OBSERVATION_WINDOW);
    let observation_elapsed = match first_activity_elapsed {
        Some(elapsed) if activity_within_window => elapsed,
        _ => INJECTION_ACTIVITY_OBSERVATION_WINDOW.min(observation_started.elapsed()),
    };
    let observation_available = output_after.is_some();
    InjectionObservation {
        pty_observation_available: observation_available,
        pty_output_bytes_after: output_after.as_ref().map(String::len),
        pty_activity_observed: observation_available.then_some(activity_within_window),
        observation_window_ms: INJECTION_ACTIVITY_OBSERVATION_WINDOW.as_millis() as u64,
        observation_elapsed_ms: observation_elapsed.as_millis() as u64,
        submit_confirmed,
        submit_confirmation_basis,
        submit_confirmation_window_ms: SUBMIT_CONFIRMATION_WINDOW.as_millis() as u64,
        submit_confirmation_elapsed_ms: observation_started.elapsed().as_millis() as u64,
    }
}

fn record_injection_submission(
    state: &AppState,
    session_id: &str,
    agent_id: &str,
    receipt: &InjectionReceipt,
) {
    if let Err(error) = state
        .session_controller
        .read()
        .record_injection_submission(
            session_id,
            agent_id,
            receipt.submitted_at,
            receipt.submit_keystroke_issued,
        )
    {
        tracing::warn!(
            "Injection succeeded but its awaiting-turn observation was not recorded for {session_id}/{agent_id}: {error}"
        );
    }
}

fn measured_response(
    message: String,
    receipt: &InjectionReceipt,
    observation: &InjectionObservation,
) -> Value {
    json!({
        "status": "success",
        "message": message,
        "write_started_at": receipt.write_started_at,
        "submitted_at": receipt.submitted_at,
        "payload_bytes_written": receipt.payload_bytes_written,
        "submit_attempted": receipt.submit_keystroke_issued,
        "submit_keystroke_issued": receipt.submit_keystroke_issued,
        "submit_bytes_written": receipt.submit_bytes_written,
        "pty_observation_available": observation.pty_observation_available,
        "pty_output_bytes_before": receipt.pty_output_before.as_ref().map(String::len),
        "pty_output_bytes_after_write": receipt.pty_output_after_write.as_ref().map(String::len),
        "pty_output_bytes_after": observation.pty_output_bytes_after,
        "pty_activity_observed": observation.pty_activity_observed,
        "observation_window_ms": observation.observation_window_ms,
        "observation_elapsed_ms": observation.observation_elapsed_ms,
        "submit_confirmed": observation.submit_confirmed,
        "submit_confirmation_basis": observation.submit_confirmation_basis,
        "submit_confirmation_window_ms": observation.submit_confirmation_window_ms,
        "submit_confirmation_elapsed_ms": observation.submit_confirmation_elapsed_ms,
        "evidence_scope": INJECTION_EVIDENCE_SCOPE,
    })
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

    let target_agent_id = payload.target_agent_id.clone();
    let injection_target_agent_id = target_agent_id.clone();
    let manager = Arc::clone(&state.injection_manager);
    let injection_session_id = id.clone();
    let injection_result = tokio::task::spawn_blocking(move || {
        manager.read().operator_inject(
            &injection_session_id,
            &injection_target_agent_id,
            &payload.message,
            payload.submit,
        )
    })
    .await
    .map_err(|error| ApiError::internal(format!("Injection task failed: {error}")))?;
    let receipt = injection_result.map_err(|error| ApiError::internal(error.to_string()))?;
    let observation = observe_injection(&state, &target_agent_id, &receipt).await;
    record_injection_submission(&state, &id, &target_agent_id, &receipt);

    Ok(Json(measured_response(
        format!("Operator injection sent to session {}", id),
        &receipt,
        &observation,
    )))
}

pub async fn queen_inject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<QueenInjectRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&id)?;
    validate_agent_id(&payload.queen_id)?;
    validate_agent_id(&payload.target_worker_id)?;

    let target_worker_id = payload.target_worker_id.clone();
    let injection_target_worker_id = target_worker_id.clone();
    let manager = Arc::clone(&state.injection_manager);
    let injection_session_id = id.clone();
    let injection_result = tokio::task::spawn_blocking(move || {
        manager.read().queen_inject(
            &injection_session_id,
            &payload.queen_id,
            &injection_target_worker_id,
            &payload.message,
            payload.submit,
        )
    })
    .await
    .map_err(|error| ApiError::internal(format!("Injection task failed: {error}")))?;
    let receipt = injection_result.map_err(map_injection_error)?;
    let observation = observe_injection(&state, &target_worker_id, &receipt).await;
    record_injection_submission(&state, &id, &target_worker_id, &receipt);

    Ok(Json(measured_response(
        format!("Queen injection sent to session {}", id),
        &receipt,
        &observation,
    )))
}

pub async fn evaluator_inject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<EvaluatorInjectRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&id)?;
    validate_agent_id(&payload.evaluator_id)?;
    validate_agent_id(&payload.target_agent_id)?;

    let target_agent_id = payload.target_agent_id.clone();
    let injection_target_agent_id = target_agent_id.clone();
    let manager = Arc::clone(&state.injection_manager);
    let injection_session_id = id.clone();
    let injection_result = tokio::task::spawn_blocking(move || {
        manager.read().evaluator_inject(
            &injection_session_id,
            &payload.evaluator_id,
            &injection_target_agent_id,
            &payload.message,
            payload.submit,
        )
    })
    .await
    .map_err(|error| ApiError::internal(format!("Injection task failed: {error}")))?;
    let receipt = injection_result.map_err(map_injection_error)?;
    let observation = observe_injection(&state, &target_agent_id, &receipt).await;
    record_injection_submission(&state, &id, &target_agent_id, &receipt);

    Ok(Json(measured_response(
        format!("Evaluator injection sent to session {}", id),
        &receipt,
        &observation,
    )))
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
    use crate::domain::HiveExecutionPolicy;
    use crate::events::EventBus;
    use crate::pty::{AgentConfig, AgentRole, AgentStatus, PtyManager};
    use crate::session::{
        AgentInfo, AuthStrategy, Session, SessionController, SessionState, SessionType,
    };
    use crate::storage::{ApplicationStateDb, QueueRepo, SessionStorage};
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        routing::post,
        Router,
    };
    use chrono::{Duration as ChronoDuration, Utc};
    use parking_lot::RwLock;
    use serde_json::{json, Value};
    use tempfile::TempDir;
    use tower::ServiceExt;

    const SESSION_ID: &str = "inject-test";
    const AGENT_ID: &str = "inject-test-worker-1";
    const QUEEN_ID: &str = "inject-test-queen";
    const QA_WORKER_ID: &str = "inject-test-qa-worker-1";

    fn agent(id: &str, role: AgentRole) -> AgentInfo {
        AgentInfo {
            id: id.to_string(),
            role,
            status: AgentStatus::Running,
            config: AgentConfig::default(),
            parent_id: None,
            role_definition_id: None,
            role_definition_version: None,
            commit_sha: None,
            base_commit_sha: None,
        }
    }

    fn running_test_session(project_path: std::path::PathBuf) -> Session {
        let now = Utc::now();
        Session {
            id: SESSION_ID.to_string(),
            name: None,
            color: None,
            session_type: SessionType::Hive { worker_count: 1 },
            project_path,
            state: SessionState::Running,
            created_at: now,
            last_activity_at: now,
            agents: vec![
                agent(
                    AGENT_ID,
                    AgentRole::Worker {
                        index: 1,
                        parent: None,
                    },
                ),
                agent(QUEEN_ID, AgentRole::Queen),
                agent(
                    QA_WORKER_ID,
                    AgentRole::QaWorker {
                        index: 1,
                        parent: Some("inject-test-evaluator".to_string()),
                    },
                ),
            ],
            default_cli: "claude".to_string(),
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

    fn setup_test_app() -> (TempDir, Router, Arc<AppState>) {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(
            SessionStorage::new_with_base(temp_dir.path().to_path_buf()).unwrap(),
        );
        storage.create_session_dir(SESSION_ID).unwrap();
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
        pty_manager
            .write()
            .create_session(
                QUEEN_ID.to_string(),
                AgentRole::Queen,
                "claude",
                &[],
                None,
                80,
                24,
            )
            .unwrap();
        pty_manager
            .write()
            .create_session(
                QA_WORKER_ID.to_string(),
                AgentRole::QaWorker {
                    index: 1,
                    parent: Some("inject-test-evaluator".to_string()),
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
        session_controller
            .read()
            .insert_test_session(running_test_session(temp_dir.path().to_path_buf()));
        session_controller
            .read()
            .update_session_metadata(SESSION_ID, None, None)
            .unwrap();
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
            .route("/api/sessions/{id}/inject/queen", post(queen_inject))
            .route(
                "/api/sessions/{id}/inject/evaluator",
                post(evaluator_inject),
            )
            .with_state(state.clone());

        (temp_dir, app, state)
    }

    async fn post_inject(app: Router, path: &str, body: Value) -> (StatusCode, Value) {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn operator_inject_omitted_submit_defaults_to_true() {
        let (_temp_dir, app, state) = setup_test_app();

        let (status, response) = post_inject(
            app,
            "/api/sessions/inject-test/inject",
            json!({ "target_agent_id": AGENT_ID, "message": "hello\r\n" }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["payload_bytes_written"], 5);
        assert_eq!(response["submit_attempted"], true);
        assert_eq!(response["submit_keystroke_issued"], true);
        assert_eq!(response["submit_bytes_written"], 1);
        assert_eq!(response["pty_observation_available"], true);
        assert_eq!(response["pty_output_bytes_before"], 0);
        // The stub mirrors stdin into the ring: 6-byte start marker + 5-byte payload
        // + 6-byte end marker + 1-byte Enter.
        assert_eq!(response["pty_output_bytes_after_write"], 18);
        assert_eq!(response["pty_output_bytes_after"], 18);
        assert_eq!(response["pty_activity_observed"], false);
        assert_eq!(response["observation_window_ms"], 250);
        assert_eq!(response["submit_confirmed"], false);
        assert_eq!(
            response["submit_confirmation_basis"],
            "no-post-submit-activity"
        );
        assert_eq!(response["submit_confirmation_window_ms"], 1500);
        assert!(response["write_started_at"].is_string());
        assert!(response["submitted_at"].is_string());
        assert!(response["evidence_scope"]
            .as_str()
            .unwrap()
            .contains("do not prove that the agent took a turn"));
        assert_eq!(
            state
                .pty_manager
                .read()
                .write_records_for_test(AGENT_ID)
                .unwrap(),
            vec![
                b"\x1b[200~".to_vec(),
                b"hello".to_vec(),
                b"\x1b[201~".to_vec(),
                b"\r".to_vec()
            ]
        );
    }

    #[tokio::test]
    async fn operator_inject_submit_false_writes_payload_without_enter() {
        let (_temp_dir, app, state) = setup_test_app();

        let (status, response) = post_inject(
            app,
            "/api/sessions/inject-test/inject",
            json!({
                "target_agent_id": AGENT_ID,
                "message": "draft\r\n",
                "submit": false
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["payload_bytes_written"], 5);
        assert_eq!(response["submit_attempted"], false);
        assert_eq!(response["submit_keystroke_issued"], false);
        assert_eq!(response["submit_bytes_written"], 0);
        assert_eq!(response["observation_window_ms"], 0);
        assert_eq!(response["observation_elapsed_ms"], 0);
        assert_eq!(response["pty_activity_observed"], Value::Null);
        assert_eq!(response["submit_confirmed"], Value::Null);
        assert_eq!(response["submit_confirmation_basis"], "not-submitted");
        assert_eq!(response["submit_confirmation_window_ms"], 0);
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

        let (status, _response) = post_inject(
            app,
            "/api/sessions/inject-test/inject",
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
        assert_eq!(
            writes,
            vec![
                b"\x1b[200~".to_vec(),
                b"line one\nline two".to_vec(),
                b"\x1b[201~".to_vec(),
                b"\r".to_vec()
            ]
        );
        assert_eq!(writes.iter().filter(|write| write.as_slice() == b"\r").count(), 1);
        assert!(!writes.last().unwrap().contains(&b'\n'));
    }

    #[tokio::test]
    async fn operator_inject_blocking_path_preserves_pty_error_status() {
        let (_temp_dir, app, _state) = setup_test_app();

        let (status, _response) = post_inject(
            app,
            "/api/sessions/inject-test/inject",
            json!({ "target_agent_id": "missing-agent", "message": "hello" }),
        )
        .await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn all_inject_routes_return_the_measured_sender_contract() {
        let (_temp_dir, app, _state) = setup_test_app();
        let cases = [
            (
                "/api/sessions/inject-test/inject",
                json!({ "target_agent_id": AGENT_ID, "message": "operator" }),
            ),
            (
                "/api/sessions/inject-test/inject/queen",
                json!({
                    "queen_id": QUEEN_ID,
                    "target_worker_id": AGENT_ID,
                    "message": "queen"
                }),
            ),
            (
                "/api/sessions/inject-test/inject/evaluator",
                json!({
                    "evaluator_id": "inject-test-evaluator",
                    "target_agent_id": QUEEN_ID,
                    "message": "evaluator"
                }),
            ),
        ];

        for (path, body) in cases {
            let (status, response) = post_inject(app.clone(), path, body).await;
            assert_eq!(status, StatusCode::OK, "{path}: {response}");
            assert_eq!(response["submit_keystroke_issued"], true, "{path}");
            assert_eq!(response["submit_bytes_written"], 1, "{path}");
            assert_eq!(response["pty_observation_available"], true, "{path}");
            assert!(response["pty_output_bytes_before"].is_number(), "{path}");
            assert!(response["pty_output_bytes_after_write"].is_number(), "{path}");
            assert_eq!(response["observation_window_ms"], 250, "{path}");
            assert_eq!(response["submit_confirmed"], false, "{path}");
            assert_eq!(response["submit_confirmation_window_ms"], 1500, "{path}");
            assert!(
                response["submit_confirmation_basis"].is_string(),
                "{path}"
            );
            assert!(response["evidence_scope"]
                .as_str()
                .unwrap()
                .contains("do not prove that the agent took a turn"));
        }
    }

    /// #256 acceptance: a payload well over 1 KB travels through the full HTTP stack in
    /// one bracketed envelope with exactly one Enter, and the receipt reports the real
    /// payload byte count.
    #[tokio::test]
    async fn operator_inject_large_payload_brackets_once_and_submits_once() {
        let (_temp_dir, app, state) = setup_test_app();
        let payload = "p".repeat(1287);

        let (status, response) = post_inject(
            app,
            "/api/sessions/inject-test/inject",
            json!({ "target_agent_id": AGENT_ID, "message": payload }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["payload_bytes_written"], 1287);
        assert_eq!(response["submit_bytes_written"], 1);
        let writes = state
            .pty_manager
            .read()
            .write_records_for_test(AGENT_ID)
            .unwrap();
        assert_eq!(
            writes,
            vec![
                b"\x1b[200~".to_vec(),
                payload.as_bytes().to_vec(),
                b"\x1b[201~".to_vec(),
                b"\r".to_vec()
            ]
        );
    }

    /// #256: payload_bytes_written reports the sanitized bytes actually written, not the
    /// request length — an embedded end marker is stripped before framing.
    #[tokio::test]
    async fn operator_inject_reports_sanitized_payload_bytes() {
        let (_temp_dir, app, state) = setup_test_app();

        let (status, response) = post_inject(
            app,
            "/api/sessions/inject-test/inject",
            json!({ "target_agent_id": AGENT_ID, "message": "safe\u{1b}[201~payload" }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["payload_bytes_written"], "safepayload".len());
        let writes = state
            .pty_manager
            .read()
            .write_records_for_test(AGENT_ID)
            .unwrap();
        assert_eq!(writes[1], b"safepayload".to_vec());
    }

    /// #256: the documented two-call workaround — an empty message with submit:true —
    /// stays a bare-Enter flush with no paste envelope.
    #[tokio::test]
    async fn operator_inject_empty_message_flushes_with_a_bare_enter() {
        let (_temp_dir, app, state) = setup_test_app();

        let (status, response) = post_inject(
            app,
            "/api/sessions/inject-test/inject",
            json!({ "target_agent_id": AGENT_ID, "message": "" }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["payload_bytes_written"], 0);
        assert_eq!(response["submit_keystroke_issued"], true);
        assert_eq!(response["submit_bytes_written"], 1);
        assert_eq!(
            state
                .pty_manager
                .read()
                .write_records_for_test(AGENT_ID)
                .unwrap(),
            vec![b"\r".to_vec()]
        );
    }

    /// #256: sustained post-submit ring activity flips submit_confirmed to true. A
    /// background task plays the role of the child TUI repainting after it accepted
    /// the Enter.
    #[tokio::test]
    async fn operator_inject_confirms_submit_on_sustained_post_submit_activity() {
        let (_temp_dir, app, state) = setup_test_app();

        let pty_manager = Arc::clone(&state.pty_manager);
        let repainter = tokio::spawn(async move {
            for tick in 0..12u32 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                pty_manager
                    .read()
                    .record_output_for_test(AGENT_ID, format!("frame-{tick};").as_bytes());
            }
        });

        let (status, response) = post_inject(
            app,
            "/api/sessions/inject-test/inject",
            json!({ "target_agent_id": AGENT_ID, "message": "go" }),
        )
        .await;
        repainter.abort();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_confirmed"], true);
        assert_eq!(
            response["submit_confirmation_basis"],
            "sustained-post-submit-activity"
        );
        assert!(response["submit_confirmation_elapsed_ms"].as_u64().unwrap() <= 1500);
    }

    #[test]
    fn submit_confirmation_classifier_distinguishes_all_three_verdicts() {
        use std::time::Duration;

        assert_eq!(
            classify_submit_confirmation(&[]),
            (Some(false), "no-post-submit-activity")
        );
        assert_eq!(
            classify_submit_confirmation(&[Duration::from_millis(40)]),
            (None, "ambiguous-post-submit-activity"),
            "a single repaint also matches a swallowed Enter"
        );
        assert_eq!(
            classify_submit_confirmation(&[
                Duration::from_millis(40),
                Duration::from_millis(120)
            ]),
            (None, "ambiguous-post-submit-activity"),
            "a short burst is not sustained activity"
        );
        assert_eq!(
            classify_submit_confirmation(&[
                Duration::from_millis(40),
                Duration::from_millis(120),
                Duration::from_millis(290)
            ]),
            (Some(true), "sustained-post-submit-activity")
        );
    }

    #[test]
    fn stall_report_distinguishes_awaiting_inject_from_crashed_and_idle() {
        let (_temp_dir, _app, state) = setup_test_app();
        let controller = state.session_controller.read();
        controller
            .update_heartbeat(SESSION_ID, AGENT_ID, "working", None)
            .unwrap();
        controller
            .update_heartbeat(SESSION_ID, QUEEN_ID, "working", None)
            .unwrap();
        controller
            .update_heartbeat(SESSION_ID, QA_WORKER_ID, "idle", None)
            .unwrap();

        let stale_at = Utc::now() - ChronoDuration::minutes(5);
        for agent_id in [AGENT_ID, QUEEN_ID, QA_WORKER_ID] {
            controller.set_heartbeat_last_activity_for_test(SESSION_ID, agent_id, stale_at);
        }
        controller
            .record_injection_submission(
                SESSION_ID,
                AGENT_ID,
                stale_at + ChronoDuration::seconds(1),
                true,
            )
            .unwrap();
        state.pty_manager.read().kill(QUEEN_ID).unwrap();

        let reports = controller.get_stalled_agent_reports(
            SESSION_ID,
            std::time::Duration::from_secs(30),
        );
        let status_for = |agent_id: &str| {
            reports
                .iter()
                .find(|(id, _, _)| id == agent_id)
                .map(|(_, _, status)| status)
                .unwrap()
        };

        assert_eq!(
            status_for(AGENT_ID),
            &AgentStatus::WaitingForInput("awaiting_injected_turn".to_string())
        );
        assert_eq!(
            status_for(QUEEN_ID),
            &AgentStatus::Error("process_not_alive".to_string())
        );
        assert_eq!(status_for(QA_WORKER_ID), &AgentStatus::Idle);
    }
}
