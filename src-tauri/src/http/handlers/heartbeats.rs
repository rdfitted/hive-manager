use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::validate_agent_id;
use super::validate_session_id;
use crate::coordination::StateManager;
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::orchestrator::org_graph::retrieval_ledger::record_ack_outcomes;
use crate::orchestrator::work_graph::completion_ledger::{
    NodeCompletionFact, NodeCompletionProvenance,
};
use crate::orchestrator::work_graph::knowledge_ack::{
    append_knowledge_ack, is_valid_knowledge_ack_tag,
};
use crate::orchestrator::work_graph::BindingRef;

use super::workers::ExecutedAs;

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum CompletedNodeEntry {
    Legacy(String),
    Detailed {
        node_id: String,
        executed_as: ExecutedAs,
    },
}

impl CompletedNodeEntry {
    fn into_parts(self) -> (String, Option<ExecutedAs>) {
        match self {
            Self::Legacy(node_id) => (node_id, None),
            Self::Detailed {
                node_id,
                executed_as,
            } => (node_id, Some(executed_as)),
        }
    }
}

/// POST /api/sessions/{id}/heartbeat - Body
#[derive(Debug, Deserialize)]
pub struct PostHeartbeatRequest {
    pub agent_id: String,
    pub status: String,
    #[serde(default)]
    pub summary: Option<String>,
    /// Durable queue assignment identity. Older prompts omit this and use the server-side
    /// deterministic fallback; a supplied identity is always treated as an exact fence.
    #[serde(default)]
    pub assignment_id: Option<i64>,
    /// Exact work-graph node ids completed by this heartbeat's resolved agent.
    #[serde(default)]
    pub(crate) completed_nodes: Vec<CompletedNodeEntry>,
    /// Explicit relevance acknowledgement for tagged knowledge references in this spawn.
    /// `None` is undecided; `Some([])` is a decided answer that none were relevant.
    #[serde(default)]
    pub knowledge_ack: Option<Vec<String>>,
}

/// Response for POST heartbeat
#[derive(Serialize)]
pub struct PostHeartbeatResponse {
    pub message: String,
}

/// Agent info with last_activity for active sessions
#[derive(Serialize)]
pub struct ActiveAgentInfo {
    pub id: String,
    pub role: String,
    pub last_activity: Option<String>,
    pub status: Option<String>,
    pub summary: Option<String>,
}

/// Session in active sessions list
#[derive(Serialize)]
pub struct ActiveSessionInfo {
    pub id: String,
    pub session_type: String,
    pub project_path: String,
    pub agents: Vec<ActiveAgentInfo>,
}

/// GET /api/sessions/active response
#[derive(Serialize)]
pub struct ActiveSessionsResponse {
    pub sessions: Vec<ActiveSessionInfo>,
}

const VALID_HEARTBEAT_STATUSES: &[&str] = &["working", "idle", "completed"];

