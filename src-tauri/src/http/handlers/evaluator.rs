use axum::{
    extract::{rejection::JsonRejection, Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeSet, HashMap},
    fmt, fs, io,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
};
use tempfile::NamedTempFile;

use crate::coordination::{
    criterion_verdict, evaluate, parse_sprint_contract, CompareOp, ContractCriterion,
    CoordinationMessage, CriterionKind, CriterionResult, CriterionValue, SprintContract,
    StateManager, ThresholdPolicy, Verdict,
};
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::judgment::ledger::{
    DecisionInput, Judge, JudgmentLedger, JudgmentMode, OutcomeInput, OutcomeSource,
};
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
    /// Optional typed-contract results. Absence preserves the legacy verdict
    /// path exactly; when present with a parsed contract, all criterion numbers
    /// must be known and covered exactly once.
    #[serde(default)]
    pub criteria: Option<Vec<PostCriterionResult>>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PostCriterionResult {
    pub number: u16,
    pub result: CriterionSubmissionResult,
    pub evidence: String,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CriterionSubmissionResult {
    Pass,
    Fail,
    Scored(f64),
    Measured(f64),
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct QaCriterionVerdictRecord {
    pub number: u16,
    pub label: String,
    pub kind: CriterionKind,
    pub result: CriterionSubmissionResult,
    pub passed: bool,
    pub evidence: String,
    pub evidence_refs: Vec<String>,
    pub decision_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct QaVerdictRecord {
    pub schema_version: u8,
    pub session_id: String,
    pub milestone_id: String,
    pub iteration: u8,
    pub verdict: String,
    pub passed: bool,
    pub summary: String,
    pub timestamp: String,
    pub contract_path: Option<String>,
    pub contract_typed: bool,
    pub omission: Option<String>,
    pub criteria: Vec<QaCriterionVerdictRecord>,
    #[serde(default)]
    pub advisory_verdict: Verdict,
    #[serde(default)]
    pub advisory_disagrees: bool,
    #[serde(default)]
    pub advisory_threshold_disagreement: bool,
    #[serde(default)]
    pub advisory_flip: bool,
    #[serde(default)]
    pub advisory_criteria: Vec<QaAdvisoryCriterionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct QaAdvisoryCriterionRecord {
    pub number: u16,
    pub status: Verdict,
    pub threshold_disagreement: bool,
    pub unchanged_evidence_flip: bool,
    pub state_hash: Option<String>,
}

struct ContractContext {
    path: Option<String>,
    contract: Option<SprintContract>,
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

/// Operator overrides (force-pass / force-fail) are valid throughout the QA lifecycle:
/// in progress, stalled as inconclusive, mid-Prince-remediation, or after a recorded
/// pass/failure. The resolved-state cases let an operator supersede a criterion verdict
/// and produce joined `operator-override` outcomes rather than losing that correction.
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
            | SessionState::QaPassed
            | SessionState::QaFailed { .. }
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
        "Cannot {}: session is in {:?} state, expected a QA state or an evaluator-backed Running session",
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

const QA_VERDICT_RECORD_SCHEMA_VERSION: u8 = 1;
const BLOCKED_CRITERION_OMISSION: &str = "blocked-verdict-no-criterion-rows";

fn judgment_ledger(state: &AppState) -> JudgmentLedger {
    JudgmentLedger::new(
        state
            .storage
            .base_dir()
            .join("judgments")
            .join("ledger.jsonl"),
    )
}

pub(crate) fn qa_verdict_record_path(session_dir: &FsPath) -> PathBuf {
    session_dir.join("state").join("qa-verdict.json")
}

pub(crate) fn read_qa_verdict_record(
    session_dir: &FsPath,
) -> io::Result<Option<QaVerdictRecord>> {
    let path = qa_verdict_record_path(session_dir);
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn write_qa_verdict_record(
    session_dir: &FsPath,
    record: &QaVerdictRecord,
) -> io::Result<()> {
    let path = qa_verdict_record_path(session_dir);
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("QA verdict record has no parent directory"))?;
    fs::create_dir_all(parent)?;
    let mut temp = NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(temp.as_file_mut(), record).map_err(io::Error::other)?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn contract_path_from_handoff(project_path: &FsPath, session_root: &FsPath) -> Option<PathBuf> {
    let handoff = fs::read(session_root.join("peer").join("milestone-ready.json")).ok()?;
    let handoff: Value = serde_json::from_slice(&handoff).ok()?;
    let content = handoff.get("content")?.as_str()?;
    let value = content.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case("contract")
            .then(|| value.trim())
    })?;
    if value.is_empty() {
        return None;
    }

    let named = PathBuf::from(value);
    if named.is_absolute() {
        return Some(named);
    }
    if named.starts_with(".hive-manager") {
        return Some(project_path.join(named));
    }
    Some(session_root.join(named))
}

fn path_is_within_session(candidate: &FsPath, session_root: &FsPath) -> bool {
    let Ok(candidate) = fs::canonicalize(candidate) else {
        return false;
    };
    let Ok(session_root) = fs::canonicalize(session_root) else {
        return false;
    };
    candidate.starts_with(session_root)
}

fn load_contract_context(
    project_path: &FsPath,
    session_root: &FsPath,
    state_manager: &StateManager,
    session_id: &str,
) -> ContractContext {
    if let Some(path) = contract_path_from_handoff(project_path, session_root) {
        let contract_display = path.to_string_lossy().to_string();
        if !path_is_within_session(&path, session_root) {
            tracing::warn!(
                %session_id,
                contract = contract_display.as_str(),
                "Ignoring milestone contract path outside the project-scoped session directory"
            );
            return ContractContext {
                path: Some(contract_display),
                contract: None,
            };
        }
        match fs::read_to_string(&path) {
            Ok(markdown) => match parse_sprint_contract(&markdown) {
                Ok(contract) => {
                    return ContractContext {
                        path: Some(contract_display),
                        contract: Some(contract),
                    };
                }
                Err(error) => {
                    tracing::warn!(
                        %session_id,
                        contract = contract_display.as_str(),
                        %error,
                        "QA contract is untyped; preserving the legacy verdict path"
                    );
                    return ContractContext {
                        path: Some(contract_display),
                        contract: None,
                    };
                }
            },
            Err(error) => {
                tracing::warn!(
                    %session_id,
                    contract = contract_display.as_str(),
                    %error,
                    "Named QA contract is unreadable; preserving the legacy verdict path"
                );
                return ContractContext {
                    path: Some(contract_display),
                    contract: None,
                };
            }
        }
    }

    let fallback = session_root.join("contracts").join("milestone-1.md");
    let fallback_display = "contracts/milestone-1.md".to_string();
    match state_manager.read_contract(1) {
        Ok(Some(contract)) => ContractContext {
            path: Some(fallback_display),
            contract: Some(contract),
        },
        Ok(None) => ContractContext {
            path: None,
            contract: None,
        },
        Err(error) => {
            tracing::warn!(
                %session_id,
                contract = %fallback.display(),
                %error,
                "Fallback QA contract is untyped; preserving the legacy verdict path"
            );
            ContractContext {
                path: Some(fallback_display),
                contract: None,
            }
        }
    }
}

fn validated_criteria(
    contract: &SprintContract,
    submitted: &[PostCriterionResult],
) -> Result<Vec<(ContractCriterion, PostCriterionResult)>, ApiError> {
    let known: BTreeSet<u16> = contract
        .acceptance_criteria
        .iter()
        .map(|criterion| criterion.number)
        .collect();
    let mut by_number = HashMap::new();
    let mut duplicates = BTreeSet::new();
    for criterion in submitted {
        if by_number
            .insert(criterion.number, criterion.clone())
            .is_some()
        {
            duplicates.insert(criterion.number);
        }
    }
    let submitted_numbers: BTreeSet<u16> = by_number.keys().copied().collect();
    let unknown: Vec<u16> = submitted_numbers.difference(&known).copied().collect();
    if !unknown.is_empty() {
        return Err(ApiError::bad_request(format!(
            "Unknown QA criterion numbers: {}",
            unknown
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if !duplicates.is_empty() {
        return Err(ApiError::bad_request(format!(
            "Duplicate QA criterion numbers: {}",
            duplicates
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let missing: Vec<u16> = known.difference(&submitted_numbers).copied().collect();
    if !missing.is_empty() {
        return Err(ApiError::bad_request(format!(
            "Missing QA criterion numbers: {}",
            missing
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    Ok(contract
        .acceptance_criteria
        .iter()
        .map(|criterion| {
            (
                criterion.clone(),
                by_number
                    .remove(&criterion.number)
                    .expect("known criterion was validated as present"),
            )
        })
        .collect())
}

fn criterion_passed(
    kind: &CriterionKind,
    result: &CriterionSubmissionResult,
) -> bool {
    match (kind, result) {
        (_, CriterionSubmissionResult::Pass) => true,
        (_, CriterionSubmissionResult::Fail | CriterionSubmissionResult::Blocked) => false,
        (
            CriterionKind::Scored { min, max, floor },
            CriterionSubmissionResult::Scored(value),
        ) => {
            let lower_bound = floor.unwrap_or(*min);
            value.is_finite()
                && *value >= f64::from(lower_bound)
                && *value <= f64::from(*max)
        }
        (
            CriterionKind::Measured { op, target, .. },
            CriterionSubmissionResult::Measured(value),
        ) if value.is_finite() && target.is_finite() => match op {
            CompareOp::Lt => value < target,
            CompareOp::Le => value <= target,
            CompareOp::Eq => value == target,
            CompareOp::Ge => value >= target,
            CompareOp::Gt => value > target,
        },
        _ => false,
    }
}

fn criterion_value(result: &CriterionSubmissionResult) -> CriterionValue {
    match result {
        CriterionSubmissionResult::Pass => CriterionValue::PassFail(true),
        CriterionSubmissionResult::Fail => CriterionValue::PassFail(false),
        CriterionSubmissionResult::Scored(value) => CriterionValue::Scored(*value),
        CriterionSubmissionResult::Measured(value) => CriterionValue::Measured(*value),
        CriterionSubmissionResult::Blocked => CriterionValue::Unspecified,
    }
}

fn base_verdict_record(
    session_id: &str,
    verdict: &str,
    rationale: Option<&str>,
    iteration: u8,
    context: &ContractContext,
) -> QaVerdictRecord {
    QaVerdictRecord {
        schema_version: QA_VERDICT_RECORD_SCHEMA_VERSION,
        session_id: session_id.to_string(),
        milestone_id: context
            .contract
            .as_ref()
            .map(|contract| contract.milestone_name.clone())
            .unwrap_or_else(|| "milestone-1".to_string()),
        iteration,
        verdict: verdict.to_string(),
        passed: verdict == "PASS",
        summary: rationale
            .map(str::to_string)
            .unwrap_or_else(|| format!("QA verdict: {verdict}")),
        timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        contract_path: context.path.clone(),
        contract_typed: context.contract.is_some(),
        omission: (verdict == "BLOCKED").then(|| BLOCKED_CRITERION_OMISSION.to_string()),
        criteria: Vec::new(),
        advisory_verdict: Verdict::Undetermined,
        advisory_disagrees: false,
        advisory_threshold_disagreement: false,
        advisory_flip: false,
        advisory_criteria: Vec::new(),
    }
}

fn persist_verdict_record_fail_open(
    state: &AppState,
    session_id: &str,
    record: &QaVerdictRecord,
) {
    if let Err(error) = write_qa_verdict_record(&state.storage.session_dir(session_id), record) {
        tracing::warn!(
            %session_id,
            %error,
            "Failed to persist per-criterion QA verdict record"
        );
    }
}

fn record_typed_criteria(
    state: &AppState,
    session_id: &str,
    verdict: &str,
    rationale: Option<&str>,
    evaluator_model: &str,
    record: &mut QaVerdictRecord,
    criteria: Vec<(ContractCriterion, PostCriterionResult)>,
) -> Vec<Option<String>> {
    let ledger = judgment_ledger(state);
    let mut state_hashes = Vec::with_capacity(criteria.len());
    for (criterion, submitted) in criteria {
        let mut subject_ref = Map::new();
        subject_ref.insert("session_id".to_string(), json!(session_id));
        subject_ref.insert("milestone_id".to_string(), json!(record.milestone_id));
        subject_ref.insert("criterion_number".to_string(), json!(criterion.number));
        let observations = json!({
            "criterion": {
                "number": criterion.number,
                "description": criterion.description,
                "kind": criterion.kind,
            },
            "evidence": submitted.evidence,
            "evidence_refs": submitted.evidence_refs,
        });
        let answer = json!({
            "result": submitted.result,
            "rationale": rationale,
        });
        let decision = ledger.record_decision(DecisionInput {
            decision_id: None,
            surface: "hive.qa.criterion".to_string(),
            subject_ref,
            observations,
            answer,
            question_id: Some("hive.qa.criterion".to_string()),
            question_version: None,
            judge: Judge::IncumbentLlm,
            model: Some(evaluator_model.to_string()),
            sampling: None,
            mode: JudgmentMode::Shadow,
            probabilities: None,
            confidence: None,
            threshold_id: Some(record.milestone_id.clone()),
            routed: format!("gated-{}", verdict.to_ascii_lowercase()),
            latency_ms: None,
            cost_usd: None,
            error: None,
            extra: Map::new(),
        });
        state_hashes.push(decision.as_ref().and_then(|decision| decision.state_hash.clone()));
        record.criteria.push(QaCriterionVerdictRecord {
            number: criterion.number,
            label: criterion.description,
            passed: criterion_passed(&criterion.kind, &submitted.result),
            kind: criterion.kind,
            result: submitted.result,
            evidence: submitted.evidence,
            evidence_refs: submitted.evidence_refs,
            decision_id: decision.map(|decision| decision.decision_id),
        });
    }
    state_hashes
}

fn record_operator_override_outcomes(
    state: &AppState,
    session_id: &str,
    verdict: &str,
    rationale: Option<&str>,
) {
    let record = match read_qa_verdict_record(&state.storage.session_dir(session_id)) {
        Ok(Some(record)) => record,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(
                %session_id,
                %error,
                "Failed to read QA verdict record for operator outcomes"
            );
            return;
        }
    };
    let label = if verdict.trim().to_ascii_uppercase().contains("PASS") {
        "pass"
    } else {
        "fail"
    };
    let ledger = judgment_ledger(state);
    for criterion in record.criteria {
        let Some(decision_id) = criterion.decision_id else {
            continue;
        };
        let mut extra = Map::new();
        extra.insert("session_id".to_string(), json!(session_id));
        extra.insert("criterion_number".to_string(), json!(criterion.number));
        let _ = ledger.record_outcome(OutcomeInput {
            decision_id,
            label: json!(label),
            source: OutcomeSource::OperatorOverride,
            note: rationale.map(str::to_string),
            extra,
        });
    }
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

/// Test-facing graph-verdict boundary for exact-id routing. Session-wide QA
/// must never select a review join by position, kind, or title.
#[cfg(test)]
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
        record_operator_override_outcomes(state, session_id, verdict, rationale);
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
    let (project_path, evaluator_id, queen_id, evaluator_model, iteration) = {
        let controller = state.session_controller.read();
        require_qa_in_progress(&controller, &session_id, "qa-verdict")?;
        let session = controller
            .get_session(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
        let evaluator = session
            .agents
            .iter()
            .find(|agent| matches!(agent.role, AgentRole::Evaluator));
        let evaluator_id = evaluator
            .map(|agent| agent.id.clone())
            .unwrap_or_else(|| format!("{}-evaluator", session_id));
        let evaluator_model = evaluator
            .map(|agent| match agent.config.model.as_deref() {
                Some(model) => format!("{}/{}", agent.config.cli, model),
                None => agent.config.cli.clone(),
            })
            .unwrap_or_else(|| match session.default_model.as_deref() {
                Some(model) => format!("{}/{}", session.default_cli, model),
                None => session.default_cli.clone(),
            });
        let iteration = match session.state {
            SessionState::QaInProgress { iteration } => iteration.unwrap_or(1),
            _ => 1,
        };
        (
            session.project_path.clone(),
            evaluator_id,
            format!("{}-queen", session_id),
            evaluator_model,
            iteration,
        )
    };

    let session_root = project_path.join(".hive-manager").join(&session_id);
    let state_manager = StateManager::new(session_root.clone());
    let contract_context = load_contract_context(
        &project_path,
        &session_root,
        &state_manager,
        &session_id,
    );
    let typed_criteria = if verdict != "BLOCKED" {
        match (&contract_context.contract, &req.criteria) {
            (Some(contract), Some(criteria)) => Some(validated_criteria(contract, criteria)?),
            _ => None,
        }
    } else {
        None
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
        let record = base_verdict_record(
            &session_id,
            verdict,
            Some(&reason),
            iteration,
            &contract_context,
        );
        persist_verdict_record_fail_open(&state, &session_id, &record);
        tracing::warn!(
            %session_id,
            omission = BLOCKED_CRITERION_OMISSION,
            "BLOCKED QA verdict intentionally omitted criterion judgment rows"
        );
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

    // Persist the record before the session transition. T13 consumes these
    // decision ids synchronously when that transition reaches a terminal QA
    // state, while work-graph validation above must still be able to reject the
    // request without leaving duplicate judgment rows behind for a retry.
    let mut verdict_record = base_verdict_record(
        &session_id,
        verdict,
        rationale,
        iteration,
        &contract_context,
    );
    if let Some(criteria) = typed_criteria {
        let prior_record = match read_qa_verdict_record(&state.storage.session_dir(&session_id)) {
            Ok(record) => record,
            Err(error) => {
                tracing::warn!(%session_id, %error, "Failed to read prior QA advisory record");
                None
            }
        };
        let results: Vec<CriterionResult> = criteria
            .iter()
            .map(|(criterion, submitted)| CriterionResult {
                kind: criterion.kind.clone(),
                value: criterion_value(&submitted.result),
            })
            .collect();
        let policy = &contract_context
            .contract
            .as_ref()
            .expect("typed criteria require a contract")
            .threshold_policy;
        verdict_record.advisory_verdict = evaluate(policy, &results);
        verdict_record.advisory_disagrees = match verdict_record.advisory_verdict {
            Verdict::Pass => verdict != "PASS",
            Verdict::Fail => verdict != "FAIL",
            Verdict::Undetermined => false,
        };
        let floor_enabled = matches!(
            policy,
            ThresholdPolicy::Rules {
                fail_scored_below_floor: true,
                ..
            }
        );
        verdict_record.advisory_criteria = criteria
            .iter()
            .zip(&results)
            .map(|((criterion, submitted), result)| {
                let status = criterion_verdict(result);
                let threshold_disagreement = matches!(
                    (&criterion.kind, &submitted.result),
                    (
                        CriterionKind::Scored {
                            min,
                            floor: Some(floor),
                            ..
                        },
                        CriterionSubmissionResult::Scored(value)
                    ) if !floor_enabled
                        && value.is_finite()
                        && *value >= f64::from(*min)
                        && *value < f64::from(*floor)
                        && status == Verdict::Pass
                        && !criterion_passed(&criterion.kind, &submitted.result)
                );
                QaAdvisoryCriterionRecord {
                    number: criterion.number,
                    status,
                    threshold_disagreement,
                    unchanged_evidence_flip: false,
                    state_hash: None,
                }
            })
            .collect();
        let state_hashes = record_typed_criteria(
            &state,
            &session_id,
            verdict,
            rationale,
            &evaluator_model,
            &mut verdict_record,
            criteria,
        );
        let mut flips = Vec::new();
        for ((current, advisory), state_hash) in verdict_record
            .criteria
            .iter()
            .zip(&mut verdict_record.advisory_criteria)
            .zip(state_hashes)
        {
            advisory.state_hash = state_hash;
            let prior = prior_record.as_ref().filter(|prior| {
                prior.milestone_id == verdict_record.milestone_id
                    && prior.iteration < verdict_record.iteration
            });
            let previous_criterion = prior.and_then(|prior| {
                prior.criteria.iter().find(|item| item.number == current.number)
            });
            let previous_hash = prior.and_then(|prior| {
                prior
                    .advisory_criteria
                    .iter()
                    .find(|item| item.number == current.number)
                    .and_then(|item| item.state_hash.as_ref())
            });
            if advisory.state_hash.is_some()
                && advisory.state_hash.as_ref() == previous_hash
                && previous_criterion.is_some_and(|item| item.result != current.result)
            {
                advisory.unchanged_evidence_flip = true;
                flips.push(json!({
                    "criterion_number": current.number,
                    "prior_decision_id": previous_criterion.and_then(|item| item.decision_id.as_ref()),
                    "current_decision_id": current.decision_id,
                }));
            }
        }
        verdict_record.advisory_flip = !flips.is_empty();
        verdict_record.advisory_threshold_disagreement = verdict_record.advisory_disagrees
            && verdict_record
                .advisory_criteria
                .iter()
                .any(|item| item.threshold_disagreement)
            && verdict_record
                .criteria
                .iter()
                .zip(&verdict_record.advisory_criteria)
                .all(|(display, advisory)| {
                    advisory.status != Verdict::Undetermined
                        && ((advisory.status == Verdict::Pass) == display.passed
                            || advisory.threshold_disagreement)
                });
        if verdict_record.advisory_disagrees || verdict_record.advisory_flip {
            let mut subject_ref = Map::new();
            subject_ref.insert("session_id".to_string(), json!(session_id));
            subject_ref.insert("milestone_id".to_string(), json!(verdict_record.milestone_id));
            subject_ref.insert("iteration".to_string(), json!(iteration));
            let mut extra = Map::new();
            extra.insert(
                "source_decision_ids".to_string(),
                json!(verdict_record
                    .criteria
                    .iter()
                    .filter_map(|item| item.decision_id.as_ref())
                    .collect::<Vec<_>>()),
            );
            extra.insert("unchanged_evidence_flips".to_string(), json!(flips));
            extra.insert(
                "threshold_disagreement".to_string(),
                json!(verdict_record.advisory_threshold_disagreement),
            );
            judgment_ledger(&state).record_decision(DecisionInput {
                decision_id: None,
                surface: "hive.qa.milestone".to_string(),
                subject_ref,
                observations: json!({
                    "contract_path": verdict_record.contract_path,
                    "criterion_numbers": verdict_record.criteria.iter().map(|item| item.number).collect::<Vec<_>>(),
                }),
                answer: json!({
                    "evaluator_verdict": verdict,
                    "advisory_verdict": verdict_record.advisory_verdict,
                    "disagrees": verdict_record.advisory_disagrees,
                }),
                question_id: Some("hive.qa.milestone".to_string()),
                question_version: None,
                judge: Judge::Code,
                model: None,
                sampling: None,
                mode: JudgmentMode::Advisory,
                probabilities: None,
                confidence: None,
                threshold_id: Some(verdict_record.milestone_id.clone()),
                routed: "advisory-shown".to_string(),
                latency_ms: None,
                cost_usd: None,
                error: None,
                extra,
            });
        }
    }
    persist_verdict_record_fail_open(&state, &session_id, &verdict_record);

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
    use std::collections::BTreeSet;

    use super::{
        apply_work_graph_verdict_for_agent, load_contract_context, map_add_qa_worker_error,
        persist_work_graph_verdict, qa_verdict_record_path, read_qa_verdict_record,
        CriterionSubmissionResult,
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::coordination::StateManager;
    use crate::http::tests::{make_test_session, setup_test_app_with_controller_at};
    use crate::orchestrator::work_graph::completion_ledger::{
        NodeCompletionFact, NodeCompletionProvenance,
    };
    use crate::orchestrator::work_graph::review::{
        instantiate_review_templates, ReviewExpansionSidecar, ReviewTemplate,
    };
    use crate::orchestrator::work_graph::{
        BindingRef, NodeContract, NodeKind, NodeStatus, TaskGraph, WorkNode,
    };
    use crate::pty::{AgentConfig, AgentRole, AgentStatus};
    use crate::session::{AgentInfo, SessionState};

    fn install_typed_contract(project: &std::path::Path, session_id: &str) {
        let contracts = project
            .join(".hive-manager")
            .join(session_id)
            .join("contracts");
        std::fs::create_dir_all(&contracts).unwrap();
        std::fs::write(
            contracts.join("milestone-1.md"),
            "# Sprint Contract: Typed HTTP QA\n\n\
             ## Acceptance Criteria\n\
             1. [FUNC] The endpoint records functional evidence\n\
             2. [DESIGN 1-10 floor 5] The result preserves a design score\n\n\
             ## Pass Threshold\n\
             - All pass/fail criteria must pass\n\
             - Any scored criterion below its floor fails\n",
        )
        .unwrap();
    }

    fn evaluator_agent(session_id: &str) -> AgentInfo {
        AgentInfo {
            id: format!("{session_id}-evaluator"),
            role: AgentRole::Evaluator,
            status: AgentStatus::Running,
            config: AgentConfig {
                cli: "codex".to_string(),
                model: Some("gpt-test".to_string()),
                ..AgentConfig::default()
            },
            parent_id: None,
            role_definition_id: None,
            role_definition_version: None,
            commit_sha: None,
            base_commit_sha: None,
        }
    }

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
    fn scored_criterion_without_floor_uses_declared_range() {
        let kind = crate::coordination::CriterionKind::Scored {
            min: 1,
            max: 10,
            floor: None,
        };

        assert!(super::criterion_passed(
            &kind,
            &super::CriterionSubmissionResult::Scored(1.0)
        ));
        assert!(super::criterion_passed(
            &kind,
            &super::CriterionSubmissionResult::Scored(10.0)
        ));
        assert!(!super::criterion_passed(
            &kind,
            &super::CriterionSubmissionResult::Scored(0.99)
        ));
        assert!(!super::criterion_passed(
            &kind,
            &super::CriterionSubmissionResult::Scored(10.01)
        ));
        assert!(!super::criterion_passed(
            &kind,
            &super::CriterionSubmissionResult::Scored(f64::NAN)
        ));
    }

    #[test]
    fn contract_path_is_resolved_from_milestone_handoff_before_fallback() {
        let project = TempDir::new().unwrap();
        let session_id = "named-contract-session";
        let session_root = project.path().join(".hive-manager").join(session_id);
        std::fs::create_dir_all(session_root.join("peer")).unwrap();
        std::fs::create_dir_all(session_root.join("contracts")).unwrap();
        std::fs::write(
            session_root.join("contracts").join("release.md"),
            "# Sprint Contract: Named Release\n\n\
             ## Acceptance Criteria\n\
             7. [FUNC] Named contract selected\n\n\
             ## Pass Threshold\n\
             - All pass/fail criteria must pass\n",
        )
        .unwrap();
        std::fs::write(
            session_root.join("peer").join("milestone-ready.json"),
            r#"{"content":"MILESTONE_READY\ncontract: contracts/release.md\nscope: named"}"#,
        )
        .unwrap();
        let manager = StateManager::new(session_root.clone());

        let context = load_contract_context(project.path(), &session_root, &manager, session_id);

        assert!(context
            .path
            .as_deref()
            .is_some_and(|path| path.ends_with("contracts\\release.md")
                || path.ends_with("contracts/release.md")));
        let contract = context.contract.unwrap();
        assert_eq!(contract.milestone_name, "Named Release");
        assert_eq!(contract.acceptance_criteria[0].number, 7);
    }

    #[tokio::test]
    async fn typed_http_verdict_writes_decisions_evidence_record_and_override_outcomes() {
        let storage = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let (app, controller, session_storage) =
            setup_test_app_with_controller_at(storage.path().to_path_buf()).await;
        let session_id = format!("typed-qa-{}", uuid::Uuid::new_v4());
        install_typed_contract(project.path(), &session_id);
        let mut session = make_test_session(&session_id, project.path().to_str().unwrap());
        session.state = SessionState::QaInProgress { iteration: Some(2) };
        session.agents.push(evaluator_agent(&session_id));
        controller.write().insert_test_session(session);

        let request = serde_json::json!({
            "verdict": "PASS",
            "rationale": "Typed criteria passed",
            "criteria": [
                {
                    "number": 1,
                    "result": "pass",
                    "evidence": "API worker observed a durable record",
                    "evidence_refs": ["qa-worker-api#1"]
                },
                {
                    "number": 2,
                    "result": {"scored": 8},
                    "evidence": "Design worker scored the rendered result",
                    "evidence_refs": ["qa-worker-ui#2"]
                }
            ]
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{session_id}/qa/verdict"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let verdict_record = read_qa_verdict_record(&session_storage.session_dir(&session_id))
            .unwrap()
            .unwrap();
        assert_eq!(verdict_record.iteration, 2);
        assert_eq!(verdict_record.milestone_id, "Typed HTTP QA");
        assert_eq!(verdict_record.criteria.len(), 2);
        assert!(verdict_record.criteria.iter().all(|criterion| criterion
            .decision_id
            .is_some()));

        let judgments = storage.path().join("judgments");
        let ledger_path = judgments.join("ledger.jsonl");
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(&ledger_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let decision_rows: Vec<_> = rows
            .iter()
            .filter(|row| row["kind"] == "decision")
            .collect();
        assert_eq!(decision_rows.len(), 2);
        assert!(decision_rows
            .iter()
            .all(|row| row["judge"] == "incumbent-llm"));
        assert!(decision_rows.iter().all(|row| row["mode"] == "shadow"));
        assert!(decision_rows.iter().all(|row| row["sampling"].is_null()));
        assert!(decision_rows
            .iter()
            .all(|row| row["model"] == "codex/gpt-test"));
        for row in &decision_rows {
            let evidence = std::fs::read_to_string(judgments.join(row["state_ref"].as_str().unwrap()))
                .unwrap();
            let evidence: serde_json::Value = serde_json::from_str(&evidence).unwrap();
            assert!(evidence.get("result").is_none());
            assert!(evidence.get("rationale").is_none());
            assert!(!evidence.to_string().contains("Typed criteria passed"));
        }
        let downstream_outcomes: Vec<_> = rows
            .iter()
            .filter(|row| row["kind"] == "outcome" && row["source"] == "downstream")
            .collect();
        assert_eq!(downstream_outcomes.len(), 2);
        assert!(downstream_outcomes
            .iter()
            .all(|row| row["label"] == "pass"));
        let expected_decision_ids: BTreeSet<_> = verdict_record
            .criteria
            .iter()
            .filter_map(|criterion| criterion.decision_id.as_deref())
            .collect();
        let downstream_decision_ids: BTreeSet<_> = downstream_outcomes
            .iter()
            .filter_map(|row| row["decision_id"].as_str())
            .collect();
        assert_eq!(downstream_decision_ids, expected_decision_ids);

        let override_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{session_id}/qa/force-fail"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"confirm":true,"rationale":"Operator found a regression"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(override_response.status(), StatusCode::OK);
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(&ledger_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let outcomes: Vec<_> = rows
            .iter()
            .filter(|row| {
                row["kind"] == "outcome" && row["source"] == "operator-override"
            })
            .collect();
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes
            .iter()
            .all(|row| row["source"] == "operator-override"));
        assert!(outcomes
            .iter()
            .all(|row| row["label"] == "fail"));
        assert!(outcomes
            .iter()
            .all(|row| row["note"] == "Operator found a regression"));
    }

    #[tokio::test]
    async fn typed_advisory_disagreement_writes_one_row_and_projects_to_panel() {
        let storage = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let (app, controller, session_storage) =
            setup_test_app_with_controller_at(storage.path().to_path_buf()).await;
        let session_id = format!("advisory-qa-{}", uuid::Uuid::new_v4());
        install_typed_contract(project.path(), &session_id);
        let mut session = make_test_session(&session_id, project.path().to_str().unwrap());
        session.state = SessionState::QaInProgress { iteration: Some(2) };
        session.agents.push(evaluator_agent(&session_id));
        controller.write().insert_test_session(session);

        let request = serde_json::json!({
            "verdict": "PASS",
            "criteria": [
                {"number": 1, "result": "pass", "evidence": "stable evidence"},
                {"number": 2, "result": {"scored": 2}, "evidence": "design score"}
            ]
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{session_id}/qa/verdict"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let record = read_qa_verdict_record(&session_storage.session_dir(&session_id))
            .unwrap()
            .unwrap();
        assert_eq!(record.advisory_verdict, crate::coordination::Verdict::Fail);
        assert!(record.advisory_disagrees);
        assert!(record.passed);
        let panel = crate::qa_commands::load_qa_verdict(&session_storage, &session_id)
            .unwrap()
            .unwrap();
        assert_eq!(panel.advisory_verdict, crate::coordination::Verdict::Fail);
        assert!(panel.advisory_disagrees);
        assert!(panel.passed);
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(
            storage.path().join("judgments").join("ledger.jsonl"),
        )
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
        let advisory: Vec<_> = rows
            .iter()
            .filter(|row| row["surface"] == "hive.qa.milestone")
            .collect();
        assert_eq!(advisory.len(), 1);
        assert_eq!(advisory[0]["judge"], "code");
        assert_eq!(advisory[0]["mode"], "advisory");
        assert_eq!(advisory[0]["routed"], "advisory-shown");
        assert_eq!(
            advisory[0]["source_decision_ids"].as_array().unwrap().len(),
            2
        );
    }

    #[tokio::test]
    async fn prose_threshold_is_undetermined_without_advisory_disagreement() {
        let storage = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let (app, controller, session_storage) =
            setup_test_app_with_controller_at(storage.path().to_path_buf()).await;
        let session_id = format!("prose-qa-{}", uuid::Uuid::new_v4());
        let contracts = project
            .path()
            .join(".hive-manager")
            .join(&session_id)
            .join("contracts");
        std::fs::create_dir_all(&contracts).unwrap();
        std::fs::write(
            contracts.join("milestone-1.md"),
            "# Sprint Contract: Prose Threshold\n\n\
             ## Acceptance Criteria\n\
             1. [FUNC] The endpoint records functional evidence\n\n\
             ## Pass Threshold\n\
             - Human judgment applies\n",
        )
        .unwrap();
        let mut session = make_test_session(&session_id, project.path().to_str().unwrap());
        session.state = SessionState::QaInProgress { iteration: Some(1) };
        session.agents.push(evaluator_agent(&session_id));
        controller.write().insert_test_session(session);

        let request = serde_json::json!({
            "verdict": "PASS",
            "criteria": [
                {"number": 1, "result": "pass", "evidence": "human-reviewed evidence"}
            ]
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{session_id}/qa/verdict"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let record = read_qa_verdict_record(&session_storage.session_dir(&session_id))
            .unwrap()
            .unwrap();
        assert_eq!(record.advisory_verdict, crate::coordination::Verdict::Undetermined);
        assert!(!record.advisory_disagrees);
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(
            storage.path().join("judgments").join("ledger.jsonl"),
        )
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
        assert!(rows.iter().all(|row| row["surface"] != "hive.qa.milestone"));
        let criterion = rows
            .iter()
            .find(|row| row["surface"] == "hive.qa.criterion")
            .unwrap();
        assert_eq!(
            record.advisory_criteria[0].state_hash.as_deref(),
            criterion["state_hash"].as_str()
        );
    }

    #[tokio::test]
    async fn unchanged_evidence_flip_is_flagged_without_replacing_current_result() {
        let storage = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let (app, controller, session_storage) =
            setup_test_app_with_controller_at(storage.path().to_path_buf()).await;
        let session_id = format!("flip-qa-{}", uuid::Uuid::new_v4());
        install_typed_contract(project.path(), &session_id);

        for (iteration, verdict, result) in [(2, "PASS", "pass"), (3, "FAIL", "fail")] {
            let mut session = make_test_session(&session_id, project.path().to_str().unwrap());
            session.state = SessionState::QaInProgress {
                iteration: Some(iteration),
            };
            session.agents.push(evaluator_agent(&session_id));
            controller.write().insert_test_session(session);
            let request = serde_json::json!({
                "verdict": verdict,
                "criteria": [
                    {"number": 1, "result": result, "evidence": "unchanged evidence"},
                    {"number": 2, "result": {"scored": 8}, "evidence": "unchanged design evidence"}
                ]
            });
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/sessions/{session_id}/qa/verdict"))
                        .header("content-type", "application/json")
                        .body(Body::from(serde_json::to_vec(&request).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        let record = read_qa_verdict_record(&session_storage.session_dir(&session_id))
            .unwrap()
            .unwrap();
        assert_eq!(record.iteration, 3);
        assert_eq!(record.criteria[0].result, CriterionSubmissionResult::Fail);
        assert!(record.advisory_flip);
        assert!(record.advisory_criteria[0].unchanged_evidence_flip);
        assert!(!record.advisory_disagrees);
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(
            storage.path().join("judgments").join("ledger.jsonl"),
        )
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
        let advisory: Vec<_> = rows
            .iter()
            .filter(|row| row["surface"] == "hive.qa.milestone")
            .collect();
        assert_eq!(advisory.len(), 1);
        assert_eq!(
            advisory[0]["unchanged_evidence_flips"][0]["criterion_number"],
            1
        );
    }

    #[tokio::test]
    async fn scored_floor_display_difference_is_labelled_as_threshold_disagreement() {
        let storage = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let (app, controller, session_storage) =
            setup_test_app_with_controller_at(storage.path().to_path_buf()).await;
        let session_id = format!("floor-qa-{}", uuid::Uuid::new_v4());
        let contracts = project
            .path()
            .join(".hive-manager")
            .join(&session_id)
            .join("contracts");
        std::fs::create_dir_all(&contracts).unwrap();
        std::fs::write(
            contracts.join("milestone-1.md"),
            "# Sprint Contract: Floor Display\n\n\
             ## Acceptance Criteria\n\
             1. [DESIGN 1-10 floor 5] The design is scored\n\n\
             ## Pass Threshold\n\
             - All pass/fail criteria must pass\n",
        )
        .unwrap();
        let mut session = make_test_session(&session_id, project.path().to_str().unwrap());
        session.state = SessionState::QaInProgress { iteration: Some(1) };
        session.agents.push(evaluator_agent(&session_id));
        controller.write().insert_test_session(session);

        let request = serde_json::json!({
            "verdict": "FAIL",
            "criteria": [
                {"number": 1, "result": {"scored": 3}, "evidence": "score is in range but below floor"}
            ]
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{session_id}/qa/verdict"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let record = read_qa_verdict_record(&session_storage.session_dir(&session_id))
            .unwrap()
            .unwrap();
        assert!(!record.criteria[0].passed);
        assert_eq!(record.advisory_criteria[0].status, crate::coordination::Verdict::Pass);
        assert!(record.advisory_criteria[0].threshold_disagreement);
        assert_eq!(record.advisory_verdict, crate::coordination::Verdict::Pass);
        assert!(record.advisory_disagrees);
        assert!(record.advisory_threshold_disagreement);
    }

    #[tokio::test]
    async fn unknown_typed_criterion_is_rejected_before_any_mutation() {
        let storage = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let (app, controller, session_storage) =
            setup_test_app_with_controller_at(storage.path().to_path_buf()).await;
        let session_id = format!("unknown-qa-{}", uuid::Uuid::new_v4());
        install_typed_contract(project.path(), &session_id);
        let mut session = make_test_session(&session_id, project.path().to_str().unwrap());
        session.state = SessionState::QaInProgress { iteration: Some(1) };
        session.agents.push(evaluator_agent(&session_id));
        controller.write().insert_test_session(session);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{session_id}/qa/verdict"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"verdict":"PASS","criteria":[{"number":99,"result":"pass","evidence":"not in contract"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("99"));
        assert!(matches!(
            controller.read().get_session(&session_id).unwrap().state,
            SessionState::QaInProgress { .. }
        ));
        assert!(!storage.path().join("judgments").join("ledger.jsonl").exists());
        assert!(!qa_verdict_record_path(&session_storage.session_dir(&session_id)).exists());
        assert!(!project
            .path()
            .join(".hive-manager")
            .join(&session_id)
            .join("peer")
            .join("qa-verdict.json")
            .exists());
    }

    #[tokio::test]
    async fn untyped_verdict_keeps_legacy_http_peer_state_and_project_shape() {
        let storage = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let (app, controller, session_storage) =
            setup_test_app_with_controller_at(storage.path().to_path_buf()).await;
        let session_id = format!("untyped-qa-{}", uuid::Uuid::new_v4());
        let mut session = make_test_session(&session_id, project.path().to_str().unwrap());
        session.state = SessionState::QaInProgress { iteration: None };
        session.agents.push(evaluator_agent(&session_id));
        controller.write().insert_test_session(session);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{session_id}/qa/verdict"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"verdict":"PASS","commit_sha":"legacy-sha","rationale":"Legacy bare verdict"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["verdict"], "PASS");
        assert_eq!(body["new_state"], "QaPassed");
        assert_eq!(body["commit_sha"], "legacy-sha");
        assert_eq!(body["rationale"], "Legacy bare verdict");

        let peer_path = project
            .path()
            .join(".hive-manager")
            .join(&session_id)
            .join("peer")
            .join("qa-verdict.json");
        let peer: crate::coordination::PeerMessageRecord =
            serde_json::from_slice(&std::fs::read(&peer_path).unwrap()).unwrap();
        assert_eq!(
            peer.content,
            r#"{"commit_sha":"legacy-sha","kind":"qa-verdict","rationale":"Legacy bare verdict","verdict":"PASS"}"#
        );
        let session_root = project.path().join(".hive-manager").join(&session_id);
        let session_entries: Vec<_> = std::fs::read_dir(&session_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(session_entries, vec![std::ffi::OsString::from("peer")]);
        let peer_entries: Vec<_> = std::fs::read_dir(session_root.join("peer"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(
            peer_entries,
            vec![std::ffi::OsString::from("qa-verdict.json")]
        );
        let record = read_qa_verdict_record(&session_storage.session_dir(&session_id))
            .unwrap()
            .unwrap();
        assert!(!record.contract_typed);
        assert!(record.criteria.is_empty());
        assert!(!storage.path().join("judgments").join("ledger.jsonl").exists());
    }

    #[tokio::test]
    async fn blocked_typed_verdict_records_named_omission_without_judgment_rows() {
        let storage = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let (app, controller, session_storage) =
            setup_test_app_with_controller_at(storage.path().to_path_buf()).await;
        let session_id = format!("blocked-qa-{}", uuid::Uuid::new_v4());
        install_typed_contract(project.path(), &session_id);
        let mut session = make_test_session(&session_id, project.path().to_str().unwrap());
        session.state = SessionState::QaInProgress { iteration: Some(3) };
        session.agents.push(evaluator_agent(&session_id));
        controller.write().insert_test_session(session);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{session_id}/qa/verdict"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"verdict":"BLOCKED","blocked_reason":"ui-unavailable","blocked_detail":"criterion 1 needs a host"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let record = read_qa_verdict_record(&session_storage.session_dir(&session_id))
            .unwrap()
            .unwrap();
        assert_eq!(record.iteration, 3);
        assert!(record.contract_typed);
        assert_eq!(
            record.omission.as_deref(),
            Some("blocked-verdict-no-criterion-rows")
        );
        assert!(record.criteria.is_empty());
        assert!(!storage.path().join("judgments").join("ledger.jsonl").exists());
    }

    #[test]
    fn evaluator_http_documentation_uses_one_typed_criteria_shape() {
        let source = include_str!("../../templates/mod.rs");
        let doc_shape = "Typed body: `{";
        assert_eq!(source.matches(doc_shape).count(), 3);
        let criterion_shape = r#""number":1,"result":"pass","evidence":"<observation>","evidence_refs":["<worker/report reference>"]"#;
        assert_eq!(source.matches(criterion_shape).count(), 6);
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
