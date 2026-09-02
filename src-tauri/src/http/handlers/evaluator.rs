use axum::{
    extract::{rejection::JsonRejection, Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fmt, sync::Arc};

use crate::coordination::{CoordinationMessage, StateManager};
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::orchestrator::work_graph::completion_ledger::{
    NodeCompletionFact, NodeCompletionProvenance,
};
use crate::orchestrator::work_graph::review::ReviewExpansionSidecar;
use crate::orchestrator::work_graph::runtime::{
    record_review_verdict_and_record, route_failed_verdict_and_record, GraphCompositionState,
    ReviewVerdict,
};
use crate::orchestrator::work_graph::{WorkGraph, WorkGraphOmission, WorkGraphOmissionReason};
use crate::pty::{AgentConfig, AgentRole};
use crate::session::{AuthStrategy, SessionController, SessionState};

use super::validate_session_id;
// validate_cli used by add_evaluator
use super::validate_cli;

#[derive(Debug, Clone, Deserialize)]
pub struct AddEvaluatorRequest {
    pub label: Option<String>,
    pub cli: Option<String>,
    pub model: Option<String>,
    pub initial_task: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddEvaluatorResponse {
    pub evaluator_id: String,
    pub cli: String,
    pub status: String,
    pub prompt_file: String,
}

/// Body for the operator QA overrides (#176).
///
/// `force-pass` / `force-fail` are destructive session-level overrides that
/// bypass the Evaluator entirely, so they must not be reachable by a bodyless
/// POST -- which is exactly how one was tripped while an agent was enumerating
/// the API surface.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ForceVerdictRequest {
    /// Must be `true`. `#[serde(default)]` is load-bearing: it moves the
    /// missing-field case out of serde's 422 and into our own 400 below, so
    /// every rejection carries the same explanatory message.
    #[serde(default)]
    pub confirm: bool,
    /// Optional operator note, recorded in the coordination audit log.
    #[serde(default)]
    pub rationale: Option<String>,
}

/// Funnel every malformed-body case -- absent `Content-Type`, `{}`,
/// `{"confirm": false}`, malformed JSON -- into one repo-shaped 400.
///
/// Taking `Result<Json<T>, JsonRejection>` rather than a bare `Json<T>` is
/// deliberate: a bare extractor answers the bodyless POST that today's UI sends
/// with axum's plain-text 415, which is neither actionable nor in our error
/// envelope.
fn require_confirmation(
    body: Result<Json<ForceVerdictRequest>, JsonRejection>,
    action: &str,
) -> Result<ForceVerdictRequest, ApiError> {
    let req = match body {
        Ok(Json(req)) => req,
        Err(_) => ForceVerdictRequest::default(),
    };

    if !req.confirm {
        return Err(ApiError::bad_request(format!(
            "Refusing to {action}: this is a destructive operator override. \
             Re-send with a JSON body {{\"confirm\": true}} (optionally with \
             \"rationale\": \"<why>\") and Content-Type: application/json."
        )));
    }

    Ok(req)
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddQaWorkerRequest {
    pub specialization: String,
    pub label: Option<String>,
    pub cli: Option<String>,
    pub model: Option<String>,
    pub initial_task: Option<String>,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddQaWorkerResponse {
    pub worker_id: String,
    pub role: String,
    pub cli: String,
    pub status: String,
    pub task_file: String,
}

fn validate_qa_specialization(specialization: &str) -> Result<(), ApiError> {
    if matches!(specialization, "ui" | "api" | "a11y" | "adversarial") {
        return Ok(());
    }

    Err(ApiError::bad_request(format!(
        "Invalid QA specialization '{}'. Valid options: ui, api, a11y, adversarial",
        specialization
    )))
}

fn qa_specialization_label(specialization: &str) -> &'static str {
    match specialization {
        "ui" => "UI QA",
        "api" => "API QA",
        "a11y" => "A11Y QA",
        "adversarial" => "Adversarial QA",
        _ => "QA Worker",
    }
}

fn map_add_qa_worker_error(error: String) -> ApiError {
    if error.contains("Session not found") {
        return ApiError::not_found(error);
    }

    if error.contains("Evaluator") && error.contains("not found for session") {
        return ApiError::bad_request(error);
    }

    if error.contains("is not an Evaluator") {
        return ApiError::bad_request(error);
    }

    if error.starts_with("Cannot add")
        || error.starts_with("Invalid")
        || error.contains("expected")
        || error.contains("precondition")
    {
        return ApiError::bad_request(error);
    }

    ApiError::internal(error)
}

pub async fn add_evaluator(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<AddEvaluatorRequest>,
) -> Result<(StatusCode, Json<AddEvaluatorResponse>), ApiError> {
    validate_session_id(&session_id)?;

    let session_default_cli = {
        let controller = state.session_controller.read();
        controller
            .get_session_default_cli(&session_id)
            .unwrap_or_else(|| "claude".to_string())
    };
    let cli = req.cli.unwrap_or(session_default_cli);
    validate_cli(&cli)?;

    let config = AgentConfig {
        cli: cli.clone(),
        model: req.model,
        flags: vec![],
        label: req.label.clone().or_else(|| Some("Evaluator".to_string())),
        name: None,
        description: None,
        role: None,
        initial_prompt: req.initial_task,
    };

    let evaluator_id = {
        let controller = state.session_controller.write();
        controller
            .launch_evaluator(&session_id, config, false)
            .map_err(ApiError::internal)?
            .id
    };

    Ok((
        StatusCode::CREATED,
        Json(AddEvaluatorResponse {
            evaluator_id,
            cli,
            status: "Running".to_string(),
            prompt_file: format!(".hive-manager/{}/prompts/evaluator-prompt.md", session_id),
        }),
    ))
}

pub async fn list_evaluators(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let controller = state.session_controller.read();
    let session = controller
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    let evaluators: Vec<Value> = session
        .agents
        .iter()
        .filter(|agent| matches!(agent.role, AgentRole::Evaluator))
        .map(|agent| {
            json!({
                "id": agent.id,
                "cli": agent.config.cli,
                "label": agent.config.label,
                "status": format!("{:?}", agent.status),
                "prompt_file": format!(".hive-manager/{}/prompts/evaluator-prompt.md", session_id),
            })
        })
        .collect();

    Ok(Json(json!({
        "session_id": session_id,
        "evaluators": evaluators,
        "count": evaluators.len()
    })))
}

pub async fn add_qa_worker(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<AddQaWorkerRequest>,
) -> Result<(StatusCode, Json<AddQaWorkerResponse>), ApiError> {
    validate_session_id(&session_id)?;
    validate_qa_specialization(&req.specialization)?;

    let session_default_cli = {
        let controller = state.session_controller.read();
        controller
            .get_session_default_cli(&session_id)
            .unwrap_or_else(|| "claude".to_string())
    };
    let cli = req.cli.unwrap_or(session_default_cli);
    validate_cli(&cli)?;

    let mut flags = Vec::new();
    // Auto-inject --chrome for UI QA workers using claude CLI
    if req.specialization == "ui" && cli == "claude" {
        flags.push("--chrome".to_string());
    }

    let config = AgentConfig {
        cli: cli.clone(),
        model: req.model,
        flags,
        label: req
            .label
            .clone()
            .or_else(|| Some(qa_specialization_label(&req.specialization).to_string())),
        name: None,
        description: None,
        role: None,
        initial_prompt: req.initial_task,
    };

    let agent_info = {
        let controller = state.session_controller.write();
        controller
            .add_qa_worker(
                &session_id,
                config,
                req.specialization.clone(),
                req.parent_id,
            )
            .map_err(map_add_qa_worker_error)?
    };

    let index = match &agent_info.role {
        AgentRole::QaWorker { index, .. } => *index,
        _ => {
            return Err(ApiError::internal(
                "add_qa_worker returned a non-QaWorker role".to_string(),
            ));
        }
    };

    Ok((
        StatusCode::CREATED,
        Json(AddQaWorkerResponse {
            worker_id: agent_info.id,
            role: qa_specialization_label(&req.specialization).to_string(),
            cli,
            status: "Running".to_string(),
            task_file: {
                let controller = state.session_controller.read();
                let session = controller.get_session(&session_id).ok_or_else(|| {
                    ApiError::not_found(format!("Session {} not found", session_id))
                })?;
                SessionController::absolute_task_file_path_for_qa_worker(
                    &session.project_path,
                    &session_id,
                    index as usize,
                )
                .to_string_lossy()
                .to_string()
            },
        }),
    ))
}

// --- Dev Login Endpoint ---

#[derive(Debug, Deserialize)]
pub struct DevLoginQuery {
    pub token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostVerdictRequest {
    pub verdict: String,
    /// Exact review-join node this session-wide QA verdict adjudicates. Absent
    /// remains backward compatible and must never be guessed from the graph.
    #[serde(default)]
    pub work_graph_verdict_id: Option<String>,
    #[serde(default)]
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub rationale: Option<String>,
    /// When the Evaluator cannot reach a PASS/FAIL it submits `verdict: "BLOCKED"`
    /// with a machine-readable category so the operator knows whether the blocker is
    /// an absent UI/host (criteria can't be exercised) or a transport failure
    /// (per-worker verdicts didn't arrive over HTTP) — the (a)-vs-(b) distinction.
    /// Examples: "ui-unavailable", "http-failure", "inconclusive".
    #[serde(default)]
    pub blocked_reason: Option<String>,
    /// Free-text detail accompanying a BLOCKED verdict (which criterion, which worker).
    #[serde(default)]
    pub blocked_detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostPrinceVerdictRequest {
    /// PASS/DONE/RESOLVED clear the gate; BLOCKED/FAIL/ESCALATE escalate to the
    /// operator when the Prince's team could not resolve the findings.
    pub verdict: String,
    #[serde(default)]
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub rationale: Option<String>,
}

pub async fn dev_login(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Query(query): Query<DevLoginQuery>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let controller = state.session_controller.read();
    let session = controller
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    match &session.auth_strategy {
        AuthStrategy::DevBypass { token } if *token == query.token => Ok(Json(json!({
            "session_id": session_id,
            "auth": "dev-bypass",
            "granted": true
        }))),
        AuthStrategy::DevBypass { .. } => Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Invalid dev-bypass token",
        )),
        AuthStrategy::None => Err(ApiError::not_found("Auth not configured for this session")),
    }
}

// --- Force Pass / Force Fail Endpoints ---

fn require_qa_in_progress(
    controller: &SessionController,
    session_id: &str,
    action: &str,
) -> Result<(), ApiError> {
    let session = controller
        .get_session(session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    if matches!(session.state, SessionState::QaInProgress { .. }) {
        return Ok(());
    }

    Err(ApiError::bad_request(format!(
        "Cannot {}: session is in {:?} state, expected QaInProgress",
        action, session.state
    )))
}

/// Operator overrides (force-pass / force-fail) are valid whenever QA is unresolved:
/// in progress, stalled as inconclusive, or mid-Prince-remediation. This lets the
/// operator unblock a session that QA left inconclusive or that the Prince couldn't
/// resolve.
fn require_qa_overridable(
    controller: &SessionController,
    session_id: &str,
    action: &str,
) -> Result<(), ApiError> {
    let session = controller
        .get_session(session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;

    if matches!(
        session.state,
        SessionState::QaInProgress { .. }
            | SessionState::QaInconclusive
            | SessionState::PrinceRemediation
    ) {
        return Ok(());
    }

    // #175(a): evaluator-backed sessions now sit in Running (or briefly
    // SpawningEvaluator) until the milestone handoff, but they still cannot
    // complete without reaching QaPassed. Without this the operator would have no
    // way to unblock a session before its first handoff.
    //
    // The widening is conditioned on the session actually having an Evaluator or
    // QA worker, which is exactly the set of sessions the timer change affects.
    // A non-evaluator session keeps the old gate -- otherwise a swarm could be
    // forced into QaPassed and permanently lose `add_planner`.
    if matches!(
        session.state,
        SessionState::Running | SessionState::SpawningEvaluator
    ) && SessionController::session_requires_internal_evaluator(&session)
    {
        return Ok(());
    }

    Err(ApiError::bad_request(format!(
        "Cannot {}: session is in {:?} state, expected QaInProgress, QaInconclusive, PrinceRemediation, or an evaluator-backed Running session",
        action, session.state
    )))
}

fn append_operator_log(
    state: &AppState,
    session_id: &str,
    action: &str,
    detail: &str,
    rationale: Option<&str>,
) {
    let mut body = format!(
        "[{}] Operator forced {} for session {}",
        action, detail, session_id
    );
    if let Some(rationale) = rationale.map(str::trim).filter(|r| !r.is_empty()) {
        body.push_str(&format!(" — rationale: {}", rationale));
    }
    let msg = crate::coordination::CoordinationMessage::system("OPERATOR", &body);
    let _ = state.storage.append_coordination_log(session_id, &msg);
}

fn override_log_details(verdict: &str) -> Result<(&'static str, &'static str), ApiError> {
    let normalized = verdict.trim().to_ascii_uppercase();
    match normalized.as_str() {
        "PASS" | "QA_VERDICT: PASS" => Ok(("FORCE-PASS", "QA pass")),
        "FAIL" | "QA_VERDICT: FAIL" => Ok(("FORCE-FAIL", "QA fail")),
        _ => Err(ApiError::bad_request(format!(
            "Unsupported QA verdict '{}'",
            verdict
        ))),
    }
}

fn normalize_post_verdict(verdict: &str) -> Result<&'static str, ApiError> {
    match verdict.trim().to_ascii_uppercase().as_str() {
        "PASS" => Ok("PASS"),
        "FAIL" => Ok("FAIL"),
        "BLOCKED" => Ok("BLOCKED"),
        other => Err(ApiError::bad_request(format!(
            "Unsupported QA verdict '{}'. Expected PASS, FAIL, or BLOCKED",
            other
        ))),
    }
}

/// Map controller state-guard errors to the right HTTP status: a state-precondition
/// miss is a 409 conflict (caller should re-read state), a missing session is 404,
/// everything else is a 500.
fn map_verdict_state_error(error: String) -> ApiError {
    if error.contains("Session not found") {
        ApiError::not_found(error)
    } else if error.contains("expected") || error.contains("Cannot ") {
        ApiError::new(StatusCode::CONFLICT, error)
    } else {
        ApiError::internal(error)
    }
}

fn build_verdict_content(
    verdict: &str,
    rationale: Option<&str>,
    commit_sha: Option<&str>,
) -> String {
    let mut content = serde_json::Map::new();
    content.insert("kind".to_string(), json!("qa-verdict"));
    content.insert("verdict".to_string(), json!(verdict));
    if let Some(rationale) = rationale {
        content.insert("rationale".to_string(), json!(rationale));
    }
    if let Some(commit_sha) = commit_sha {
        content.insert("commit_sha".to_string(), json!(commit_sha));
    }
    Value::Object(content).to_string()
}

const MISSING_WORK_GRAPH_VERDICT_ID: &str = "qa-verdict:missing-work-graph-verdict-id";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum WorkGraphVerdictRouting {
    OmittedMissingVerdictId {
        omission_persisted: bool,
    },
    Passed {
        verdict_id: String,
        delta_sequence: Option<u64>,
    },
    FailedRouted {
        verdict_id: String,
        next_verdict_id: String,
        remediation_id: String,
        delta_sequence: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkGraphVerdictError {
    State(String),
    MissingGraph,
    MissingSidecar,
    UnknownVerdict(String),
    StaleVerdict { requested: String, current: String },
    Mutation(String),
}

impl fmt::Display for WorkGraphVerdictError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(message) => formatter.write_str(message),
            Self::MissingGraph => formatter.write_str(
                "work_graph_verdict_id was provided, but no persisted work graph exists",
            ),
            Self::MissingSidecar => formatter.write_str(
                "work_graph_verdict_id was provided, but no persisted review expansion sidecar exists",
            ),
            Self::UnknownVerdict(verdict_id) => write!(
                formatter,
                "work_graph_verdict_id '{verdict_id}' is not present in the review expansion sidecar"
            ),
            Self::StaleVerdict { requested, current } => write!(
                formatter,
                "work_graph_verdict_id '{requested}' is not the current review verdict '{current}'"
            ),
            Self::Mutation(message) => formatter.write_str(message),
        }
    }
}

fn map_work_graph_verdict_error(error: WorkGraphVerdictError) -> ApiError {
    match error {
        WorkGraphVerdictError::UnknownVerdict(_) | WorkGraphVerdictError::StaleVerdict { .. } => {
            ApiError::bad_request(error.to_string())
        }
        WorkGraphVerdictError::MissingGraph | WorkGraphVerdictError::MissingSidecar => {
            ApiError::new(StatusCode::CONFLICT, error.to_string())
        }
        WorkGraphVerdictError::State(_) | WorkGraphVerdictError::Mutation(_) => {
            ApiError::internal(error.to_string())
        }
    }
}

fn load_authoritative_work_graph(
    state_manager: &StateManager,
) -> Result<(WorkGraph, Option<GraphCompositionState>), WorkGraphVerdictError> {
    let composition = state_manager
        .read_graph_composition_state()
        .map_err(|error| {
            WorkGraphVerdictError::State(format!(
                "Failed to read graph composition for QA verdict: {error}"
            ))
        })?;
    if let Some(composition) = composition {
        return Ok((composition.graph.clone(), Some(composition)));
    }
    let graph = state_manager
        .read_work_graph()
        .map_err(|error| {
            WorkGraphVerdictError::State(format!(
                "Failed to read work graph for QA verdict: {error}"
            ))
        })?
        .ok_or(WorkGraphVerdictError::MissingGraph)?;
    Ok((graph, None))
}

fn persist_work_graph_verdict(
    state_manager: &StateManager,
    graph: &WorkGraph,
    sidecar: &ReviewExpansionSidecar,
    mut composition: Option<GraphCompositionState>,
    completion_facts: &[NodeCompletionFact],
) -> Result<(), WorkGraphVerdictError> {
    state_manager.write_work_graph(graph).map_err(|error| {
        WorkGraphVerdictError::State(format!(
            "Failed to persist work graph after QA verdict: {error}"
        ))
    })?;
    state_manager
        .write_review_expansion_sidecar(sidecar)
        .map_err(|error| {
            WorkGraphVerdictError::State(format!(
                "Failed to persist review expansion sidecar after QA verdict: {error}"
            ))
        })?;
    if let Some(composition) = composition.as_mut() {
        composition.graph = graph.clone();
        composition.reviews = sidecar.clone();
        state_manager
            .write_graph_composition_state(composition)
            .map_err(|error| {
                WorkGraphVerdictError::State(format!(
                    "Failed to persist graph composition after QA verdict: {error}"
                ))
            })?;
    }
    state_manager
        .append_node_completion_facts(completion_facts)
        .map_err(|error| {
            WorkGraphVerdictError::State(format!(
                "Failed to persist declared completion after QA verdict: {error}"
            ))
        })?;
    Ok(())
}

fn persist_missing_verdict_id_omission(
    state_manager: &StateManager,
    graph: &WorkGraph,
    mut composition: Option<GraphCompositionState>,
) -> Result<(), WorkGraphVerdictError> {
    state_manager.write_work_graph(graph).map_err(|error| {
        WorkGraphVerdictError::State(format!(
            "Failed to persist missing graph-verdict-id omission: {error}"
        ))
    })?;
    if let Some(composition) = composition.as_mut() {
        composition.graph = graph.clone();
        state_manager
            .write_graph_composition_state(composition)
            .map_err(|error| {
                WorkGraphVerdictError::State(format!(
                    "Failed to persist graph composition omission: {error}"
                ))
            })?;
    }
    Ok(())
}

/// Production graph-verdict boundary called by `post_verdict`. It deliberately
/// accepts only an explicit verdict id; session-wide QA must never select a
/// review join by position, kind, or title.
pub(crate) fn apply_work_graph_verdict(
    state_manager: &StateManager,
    session_id: &str,
    work_graph_verdict_id: Option<&str>,
    verdict: &str,
) -> Result<WorkGraphVerdictRouting, WorkGraphVerdictError> {
    apply_work_graph_verdict_for_agent(
        state_manager,
        session_id,
        work_graph_verdict_id,
        verdict,
        None,
    )
    .map(|(routing, _)| routing)
}

fn apply_work_graph_verdict_for_agent(
    state_manager: &StateManager,
    session_id: &str,
    work_graph_verdict_id: Option<&str>,
    verdict: &str,
    evaluator_agent_id: Option<&str>,
) -> Result<(WorkGraphVerdictRouting, Vec<NodeCompletionFact>), WorkGraphVerdictError> {
    let verdict_id = work_graph_verdict_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if verdict_id.is_none() {
        let loaded = load_authoritative_work_graph(state_manager);
        let Ok((mut graph, composition)) = loaded else {
            tracing::warn!(
                session_id,
                omission = MISSING_WORK_GRAPH_VERDICT_ID,
                "QA verdict has no explicit work-graph verdict id; no graph node was guessed"
            );
            return Ok((
                WorkGraphVerdictRouting::OmittedMissingVerdictId {
                    omission_persisted: false,
                },
                Vec::new(),
            ));
        };
        let omission = WorkGraphOmission::new(
            WorkGraphOmissionReason::ResolutionIncomplete,
            1,
            vec![MISSING_WORK_GRAPH_VERDICT_ID.to_string()],
        );
        if !graph.omissions.contains(&omission) {
            graph.omissions.push(omission);
        }
        let persisted =
            persist_missing_verdict_id_omission(state_manager, &graph, composition).is_ok();
        if !persisted {
            tracing::warn!(
                session_id,
                omission = MISSING_WORK_GRAPH_VERDICT_ID,
                "Could not persist missing work-graph verdict-id omission"
            );
        }
        return Ok((
            WorkGraphVerdictRouting::OmittedMissingVerdictId {
                omission_persisted: persisted,
            },
            Vec::new(),
        ));
    }
    let verdict_id = verdict_id.expect("checked above").to_string();
    let (mut graph, composition) = load_authoritative_work_graph(state_manager)?;
    let mut sidecar = state_manager
        .read_review_expansion_sidecar()
        .map_err(|error| {
            WorkGraphVerdictError::State(format!(
                "Failed to read review expansion sidecar for QA verdict: {error}"
            ))
        })?
        .or_else(|| composition.as_ref().map(|state| state.reviews.clone()))
        .ok_or(WorkGraphVerdictError::MissingSidecar)?;

    if sidecar.record_for_verdict(&verdict_id).is_none() {
        return Err(WorkGraphVerdictError::UnknownVerdict(verdict_id));
    }

    let routing = match verdict {
        "PASS" => {
            let delta = record_review_verdict_and_record(
                session_id,
                &mut graph,
                &verdict_id,
                ReviewVerdict::Passed,
            )
            .map_err(|error| WorkGraphVerdictError::Mutation(error.to_string()))?;
            WorkGraphVerdictRouting::Passed {
                verdict_id: verdict_id.clone(),
                delta_sequence: delta.map(|delta| delta.sequence),
            }
        }
        "FAIL" => {
            let record = sidecar
                .record_for_verdict_mut(&verdict_id)
                .ok_or_else(|| WorkGraphVerdictError::UnknownVerdict(verdict_id.clone()))?;
            let current = record
                .expansion
                .rounds
                .last()
                .map(|round| round.verdict_id.clone())
                .ok_or_else(|| WorkGraphVerdictError::UnknownVerdict(verdict_id.clone()))?;
            if current != verdict_id {
                return Err(WorkGraphVerdictError::StaleVerdict {
                    requested: verdict_id,
                    current,
                });
            }
            let (round, delta) = route_failed_verdict_and_record(
                session_id,
                &mut graph,
                &record.template,
                &mut record.expansion,
            )
            .map_err(|error| WorkGraphVerdictError::Mutation(error.to_string()))?;
            let remediation_id = record
                .expansion
                .remediation_ids
                .last()
                .cloned()
                .ok_or_else(|| {
                    WorkGraphVerdictError::Mutation(
                        "failed review route did not produce a remediation node".to_string(),
                    )
                })?;
            WorkGraphVerdictRouting::FailedRouted {
                verdict_id: verdict_id.clone(),
                next_verdict_id: round.verdict_id,
                remediation_id,
                delta_sequence: delta.map(|delta| delta.sequence),
            }
        }
        other => {
            return Err(WorkGraphVerdictError::Mutation(format!(
                "Unsupported graph verdict '{other}'"
            )))
        }
    };
    let completion_facts = match (&routing, evaluator_agent_id) {
        (WorkGraphVerdictRouting::Passed { verdict_id, .. }, Some(agent_id)) => {
            vec![NodeCompletionFact::new(
                verdict_id.clone(),
                agent_id.to_string(),
                NodeCompletionProvenance::EvaluatorVerdict,
            )]
        }
        _ => Vec::new(),
    };
    persist_work_graph_verdict(
        state_manager,
        &graph,
        &sidecar,
        composition,
        &completion_facts,
    )?;
    Ok((routing, completion_facts))
}

pub(crate) fn apply_verdict(
    state: &AppState,
    session_id: &str,
    verdict: &str,
    is_override: bool,
    rationale: Option<&str>,
) -> Result<SessionState, ApiError> {
    let action = if is_override {
        let (action, _) = override_log_details(verdict)?;
        match action {
            "FORCE-PASS" => "force-pass",
            "FORCE-FAIL" => "force-fail",
            _ => unreachable!("override verdicts are normalized before logging"),
        }
    } else {
        "qa-verdict"
    };

    let controller = state.session_controller.read();
    require_qa_overridable(&controller, session_id, action)?;
    let new_state = controller
        .on_qa_verdict(session_id, verdict)
        .map_err(ApiError::internal)?;
    drop(controller);

    if is_override {
        let (log_action, detail) = override_log_details(verdict)?;
        append_operator_log(state, session_id, log_action, detail, rationale);
    }

    Ok(new_state)
}

pub async fn post_verdict(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<PostVerdictRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let verdict = normalize_post_verdict(&req.verdict)?;
    let commit_sha = req
        .commit_sha
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let rationale = req
        .rationale
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    // #175(a) self-heal. The QA window normally opens when the peer watcher
    // observes milestone-ready, but that watcher does not exist when the session
    // was launched without an app handle. Without this, such a session
    // hard-deadlocks: the Evaluator's real verdict is rejected, and so is its
    // documented BLOCKED fallback (this guard runs before the BLOCKED fork), while
    // the Queen polls forever with no clock. This also rescues in-flight sessions
    // that were launched under the old prompt.
    {
        let controller = state.session_controller.read();
        let needs_window = controller.get_session(&session_id).is_some_and(|session| {
            matches!(
                session.state,
                SessionState::Running | SessionState::SpawningEvaluator
            ) && session
                .agents
                .iter()
                .any(|agent| matches!(agent.role, AgentRole::Evaluator))
        });
        if needs_window {
            tracing::info!(
                session_id = %session_id,
                "Opening a QA window from an incoming verdict: the milestone handoff was never observed"
            );
            controller
                .begin_qa_window(&session_id)
                .map_err(ApiError::internal)?;
        }
    }

    // Resolve peer identities + project path up front (needed for both BLOCKED and
    // PASS/FAIL paths). require_qa_in_progress rejects verdicts posted outside the
    // QaInProgress window so a stale POST can't jump the state machine.
    let (project_path, evaluator_id, queen_id) = {
        let controller = state.session_controller.read();
        require_qa_in_progress(&controller, &session_id, "qa-verdict")?;
        let session = controller
            .get_session(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
        let evaluator_id = session
            .agents
            .iter()
            .find(|agent| matches!(agent.role, AgentRole::Evaluator))
            .map(|agent| agent.id.clone())
            .unwrap_or_else(|| format!("{}-evaluator", session_id));
        (
            session.project_path.clone(),
            evaluator_id,
            format!("{}-queen", session_id),
        )
    };

    // BLOCKED: the Evaluator could not produce a usable PASS/FAIL. Mark the session
    // inconclusive (which writes the BLOCKED peer file + emits to the operator) and
    // return without shipping. This is root-cause (a)/(b)/(c): the operator can see
    // *why* it stalled instead of the Queen waiting forever.
    if verdict == "BLOCKED" {
        let reason = blocked_reason_message(
            req.blocked_reason.as_deref(),
            req.blocked_detail.as_deref(),
            rationale,
        );
        let new_state = {
            let controller = state.session_controller.read();
            controller
                .mark_qa_inconclusive(&session_id, &reason)
                .map_err(map_verdict_state_error)?
        };
        return Ok(Json(json!({
            "session_id": session_id,
            "action": "qa-verdict",
            "verdict": "BLOCKED",
            "blocked_reason": req.blocked_reason,
            "new_state": format!("{:?}", new_state),
            "persisted": true,
            "peer_file_written": true,
            "rationale": reason,
        })));
    }

    let verdict_content = build_verdict_content(verdict, rationale, commit_sha);

    // Persist the verdict peer file FIRST, while the session is still QaInProgress.
    // If this write fails we return 500 (the Evaluator's protocol retries) WITHOUT
    // having transitioned the state machine — so the retry lands cleanly instead of
    // hitting an "expected QaInProgress" rejection. This closes root-cause (b):
    // a failed peer-file write is no longer swallowed behind an HTTP 200.
    let state_manager = StateManager::new(project_path.join(".hive-manager").join(&session_id));
    state_manager
        .write_qa_verdict_async(&evaluator_id, &queen_id, &verdict_content, commit_sha)
        .await
        .map_err(|err| {
            ApiError::internal(format!(
                "Failed to persist QA verdict peer record (retry the POST): {}",
                err
            ))
        })?;

    let verdict_message =
        CoordinationMessage::qa_verdict(&evaluator_id, &queen_id, &verdict_content);
    if let Err(err) = state
        .storage
        .append_coordination_log(&session_id, &verdict_message)
    {
        tracing::warn!(
            session_id = %session_id,
            error = %err,
            "Failed to append QA verdict audit log after HTTP verdict"
        );
    }

    // The session verdict and the graph verdict are separate facts. Only an
    // explicitly supplied join id may mutate the graph; the legacy request
    // shape records a ResolutionIncomplete omission without selecting a node.
    let graph_state_manager = StateManager::new(state.storage.session_dir(&session_id));
    let (work_graph_routing, completion_facts) = apply_work_graph_verdict_for_agent(
        &graph_state_manager,
        &session_id,
        req.work_graph_verdict_id.as_deref(),
        verdict,
        Some(&evaluator_id),
    )
    .map_err(map_work_graph_verdict_error)?;
    for fact in completion_facts {
        if let Err(error) = state.event_bus.publish(fact.event(&session_id)).await {
            tracing::warn!(
                session_id = %session_id,
                task_id = %fact.task_id,
                "Failed to publish durable evaluator completion event: {error}"
            );
        }
    }

    let new_state = {
        let controller = state.session_controller.read();
        controller
            .record_http_qa_verdict(&session_id, &evaluator_id, verdict, commit_sha)
            .map_err(map_verdict_state_error)?
    };

    Ok(Json(json!({
        "session_id": session_id,
        "action": "qa-verdict",
        "verdict": verdict,
        "new_state": format!("{:?}", new_state),
        "commit_sha": commit_sha,
        "rationale": rationale,
        "persisted": true,
        "peer_file_written": true,
        "work_graph": work_graph_routing,
    })))
}

/// Build a human-readable reason for a BLOCKED verdict from the structured category
/// plus any free-text detail, so the operator banner explains the (a)-vs-(b) cause.
fn blocked_reason_message(
    blocked_reason: Option<&str>,
    blocked_detail: Option<&str>,
    rationale: Option<&str>,
) -> String {
    let category = match blocked_reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some("ui-unavailable") | Some("ui_unavailable") => {
            "A pass-criterion requires a UI/host that isn't running, so it can't be exercised."
        }
        Some("http-failure") | Some("http_failure") => {
            "One or more QA-worker verdicts could not be delivered over HTTP."
        }
        Some(other) => {
            return match blocked_detail.or(rationale) {
                Some(detail) if !detail.trim().is_empty() => {
                    format!("QA blocked ({}): {}", other, detail.trim())
                }
                _ => format!("QA blocked: {}", other),
            }
        }
        None => "QA could not reach a PASS/FAIL verdict.",
    };
    match blocked_detail.or(rationale) {
        Some(detail) if !detail.trim().is_empty() => format!("{} {}", category, detail.trim()),
        _ => category.to_string(),
    }
}

/// The Prince's remediation verdict. Posted after the Prince's fix team resolves the
/// QA findings: PASS/DONE clears the gate so the Queen may push; BLOCKED escalates to
/// the operator. Mirrors `post_verdict`: persist the peer file before transitioning,
/// and surface a persistence failure as a 500 so the Prince retries.
pub async fn post_prince_verdict(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<PostPrinceVerdictRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let normalized = req.verdict.trim().to_ascii_uppercase();
    if !matches!(
        normalized.as_str(),
        "PASS" | "DONE" | "RESOLVED" | "BLOCKED" | "FAIL" | "ESCALATE"
    ) {
        return Err(ApiError::bad_request(format!(
            "Unsupported Prince verdict '{}'. Expected PASS/DONE/RESOLVED (clear) or BLOCKED/FAIL/ESCALATE (escalate)",
            req.verdict
        )));
    }
    let commit_sha = req
        .commit_sha
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let rationale = req
        .rationale
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let (project_path, prince_id, queen_id) = {
        let controller = state.session_controller.read();
        let session = controller
            .get_session(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
        if !matches!(session.state, SessionState::PrinceRemediation) {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                format!(
                    "Cannot record Prince verdict: session is in {:?} state, expected PrinceRemediation",
                    session.state
                ),
            ));
        }
        let prince_id = session
            .agents
            .iter()
            .find(|agent| matches!(agent.role, AgentRole::Prince))
            .map(|agent| agent.id.clone())
            .unwrap_or_else(|| format!("{}-prince", session_id));
        (
            session.project_path.clone(),
            prince_id,
            format!("{}-queen", session_id),
        )
    };

    let mut content = serde_json::Map::new();
    content.insert("kind".to_string(), json!("prince-verdict"));
    content.insert("verdict".to_string(), json!(normalized));
    if let Some(rationale) = rationale {
        content.insert("rationale".to_string(), json!(rationale));
    }
    if let Some(commit_sha) = commit_sha {
        content.insert("commit_sha".to_string(), json!(commit_sha));
    }
    let verdict_content = Value::Object(content).to_string();

    let state_manager = StateManager::new(project_path.join(".hive-manager").join(&session_id));
    state_manager
        .write_prince_verdict_async(&prince_id, &queen_id, &verdict_content, commit_sha)
        .await
        .map_err(|err| {
            ApiError::internal(format!(
                "Failed to persist Prince verdict peer record (retry the POST): {}",
                err
            ))
        })?;

    let new_state = {
        let controller = state.session_controller.read();
        controller
            .record_prince_verdict(&session_id, &normalized)
            .map_err(map_verdict_state_error)?
    };

    Ok(Json(json!({
        "session_id": session_id,
        "action": "prince-verdict",
        "verdict": normalized,
        "new_state": format!("{:?}", new_state),
        "commit_sha": commit_sha,
        "rationale": rationale,
        "persisted": true,
        "peer_file_written": true,
    })))
}

/// POST /api/sessions/{id}/milestone-ready
///
/// #175(a): the Queen signals that a milestone is ready for QA review. This opens
/// the QA window and arms the QA timeout. Before this, the clock was armed at
/// session launch, so any session whose first milestone took longer than the
/// timeout was deterministically poisoned to `QaInconclusive` with a BLOCKED
/// verdict for work that had never been submitted.
pub async fn post_milestone_ready(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;

    let controller = state.session_controller.read();
    controller
        .get_session(&session_id)
        .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
    controller
        .on_milestone_ready(&session_id)
        .map_err(ApiError::internal)?;
    let new_state = controller
        .get_session(&session_id)
        .map(|s| format!("{:?}", s.state))
        .unwrap_or_default();
    drop(controller);

    Ok(Json(json!({
        "session_id": session_id,
        "action": "milestone-ready",
        "new_state": new_state,
    })))
}

/// #176: the confirmation guard runs BEFORE the session lookup, so a bodyless
/// POST to a nonexistent session answers with the confirm-400 rather than the
/// 404 `require_qa_overridable` used to produce. That precedence is deliberate
/// and pinned by a test.
pub async fn force_pass(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    body: Result<Json<ForceVerdictRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;
    let req = require_confirmation(body, "force-pass")?;

    let new_state = apply_verdict(
        &state,
        &session_id,
        "QA_VERDICT: PASS",
        true,
        req.rationale.as_deref(),
    )?;

    Ok(Json(json!({
        "session_id": session_id,
        "action": "force-pass",
        "new_state": format!("{:?}", new_state),
        "rationale": req.rationale,
    })))
}

pub async fn force_fail(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    body: Result<Json<ForceVerdictRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&session_id)?;
    let req = require_confirmation(body, "force-fail")?;

    let new_state = apply_verdict(
        &state,
        &session_id,
        "QA_VERDICT: FAIL",
        true,
        req.rationale.as_deref(),
    )?;

    Ok(Json(json!({
        "session_id": session_id,
        "action": "force-fail",
        "new_state": format!("{:?}", new_state),
        "rationale": req.rationale,
    })))
}