/// POST /api/sessions/{id}/heartbeat
pub async fn post_heartbeat(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<PostHeartbeatRequest>,
) -> Result<(StatusCode, Json<PostHeartbeatResponse>), ApiError> {
    validate_session_id(&session_id)?;
    validate_agent_id(&req.agent_id)?;

    if !VALID_HEARTBEAT_STATUSES.contains(&req.status.as_str()) {
        return Err(ApiError::bad_request(
            "Status must be one of: working, idle, completed",
        ));
    }
    if req
        .assignment_id
        .is_some_and(|assignment_id| assignment_id <= 0)
    {
        return Err(ApiError::bad_request(
            "assignment_id must be a positive server-issued identity",
        ));
    }
    let knowledge_ack = match req.knowledge_ack.as_ref() {
        None => None,
        Some(tags) => {
            if req.status != "completed" {
                return Err(ApiError::bad_request(
                    "knowledge_ack may only be supplied with status completed",
                ));
            }
            if let Some(invalid) = tags.iter().find(|tag| !is_valid_knowledge_ack_tag(tag)) {
                return Err(ApiError::bad_request(format!(
                    "Invalid knowledge_ack tag {invalid:?}; expected ^k[1-9][0-9]*$"
                )));
            }
            Some(tags.clone())
        }
    };

    // #175(f): reject heartbeats for agents that are not part of this session.
    // A ghost id previously returned 200 and, worse, could finalize a durable
    // queue row via `record_heartbeat` — which is how a `completed` beat for a
    // nonexistent `worker-4` silently retired a real queue row.
    //
    // The gate is deliberately FAIL-OPEN across three signals (roster, live PTY,
    // queue row). Every spawn path starts the PTY *before* pushing the roster
    // entry, and several remove the old entry first, so a strict roster check
    // would 404 during those windows — and the heartbeat snippet agents run uses
    // `curl -fsS`, which turns a 404 into a hard non-zero exit that aborts the
    // agent's shell block. Breaking heartbeating is worse than the bug.
    //
    // It still closes the hole: the harmful case is an id with no roster entry,
    // no live PTY and no queue row, and this runs BEFORE `record_heartbeat`.
    // The Queen posts a bare `queen` alias while its roster entry is
    // `{session}-queen`, so RESOLVE the posted id rather than rewriting it: an id
    // that is already a member wins, and only an unknown bare alias is expanded.
    // Rewriting unconditionally would break every agent whose roster id genuinely
    // is unqualified.
    let qualified = super::canonical_agent_id(&session_id, &req.agent_id);

    let (raw_known, qualified_known) = {
        let controller = state.session_controller.read();
        let session = controller
            .get_session(&session_id)
            .ok_or_else(|| ApiError::not_found(format!("Session {} not found", session_id)))?;
        let pty_manager = state.pty_manager.read();
        let known =
            |id: &str| session.agents.iter().any(|a| a.id == id) || pty_manager.is_alive(id);
        (known(&req.agent_id), known(&qualified))
    };

    let agent_id = if raw_known {
        req.agent_id.clone()
    } else if qualified_known {
        qualified
    } else {
        // Last resort: a durable queue row is proof of a real worker whose roster
        // push has not landed yet.
        let rows = state
            .queue_manager
            .repo()
            .rows_for_session(&session_id)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if rows.iter().any(|row| row.worker_id == req.agent_id) {
            req.agent_id.clone()
        } else if rows.iter().any(|row| row.worker_id == qualified) {
            qualified
        } else {
            tracing::warn!(
                session_id = %session_id,
                agent_id = %req.agent_id,
                "rejecting heartbeat from an agent that is not a member of this session"
            );
            return Err(ApiError::not_found(format!(
                "Agent {} is not a member of session {}",
                req.agent_id, session_id
            )));
        }
    };

    // Validate the whole declared set before the queue or controller is mutated. Exact ids are
    // deliberate: aliases and labels are not node identity, and a mixed valid/invalid request
    // must apply none of its declarations.
    let (completed_nodes, external_completion_ids) = if req.completed_nodes.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        if req.status != "completed" {
            return Err(ApiError::bad_request(
                "completed_nodes may only be supplied with status completed",
            ));
        }
        let mut normalized = BTreeMap::<String, Option<ExecutedAs>>::new();
        for entry in req.completed_nodes.iter().cloned() {
            let (node_id, executed_as) = entry.into_parts();
            if let Some(existing) = normalized.get(&node_id) {
                if existing != &executed_as {
                    return Err(ApiError::bad_request(format!(
                        "Conflicting executed_as values for duplicate completed_nodes node_id: {node_id}"
                    )));
                }
            } else {
                normalized.insert(node_id, executed_as);
            }
        }

        let state_manager = StateManager::new(state.storage.session_dir(&session_id));
        let composition = state_manager
            .read_graph_composition_state()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read graph composition for completed_nodes validation: {error}"
                ))
            })?;
        let graph = if let Some(composition) = composition {
            Some(composition.graph)
        } else {
            state_manager.read_work_graph().map_err(|error| {
                ApiError::internal(format!(
                    "Failed to read work graph for completed_nodes validation: {error}"
                ))
            })?
        };
        let known_ids: BTreeSet<&str> = graph
            .as_ref()
            .into_iter()
            .flat_map(|graph| graph.nodes.iter().map(|node| node.id.as_str()))
            .collect();
        let unknown: BTreeSet<&str> = normalized
            .keys()
            .map(String::as_str)
            .filter(|task_id| !known_ids.contains(task_id))
            .collect();
        if !unknown.is_empty() {
            return Err(ApiError::bad_request(format!(
                "Unknown completed_nodes: {}",
                unknown.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }

        let hierarchy = state_manager.read_hierarchy().map_err(|error| {
            ApiError::internal(format!(
                "Failed to read hierarchy for completed_nodes validation: {error}"
            ))
        })?;
        if let Some(agent_principal) = hierarchy
            .iter()
            .find(|node| node.id == agent_id)
            .and_then(|node| node.principal.as_deref())
            .map(str::trim)
            .filter(|principal| !principal.is_empty())
        {
            let ownership_conflicts = graph
                .as_ref()
                .into_iter()
                .flat_map(|graph| graph.nodes.iter())
                .filter(|node| normalized.contains_key(&node.id))
                .filter_map(|node| {
                    let node_principal = match &node.binding {
                        BindingRef::Role(principal) | BindingRef::Zone(principal) => principal.trim(),
                    };
                    (!node_principal.is_empty() && node_principal != agent_principal).then(|| {
                        format!(
                            "node {} is bound to principal {}, but agent {} resolves to principal {}",
                            node.id, node_principal, agent_id, agent_principal
                        )
                    })
                })
                .collect::<Vec<_>>();
            if !ownership_conflicts.is_empty() {
                return Err(ApiError::bad_request(format!(
                    "completed_nodes ownership mismatch: {}",
                    ownership_conflicts.join("; ")
                )));
            }
        }
        // A missing principal remains valid for legacy completion facts, but cannot
        // authorize a queue prerequisite that has no row of its own.
        let external_principal = hierarchy
            .iter()
            .find(|node| node.id == agent_id)
            .and_then(|node| {
                node.principal
                    .as_deref()
                    .or_else(|| (node.role == "Queen").then_some("Queen"))
            })
            .map(str::trim)
            .filter(|principal| !principal.is_empty());
        let external_completion_ids = graph
            .as_ref()
            .into_iter()
            .flat_map(|graph| graph.nodes.iter())
            .filter(|node| normalized.contains_key(&node.id))
            .filter(|node| {
                let node_principal = match &node.binding {
                    BindingRef::Role(principal) | BindingRef::Zone(principal) => principal.trim(),
                };
                external_principal.is_some_and(|principal| principal == node_principal)
            })
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        (normalized.into_iter().collect::<Vec<_>>(), external_completion_ids)
    };

    // A supplied assignment identity is an exact durable fence. Check it before
    // mutating controller liveness so a stale worker cannot report a completion
    // that the queue rejected. Assignment-free callers remain fail-open below:
    // the Queen has no queue row, and legacy worker prompts omit this field.
    if let Some(assignment_id) = req.assignment_id {
        let recorded = state
            .queue_manager
            .record_heartbeat_for_assignment(
                &session_id,
                &agent_id,
                Some(assignment_id),
                &req.status,
            )
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if !recorded {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                format!(
                    "Assignment {} is stale or does not belong to agent {}",
                    assignment_id, agent_id
                ),
            ));
        }
    }

    // Scope the (non-Send) parking_lot guard so it is dropped before the no-ID await below.
    {
        let controller = state.session_controller.read();
        controller
            .update_heartbeat(&session_id, &agent_id, &req.status, req.summary.as_deref())
            .map_err(ApiError::internal)?;
    }

    // #126: without an explicit fence, retain the legacy deterministic fallback.
    // A caller with no matching queue row (notably the Queen) is intentionally a no-op.
    if req.assignment_id.is_none() {
        state
            .queue_manager
            .record_heartbeat_for_assignment(&session_id, &agent_id, None, &req.status)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }

    if !completed_nodes.is_empty() {
        let facts = completed_nodes
            .into_iter()
            .map(|(task_id, executed_as)| {
                let fact = NodeCompletionFact::new(
                    task_id,
                    agent_id.clone(),
                    NodeCompletionProvenance::Heartbeat,
                );
                match executed_as {
                    Some(executed_as) => fact.with_executed_as(executed_as),
                    None => fact,
                }
            })
            .collect::<Vec<_>>();
        StateManager::new(state.storage.session_dir(&session_id))
            .append_node_completion_facts(&facts)
            .map_err(|error| {
                ApiError::internal(format!("Failed to persist completed_nodes: {error}"))
            })?;
        state
            .queue_manager
            .record_external_completions(&session_id, &agent_id, &external_completion_ids)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to persist external completed_nodes: {error}"
                ))
            })?;
        for fact in facts {
            if let Err(error) = state.event_bus.publish(fact.event(&session_id)).await {
                tracing::warn!(
                    session_id = %session_id,
                    task_id = %fact.task_id,
                    "Failed to publish durable heartbeat completion event: {error}"
                );
            }
        }
    }

    if let Some(knowledge_ack) = knowledge_ack {
        match append_knowledge_ack(
            &state.storage.session_dir(&session_id),
            &session_id,
            &agent_id,
            &knowledge_ack,
        ) {
            Ok(()) => record_knowledge_ack_outcomes(&state, &session_id, &agent_id, &knowledge_ack),
            Err(error) => tracing::warn!(
                session_id = %session_id,
                agent_id = %agent_id,
                "Failed to persist knowledge_ack after heartbeat effects committed: {error}"
            ),
        }
    }

    Ok((
        StatusCode::OK,
        Json(PostHeartbeatResponse {
            message: "Heartbeat recorded".to_string(),
        }),
    ))
}

/// Retrieval outcomes are local observations and must never change the heartbeat response.
fn record_knowledge_ack_outcomes(
    state: &AppState,
    session_id: &str,
    agent_id: &str,
    knowledge_ack: &[String],
) {
    let project_path = {
        let controller = state.session_controller.read();
        controller
            .get_session(session_id)
            .map(|session| session.project_path.clone())
    };
    let Some(project_path) = project_path else {
        tracing::warn!(session_id, agent_id, "Skipping retrieval outcomes: session unavailable");
        return;
    };
    let sidecar_path = project_path
        .join(".hive-manager")
        .join(session_id)
        .join("prompts")
        .join(format!("{agent_id}-context.json"));
    let sidecar = match std::fs::read(&sidecar_path).and_then(|bytes| {
        serde_json::from_slice::<serde_json::Value>(&bytes).map_err(std::io::Error::other)
    }) {
        Ok(sidecar) => sidecar,
        Err(error) => {
            tracing::warn!(session_id, agent_id, path = %sidecar_path.display(),
                "Skipping retrieval outcomes: failed to read spawn context sidecar: {error}");
            return;
        }
    };
    if sidecar.get("session_id").and_then(serde_json::Value::as_str) != Some(session_id)
        || sidecar.get("agent_id").and_then(serde_json::Value::as_str) != Some(agent_id)
    {
        tracing::warn!(session_id, agent_id, path = %sidecar_path.display(),
            "Skipping retrieval outcomes: spawn context identity does not match heartbeat");
        return;
    }
    let ledger_path = state.storage.base_dir().join("judgments").join("ledger.jsonl");
    record_ack_outcomes(&ledger_path, &sidecar, knowledge_ack);
}