#[cfg(test)]
mod tests {
    use super::{
        apply_work_graph_verdict_for_agent, map_add_qa_worker_error, persist_work_graph_verdict,
    };
    use axum::http::StatusCode;
    use tempfile::TempDir;

    use crate::coordination::StateManager;
    use crate::orchestrator::work_graph::completion_ledger::{
        NodeCompletionFact, NodeCompletionProvenance,
    };
    use crate::orchestrator::work_graph::review::{
        instantiate_review_templates, ReviewExpansionSidecar, ReviewTemplate,
    };
    use crate::orchestrator::work_graph::{
        BindingRef, NodeContract, NodeKind, NodeStatus, TaskGraph, WorkNode,
    };

    #[test]
    fn maps_missing_session_to_not_found() {
        let error = map_add_qa_worker_error("Session not found: demo-session".to_string());
        assert_eq!(error.status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn maps_missing_evaluator_to_bad_request() {
        let error = map_add_qa_worker_error(
            "Evaluator demo-evaluator not found for session demo-session".to_string(),
        );
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn maps_spawn_failures_to_internal() {
        let error = map_add_qa_worker_error("Failed to spawn QA worker 1: boom".to_string());
        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn passing_evaluator_verdict_appends_an_exact_completion_fact() {
        let temp = TempDir::new().unwrap();
        let manager = StateManager::new(temp.path().to_path_buf());
        let session_id = format!("evaluator-completion-{}", uuid::Uuid::new_v4());
        let mut graph = TaskGraph::new(
            vec![WorkNode::new(
                "implementation",
                NodeKind::Task,
                "Implementation",
                NodeContract {
                    inputs: Vec::new(),
                    outputs: vec!["code".to_string()],
                    acceptance: vec!["accepted".to_string()],
                },
                BindingRef::Role("worker".to_string()),
                NodeStatus::Pending,
            )],
            Vec::new(),
        );
        let template = ReviewTemplate::code_tasks("qa");
        let expansions = instantiate_review_templates(&mut graph, &[template.clone()]).unwrap();
        let verdict_id = expansions[0].rounds[0].verdict_id.clone();
        let sidecar = ReviewExpansionSidecar::from_expansions(&[template], expansions).unwrap();
        manager.write_work_graph(&graph).unwrap();
        manager.write_review_expansion_sidecar(&sidecar).unwrap();

        let (_, facts) = apply_work_graph_verdict_for_agent(
            &manager,
            &session_id,
            Some(&verdict_id),
            "PASS",
            Some("session-evaluator"),
        )
        .unwrap();

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].task_id, verdict_id);
        assert_eq!(facts[0].agent_id, "session-evaluator");
        assert_eq!(
            facts[0].provenance,
            NodeCompletionProvenance::EvaluatorVerdict
        );
        assert_eq!(manager.read_node_completion_facts().unwrap(), facts);
    }

    #[test]
    fn later_verdict_state_write_failure_does_not_append_completion_fact() {
        let temp = TempDir::new().unwrap();
        let manager = StateManager::new(temp.path().to_path_buf());
        let graph = TaskGraph::new(Vec::new(), Vec::new());
        let sidecar = ReviewExpansionSidecar::default();
        let fact = NodeCompletionFact::new(
            "qa-verdict",
            "session-evaluator",
            NodeCompletionProvenance::EvaluatorVerdict,
        );
        std::fs::create_dir_all(temp.path().join("state/work-graph-reviews.json")).unwrap();

        let error = persist_work_graph_verdict(
            &manager,
            &graph,
            &sidecar,
            None,
            std::slice::from_ref(&fact),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("Failed to persist review expansion sidecar after QA verdict"));
        assert!(manager.read_node_completion_facts().unwrap().is_empty());
    }
}