/// GET /api/sessions/active - Returns active sessions and agent heartbeats
pub async fn get_active_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ActiveSessionsResponse>, ApiError> {
    let controller = state.session_controller.read();
    let all_sessions = controller.list_sessions();

    let sessions: Vec<ActiveSessionInfo> = all_sessions
        .into_iter()
        .filter(|s| s.state.is_monitorable())
        .map(|session| {
            let agents_with_heartbeats = controller.get_heartbeat_info(&session.id);
            let agents: Vec<ActiveAgentInfo> = session
                .agents
                .iter()
                .map(|a| {
                    let hb = agents_with_heartbeats.get(&a.id);
                    ActiveAgentInfo {
                        id: a.id.clone(),
                        role: format!("{:?}", a.role),
                        last_activity: hb.map(|h| h.last_activity.to_rfc3339()),
                        status: hb.map(|h| h.status.clone()),
                        summary: hb.and_then(|h| h.summary.clone()),
                    }
                })
                .collect();

            ActiveSessionInfo {
                id: session.id.clone(),
                session_type: match &session.session_type {
                    crate::session::SessionType::Hive { worker_count } => {
                        format!("Hive ({})", worker_count)
                    }
                    crate::session::SessionType::Swarm { planner_count } => {
                        format!("Swarm ({})", planner_count)
                    }
                    crate::session::SessionType::Fusion { .. } => "Fusion".to_string(),
                    crate::session::SessionType::Debate { .. } => "Debate".to_string(),
                    crate::session::SessionType::Solo { cli, .. } => format!("Solo ({})", cli),
                },
                project_path: session.project_path.to_string_lossy().to_string(),
                agents,
            }
        })
        .collect();

    Ok(Json(ActiveSessionsResponse { sessions }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use parking_lot::RwLock;
    use tempfile::TempDir;

    use crate::coordination::{HierarchyNode, InjectionManager, QueueManager};
    use crate::domain::HiveExecutionPolicy;
    use crate::events::EventBus;
    use crate::orchestrator::work_graph::knowledge_ack::{
        knowledge_ack_path, KnowledgeAckRecord, KNOWLEDGE_ACK_SCHEMA_VERSION,
    };
    use crate::orchestrator::work_graph::{
        NodeContract, NodeKind, NodeStatus, TaskGraph, WorkNode,
    };
    use crate::pty::{AgentConfig, AgentRole, AgentStatus, PtyManager};
    use crate::session::{
        AgentInfo, AuthStrategy, Session, SessionController, SessionState, SessionType,
    };
    use crate::storage::queue::{QueueRow, QueueStatus};
    use crate::storage::{ApplicationStateDb, QueueRepo, SessionStorage};

    const SESSION_ID: &str = "heartbeat-assignment-fence";
    const WORKER_ID: &str = "heartbeat-assignment-fence-worker-1";
    const QUEEN_ID: &str = "heartbeat-assignment-fence-queen";
    const ROW_ID: &str = "heartbeat-assignment-row";
    const ASSIGNMENT_ID: i64 = 41;

    struct HeartbeatFixture {
        _temp: TempDir,
        state: Arc<AppState>,
    }

    fn agent(id: &str, role: AgentRole, parent_id: Option<&str>) -> AgentInfo {
        AgentInfo {
            id: id.to_string(),
            role,
            status: AgentStatus::Running,
            config: AgentConfig::default(),
            parent_id: parent_id.map(str::to_string),
            role_definition_id: None,
            role_definition_version: None,
            commit_sha: None,
            base_commit_sha: None,
        }
    }

    fn fixture() -> HeartbeatFixture {
        let temp = TempDir::new().expect("heartbeat fixture directory");
        let storage = Arc::new(
            SessionStorage::new_with_base(temp.path().to_path_buf()).expect("session storage"),
        );
        storage
            .create_session_dir(SESSION_ID)
            .expect("session directory");
        let config = Arc::new(tokio::sync::RwLock::new(
            storage.load_config().expect("test config"),
        ));
        let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
        let session_controller = Arc::new(RwLock::new(SessionController::new(Arc::clone(
            &pty_manager,
        ))));
        session_controller.write().set_storage(Arc::clone(&storage));
        let injection_manager = Arc::new(RwLock::new(InjectionManager::new(
            Arc::clone(&pty_manager),
            SessionStorage::new_with_base(temp.path().to_path_buf()).expect("injection storage"),
        )));
        let event_bus = EventBus::new(storage.base_dir().clone());
        let app_state_db =
            Arc::new(ApplicationStateDb::open_in_memory().expect("application state database"));
        let queue_repo = Arc::new(QueueRepo::new(Arc::clone(&app_state_db)));
        queue_repo.ensure_schema().expect("queue schema");
        let queue_manager = Arc::new(QueueManager::new(queue_repo, Arc::clone(&event_bus)));
        let state = Arc::new(AppState::new(
            config,
            pty_manager,
            Arc::clone(&session_controller),
            injection_manager,
            Arc::clone(&storage),
            event_bus,
            app_state_db,
            queue_manager,
            None,
        ));

        let now = Utc::now();
        session_controller.read().insert_test_session(Session {
            id: SESSION_ID.to_string(),
            name: None,
            color: None,
            session_type: SessionType::Hive { worker_count: 1 },
            project_path: temp.path().to_path_buf(),
            state: SessionState::Running,
            created_at: now,
            last_activity_at: now,
            agents: vec![
                agent(QUEEN_ID, AgentRole::Queen, None),
                agent(
                    WORKER_ID,
                    AgentRole::Worker {
                        index: 1,
                        parent: Some(QUEEN_ID.to_string()),
                    },
                    Some(QUEEN_ID),
                ),
            ],
            default_cli: "codex".to_string(),
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
            worktree_path: None,
            worktree_branch: None,
            no_git: true,
            resume_report: None,
        });

        HeartbeatFixture { _temp: temp, state }
    }

    fn running_row() -> QueueRow {
        QueueRow {
            id: ROW_ID.to_string(),
            task_id: Some("T-review-3".to_string()),
            session_id: SESSION_ID.to_string(),
            worker_id: WORKER_ID.to_string(),
            role_type: "backend".to_string(),
            cli: "codex".to_string(),
            status: QueueStatus::Running,
            payload: serde_json::json!({}),
            attempts: 1,
            continuation_count: 0,
            no_progress_count: 0,
            last_status: None,
            heartbeat_at: None,
            assignment_id: ASSIGNMENT_ID,
            blocked_reason: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    async fn heartbeat(
        state: Arc<AppState>,
        agent_id: &str,
        status: &str,
        assignment_id: Option<i64>,
    ) -> Result<(StatusCode, Json<PostHeartbeatResponse>), ApiError> {
        heartbeat_with_completed_nodes(state, agent_id, status, assignment_id, &[]).await
    }

    async fn heartbeat_with_completed_nodes(
        state: Arc<AppState>,
        agent_id: &str,
        status: &str,
        assignment_id: Option<i64>,
        completed_nodes: &[&str],
    ) -> Result<(StatusCode, Json<PostHeartbeatResponse>), ApiError> {
        heartbeat_with_completed_node_entries(
            state,
            agent_id,
            status,
            assignment_id,
            completed_nodes
                .iter()
                .map(|task_id| CompletedNodeEntry::Legacy((*task_id).to_string()))
                .collect(),
        )
        .await
    }

    async fn heartbeat_with_completed_node_entries(
        state: Arc<AppState>,
        agent_id: &str,
        status: &str,
        assignment_id: Option<i64>,
        completed_nodes: Vec<CompletedNodeEntry>,
    ) -> Result<(StatusCode, Json<PostHeartbeatResponse>), ApiError> {
        post_heartbeat(
            State(state),
            Path(SESSION_ID.to_string()),
            Json(PostHeartbeatRequest {
                agent_id: agent_id.to_string(),
                status: status.to_string(),
                summary: Some(format!("{status} from handler test")),
                assignment_id,
                completed_nodes,
                knowledge_ack: None,
            }),
        )
        .await
    }

    async fn heartbeat_with_knowledge_ack(
        state: Arc<AppState>,
        agent_id: &str,
        status: &str,
        knowledge_ack: Option<Vec<String>>,
    ) -> Result<(StatusCode, Json<PostHeartbeatResponse>), ApiError> {
        post_heartbeat(
            State(state),
            Path(SESSION_ID.to_string()),
            Json(PostHeartbeatRequest {
                agent_id: agent_id.to_string(),
                status: status.to_string(),
                summary: Some(format!("{status} knowledge acknowledgement")),
                assignment_id: None,
                completed_nodes: Vec::new(),
                knowledge_ack,
            }),
        )
        .await
    }

    fn stored_knowledge_acks(fixture: &HeartbeatFixture) -> Vec<KnowledgeAckRecord> {
        let path = knowledge_ack_path(&fixture.state.storage.session_dir(SESSION_ID));
        std::fs::read_to_string(path)
            .expect("knowledge acknowledgement store")
            .lines()
            .map(|line| serde_json::from_str(line).expect("knowledge acknowledgement row"))
            .collect()
    }

    fn retrieval_ledger_path(fixture: &HeartbeatFixture) -> std::path::PathBuf {
        fixture
            .state
            .storage
            .base_dir()
            .join("judgments")
            .join("ledger.jsonl")
    }

    fn write_retrieval_sidecar(fixture: &HeartbeatFixture, sampled: bool) {
        let sidecar_path = fixture
            ._temp
            .path()
            .join(".hive-manager")
            .join(SESSION_ID)
            .join("prompts")
            .join(format!("{WORKER_ID}-context.json"));
        std::fs::create_dir_all(sidecar_path.parent().unwrap()).unwrap();
        let sidecar = serde_json::json!({
            "schema_version": "hive.spawn-context/v1",
            "session_id": SESSION_ID,
            "agent_id": WORKER_ID,
            "plan_task_id": "T-knowledge",
            "sampled": sampled,
            "kept": [
                {"tag": "k1", "pointer": "project.md"},
                {"tag": "k2", "pointer": "wiki.md"}
            ],
            "dropped": [{"pointer": "other.md"}],
            "decision_ids": ["decision-kept-1", "decision-kept-2", "decision-dropped"]
        });
        std::fs::write(sidecar_path, serde_json::to_vec(&sidecar).unwrap()).unwrap();
    }

    fn stored_retrieval_outcomes(fixture: &HeartbeatFixture) -> Vec<serde_json::Value> {
        let path = retrieval_ledger_path(fixture);
        if !path.exists() {
            return Vec::new();
        }
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|record| record["kind"] == "outcome")
            .collect()
    }

    fn work_node(id: &str, principal: &str) -> WorkNode {
        WorkNode::new(
            id,
            NodeKind::Task,
            format!("Task {id}"),
            NodeContract::default(),
            BindingRef::Role(principal.to_string()),
            NodeStatus::Pending,
        )
    }

    fn write_graph_and_worker_principal(
        fixture: &HeartbeatFixture,
        graph: TaskGraph,
        principal: Option<&str>,
    ) -> StateManager {
        let state_manager = StateManager::new(fixture.state.storage.session_dir(SESSION_ID));
        state_manager
            .write_work_graph(&graph)
            .expect("heartbeat test work graph");
        state_manager
            .update_hierarchy(&[HierarchyNode {
                id: WORKER_ID.to_string(),
                role: "Worker-1".to_string(),
                principal: principal.map(str::to_string),
                parent_id: Some(QUEEN_ID.to_string()),
                children: Vec::new(),
            }])
            .expect("heartbeat test hierarchy");
        state_manager
    }

    fn expect_success(
        response: Result<(StatusCode, Json<PostHeartbeatResponse>), ApiError>,
        context: &str,
    ) -> (StatusCode, Json<PostHeartbeatResponse>) {
        match response {
            Ok(response) => response,
            Err(error) => panic!("{context}: {} ({})", error.message, error.status),
        }
    }

    #[tokio::test]
    async fn knowledge_ack_requires_completed_without_persisting_or_mutating_heartbeat() {
        let fixture = fixture();
        let path = knowledge_ack_path(&fixture.state.storage.session_dir(SESSION_ID));

        let result = heartbeat_with_knowledge_ack(
            Arc::clone(&fixture.state),
            WORKER_ID,
            "working",
            Some(vec!["k1".to_string()]),
        )
        .await;
        let error = match result {
            Err(error) => error,
            Ok((status, _)) => panic!("non-completed knowledge_ack must fail: got {status}"),
        };

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(!path.exists());
        assert!(fixture
            .state
            .session_controller
            .read()
            .get_heartbeat_info(SESSION_ID)
            .is_empty());
    }

    #[tokio::test]
    async fn invalid_knowledge_ack_tag_is_rejected_before_persistence() {
        let fixture = fixture();
        let path = knowledge_ack_path(&fixture.state.storage.session_dir(SESSION_ID));

        let result = heartbeat_with_knowledge_ack(
            Arc::clone(&fixture.state),
            WORKER_ID,
            "completed",
            Some(vec!["k0".to_string()]),
        )
        .await;
        let error = match result {
            Err(error) => error,
            Ok((status, _)) => panic!("invalid knowledge_ack tag must fail: got {status}"),
        };

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("k0"));
        assert!(!path.exists());
        assert!(fixture
            .state
            .session_controller
            .read()
            .get_heartbeat_info(SESSION_ID)
            .is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn absent_knowledge_ack_is_distinct_from_an_explicit_empty_ack_on_disk() {
        let _environment_lock = crate::orchestrator::org_graph::retrieval_ledger::RETRIEVAL_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fixture = fixture();
        write_retrieval_sidecar(&fixture, true);
        let session_dir = fixture.state.storage.session_dir(SESSION_ID);
        let path = knowledge_ack_path(&session_dir);

        let (status, _) = expect_success(
            heartbeat(Arc::clone(&fixture.state), WORKER_ID, "completed", None).await,
            "completed heartbeat without knowledge_ack",
        );
        assert_eq!(status, StatusCode::OK);
        assert!(!path.exists(), "absent knowledge_ack must write no row");
        assert!(stored_retrieval_outcomes(&fixture).is_empty());

        let (status, _) = expect_success(
            heartbeat_with_knowledge_ack(
                Arc::clone(&fixture.state),
                WORKER_ID,
                "completed",
                Some(Vec::new()),
            )
            .await,
            "completed heartbeat with explicit empty knowledge_ack",
        );
        assert_eq!(status, StatusCode::OK);
        let records = stored_knowledge_acks(&fixture);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].schema_version, KNOWLEDGE_ACK_SCHEMA_VERSION);
        assert_eq!(records[0].session_id, SESSION_ID);
        assert_eq!(records[0].agent_id, WORKER_ID);
        assert!(records[0].knowledge_ack.is_empty());
        let outcomes = stored_retrieval_outcomes(&fixture);
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes
            .iter()
            .all(|row| row["label"] == serde_json::json!({ "result": "unused" })));

        assert!(path.starts_with(fixture.state.storage.base_dir()));
        assert!(
            SessionStorage::new().is_err(),
            "tests must not resolve the production app-data storage base"
        );
    }

    #[tokio::test]
    async fn bare_alias_knowledge_ack_is_persisted_under_the_canonical_agent_id() {
        let fixture = fixture();

        let (status, _) = expect_success(
            heartbeat_with_knowledge_ack(
                Arc::clone(&fixture.state),
                "worker-1",
                "completed",
                Some(vec!["k1".to_string(), "k12".to_string()]),
            )
            .await,
            "completed heartbeat with bare worker alias",
        );
        assert_eq!(status, StatusCode::OK);

        let records = stored_knowledge_acks(&fixture);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].agent_id, WORKER_ID);
        assert_eq!(records[0].knowledge_ack, ["k1", "k12"]);
    }

    #[tokio::test]
    async fn knowledge_ack_append_failure_is_fail_open_after_heartbeat_commit() {
        let fixture = fixture();
        write_retrieval_sidecar(&fixture, true);
        let path = knowledge_ack_path(&fixture.state.storage.session_dir(SESSION_ID));
        std::fs::create_dir_all(&path).expect("directory occupying ack ledger path");

        let response = expect_success(
            heartbeat_with_knowledge_ack(
                Arc::clone(&fixture.state),
                WORKER_ID,
                "completed",
                Some(vec!["k1".to_string()]),
            )
            .await,
            "ack persistence failure after heartbeat commit",
        );

        assert_eq!(response.0, StatusCode::OK);
        let heartbeats = fixture
            .state
            .session_controller
            .read()
            .get_heartbeat_info(SESSION_ID);
        assert_eq!(heartbeats[WORKER_ID].status, "completed");
        assert!(path.is_dir(), "failed append must not replace the obstacle");
        assert!(stored_retrieval_outcomes(&fixture).is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sampled_knowledge_ack_joins_kept_decisions_once() {
        let _environment_lock = crate::orchestrator::org_graph::retrieval_ledger::RETRIEVAL_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fixture = fixture();
        write_retrieval_sidecar(&fixture, true);

        let response = expect_success(
            heartbeat_with_knowledge_ack(
                Arc::clone(&fixture.state),
                WORKER_ID,
                "completed",
                Some(vec!["k1".into()]),
            )
            .await,
            "sampled completed knowledge acknowledgement",
        );
        assert_eq!(response.0, StatusCode::OK);
        let outcomes = stored_retrieval_outcomes(&fixture);
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0]["decision_id"], "decision-kept-1");
        assert_eq!(outcomes[0]["label"], serde_json::json!({ "result": "used" }));
        assert_eq!(outcomes[0]["source"], "model-ack");
        assert_eq!(outcomes[1]["decision_id"], "decision-kept-2");
        assert_eq!(
            outcomes[1]["label"],
            serde_json::json!({ "result": "unused" })
        );
        assert!(outcomes
            .iter()
            .all(|row| row["decision_id"] != "decision-dropped"));

        let response = expect_success(
            heartbeat_with_knowledge_ack(
                Arc::clone(&fixture.state),
                WORKER_ID,
                "completed",
                Some(vec!["k1".into()]),
            )
            .await,
            "repeated sampled acknowledgement",
        );
        assert_eq!(response.0, StatusCode::OK);
        assert_eq!(stored_knowledge_acks(&fixture).len(), 2);
        assert_eq!(stored_retrieval_outcomes(&fixture).len(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unsampled_and_out_of_context_acks_write_no_retrieval_outcomes() {
        let _environment_lock = crate::orchestrator::org_graph::retrieval_ledger::RETRIEVAL_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let unsampled = fixture();
        write_retrieval_sidecar(&unsampled, false);
        let response = expect_success(
            heartbeat_with_knowledge_ack(
                Arc::clone(&unsampled.state),
                WORKER_ID,
                "completed",
                Some(vec!["k1".into()]),
            )
            .await,
            "unsampled acknowledgement",
        );
        assert_eq!(response.0, StatusCode::OK);
        assert!(stored_retrieval_outcomes(&unsampled).is_empty());

        let out_of_context = fixture();
        write_retrieval_sidecar(&out_of_context, true);
        let response = expect_success(
            heartbeat_with_knowledge_ack(
                Arc::clone(&out_of_context.state),
                WORKER_ID,
                "completed",
                Some(vec!["k1".into(), "k3".into()]),
            )
            .await,
            "out-of-context acknowledgement",
        );
        assert_eq!(response.0, StatusCode::OK);
        assert!(stored_retrieval_outcomes(&out_of_context).is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retrieval_ledger_failure_preserves_completed_heartbeat_response() {
        let _environment_lock = crate::orchestrator::org_graph::retrieval_ledger::RETRIEVAL_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fixture = fixture();
        write_retrieval_sidecar(&fixture, true);
        let ledger_path = retrieval_ledger_path(&fixture);
        std::fs::create_dir_all(&ledger_path).unwrap();

        let response = expect_success(
            heartbeat_with_knowledge_ack(
                Arc::clone(&fixture.state),
                WORKER_ID,
                "completed",
                Some(vec!["k1".into()]),
            )
            .await,
            "ledger failure after acknowledgement append",
        );
        assert_eq!(response.0, StatusCode::OK);
        assert_eq!(stored_knowledge_acks(&fixture).len(), 1);
        assert!(ledger_path.is_dir());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retrieval_kill_switch_preserves_ack_without_outcomes() {
        let _guard = crate::orchestrator::org_graph::retrieval_ledger::RETRIEVAL_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os("HIVE_RETRIEVAL_LEDGER");
        std::env::set_var("HIVE_RETRIEVAL_LEDGER", "off");
        let fixture = fixture();
        write_retrieval_sidecar(&fixture, true);
        let response = heartbeat_with_knowledge_ack(
            Arc::clone(&fixture.state),
            WORKER_ID,
            "completed",
            Some(vec!["k1".into()]),
        )
        .await;
        match previous {
            Some(value) => std::env::set_var("HIVE_RETRIEVAL_LEDGER", value),
            None => std::env::remove_var("HIVE_RETRIEVAL_LEDGER"),
        }

        assert_eq!(
            expect_success(response, "kill-switch acknowledgement").0,
            StatusCode::OK
        );
        assert_eq!(stored_knowledge_acks(&fixture).len(), 1);
        assert!(stored_retrieval_outcomes(&fixture).is_empty());
    }

    #[tokio::test]
    async fn explicit_assignment_mismatch_is_conflict_without_mutating_queue_or_controller() {
        let fixture = fixture();
        let original_row = running_row();
        fixture
            .state
            .queue_manager
            .repo()
            .enqueue(&original_row)
            .expect("running queue row");

        let response = heartbeat(
            Arc::clone(&fixture.state),
            WORKER_ID,
            "completed",
            Some(ASSIGNMENT_ID + 1),
        )
        .await;
        let status = match response {
            Ok((status, _)) => status,
            Err(error) => error.status,
        };

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            fixture
                .state
                .queue_manager
                .repo()
                .get_row(ROW_ID)
                .expect("queue lookup")
                .expect("queue row"),
            original_row,
            "a stale explicit assignment must not mutate the durable row",
        );
        assert!(
            fixture
                .state
                .session_controller
                .read()
                .get_heartbeat_info(SESSION_ID)
                .is_empty(),
            "a stale explicit assignment must not update controller liveness",
        );
    }

    #[tokio::test]
    async fn explicit_assignment_match_records_durable_and_controller_heartbeat() {
        let fixture = fixture();
        fixture
            .state
            .queue_manager
            .repo()
            .enqueue(&running_row())
            .expect("running queue row");

        let response = expect_success(
            heartbeat(
                Arc::clone(&fixture.state),
                WORKER_ID,
                "completed",
                Some(ASSIGNMENT_ID),
            )
            .await,
            "matching assignment heartbeat",
        );

        assert_eq!(response.0, StatusCode::OK);
        let updated_row = fixture
            .state
            .queue_manager
            .repo()
            .get_row(ROW_ID)
            .expect("queue lookup")
            .expect("queue row");
        assert_eq!(updated_row.status, QueueStatus::Finalized);
        assert_eq!(updated_row.last_status.as_deref(), Some("completed"));
        assert!(updated_row.heartbeat_at.is_some());
        let heartbeats = fixture
            .state
            .session_controller
            .read()
            .get_heartbeat_info(SESSION_ID);
        assert_eq!(heartbeats[WORKER_ID].status, "completed");
        assert_eq!(
            heartbeats[WORKER_ID].summary.as_deref(),
            Some("completed from handler test"),
        );
    }

    #[tokio::test]
    async fn queen_heartbeat_without_assignment_remains_fail_open() {
        let fixture = fixture();

        let response = expect_success(
            heartbeat(Arc::clone(&fixture.state), "queen", "working", None).await,
            "assignment-free Queen heartbeat",
        );

        assert_eq!(response.0, StatusCode::OK);
        assert!(fixture
            .state
            .queue_manager
            .repo()
            .rows_for_session(SESSION_ID)
            .expect("queue lookup")
            .is_empty());
        let heartbeats = fixture
            .state
            .session_controller
            .read()
            .get_heartbeat_info(SESSION_ID);
        assert!(!heartbeats.contains_key("queen"));
        assert_eq!(heartbeats[QUEEN_ID].status, "working");
    }

    #[tokio::test]
    async fn contradictory_node_principal_rejects_the_entire_completion_set() {
        let fixture = fixture();
        let state_manager = write_graph_and_worker_principal(
            &fixture,
            TaskGraph::new(
                vec![work_node("T-owned", "P1"), work_node("T-other", "P2")],
                Vec::new(),
            ),
            Some("P1"),
        );

        let response = heartbeat_with_completed_nodes(
            Arc::clone(&fixture.state),
            "worker-1",
            "completed",
            None,
            &["T-owned", "T-other"],
        )
        .await;
        let error = match response {
            Ok((status, _)) => panic!(
                "a resolved principal must not complete another principal's node: got {status}"
            ),
            Err(error) => error,
        };

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("node T-other"));
        assert!(error.message.contains("principal P2"));
        assert!(error.message.contains("principal P1"));
        assert!(
            state_manager
                .read_node_completion_facts()
                .expect("completion ledger")
                .is_empty(),
            "one ownership conflict must prevent every fact in the request"
        );
        assert!(
            fixture
                .state
                .session_controller
                .read()
                .get_heartbeat_info(SESSION_ID)
                .is_empty(),
            "ownership validation must run before controller liveness is mutated"
        );
    }

    #[tokio::test]
    async fn missing_agent_principal_allows_a_bound_node_completion() {
        let fixture = fixture();
        let state_manager = write_graph_and_worker_principal(
            &fixture,
            TaskGraph::new(vec![work_node("T-bound", "P2")], Vec::new()),
            None,
        );

        // Most spawn paths intentionally persist no principal. This is the normal fail-open case,
        // not evidence that the agent contradicts the node's recorded binding.
        let response = expect_success(
            heartbeat_with_completed_nodes(
                Arc::clone(&fixture.state),
                "worker-1",
                "completed",
                None,
                &["T-bound"],
            )
            .await,
            "completion from an agent with no recorded principal",
        );

        assert_eq!(response.0, StatusCode::OK);
        let facts = state_manager
            .read_node_completion_facts()
            .expect("completion ledger");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].task_id, "T-bound");
        assert_eq!(
            facts[0].agent_id, WORKER_ID,
            "the fact must retain the resolved full agent id"
        );
    }

    #[tokio::test]
    async fn missing_node_binding_allows_a_resolved_principal_completion() {
        let fixture = fixture();
        let state_manager = write_graph_and_worker_principal(
            &fixture,
            TaskGraph::new(vec![work_node("T-unbound", "  ")], Vec::new()),
            Some("P1"),
        );

        let response = expect_success(
            heartbeat_with_completed_nodes(
                Arc::clone(&fixture.state),
                "worker-1",
                "completed",
                None,
                &["T-unbound"],
            )
            .await,
            "completion for a node with no recorded binding",
        );

        assert_eq!(response.0, StatusCode::OK);
        let facts = state_manager
            .read_node_completion_facts()
            .expect("completion ledger");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].task_id, "T-unbound");
        assert_eq!(facts[0].agent_id, WORKER_ID);
    }

    #[tokio::test]
    async fn detailed_completed_node_records_declared_native_execution() {
        let fixture = fixture();
        let state_manager = write_graph_and_worker_principal(
            &fixture,
            TaskGraph::new(vec![work_node("T-native", "P1")], Vec::new()),
            Some("P1"),
        );
        let executed_as_json = serde_json::json!({
            "provider": "codex",
            "tier": "high",
            "model": "gpt-5.6-sol",
            "flags": ["-c", "model_reasoning_effort=\"high\""],
            "channel": "native",
            "source": "node"
        });
        let executed_as: ExecutedAs =
            serde_json::from_value(executed_as_json.clone()).expect("valid native execution");

        let response = expect_success(
            heartbeat_with_completed_node_entries(
                Arc::clone(&fixture.state),
                "worker-1",
                "completed",
                None,
                vec![CompletedNodeEntry::Detailed {
                    node_id: "T-native".to_string(),
                    executed_as,
                }],
            )
            .await,
            "detailed native completion",
        );

        assert_eq!(response.0, StatusCode::OK);
        let facts = state_manager
            .read_node_completion_facts()
            .expect("completion ledger");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].task_id, "T-native");
        assert_eq!(facts[0].agent_id, WORKER_ID);
        assert_eq!(
            serde_json::to_value(facts[0].executed_as.as_ref().unwrap()).unwrap(),
            executed_as_json
        );
    }

    #[tokio::test]
    async fn conflicting_duplicate_completed_node_execution_is_rejected_before_mutation() {
        let fixture = fixture();
        let state_manager = write_graph_and_worker_principal(
            &fixture,
            TaskGraph::new(vec![work_node("T-native", "P1")], Vec::new()),
            Some("P1"),
        );
        let execution = |model: &str| {
            serde_json::from_value::<ExecutedAs>(serde_json::json!({
                "provider": "codex",
                "tier": "high",
                "model": model,
                "flags": ["-c", "model_reasoning_effort=\"high\""],
                "channel": "native",
                "source": "node"
            }))
            .unwrap()
        };

        let result = heartbeat_with_completed_node_entries(
            Arc::clone(&fixture.state),
            "worker-1",
            "completed",
            None,
            vec![
                CompletedNodeEntry::Detailed {
                    node_id: "T-native".to_string(),
                    executed_as: execution("gpt-5.6-sol"),
                },
                CompletedNodeEntry::Detailed {
                    node_id: "T-native".to_string(),
                    executed_as: execution("gpt-5.6-terra"),
                },
            ],
        )
        .await;
        let error = match result {
            Err(error) => error,
            Ok((status, _)) => {
                panic!("conflicting duplicate execution tuples must fail closed: got {status}")
            }
        };

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("T-native"));
        assert!(state_manager
            .read_node_completion_facts()
            .expect("completion ledger")
            .is_empty());
        assert!(fixture
            .state
            .session_controller
            .read()
            .get_heartbeat_info(SESSION_ID)
            .is_empty());
    }
}
