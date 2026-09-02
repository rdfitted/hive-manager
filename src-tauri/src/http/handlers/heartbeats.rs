use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

use super::validate_agent_id;
use super::validate_session_id;
use crate::coordination::StateManager;
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::orchestrator::work_graph::completion_ledger::{
    NodeCompletionFact, NodeCompletionProvenance,
};
use crate::orchestrator::work_graph::BindingRef;

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
    pub completed_nodes: Vec<String>,
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
    let completed_nodes = if req.completed_nodes.is_empty() {
        Vec::new()
    } else {
        if req.status != "completed" {
            return Err(ApiError::bad_request(
                "completed_nodes may only be supplied with status completed",
            ));
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
        let unknown: BTreeSet<&str> = req
            .completed_nodes
            .iter()
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
                .filter(|node| req.completed_nodes.iter().any(|task_id| task_id == &node.id))
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
        req.completed_nodes
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
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
            .map_err(|e| ApiError::internal(e))?;
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
            .map(|task_id| {
                NodeCompletionFact::new(
                    task_id,
                    agent_id.clone(),
                    NodeCompletionProvenance::Heartbeat,
                )
            })
            .collect::<Vec<_>>();
        StateManager::new(state.storage.session_dir(&session_id))
            .append_node_completion_facts(&facts)
            .map_err(|error| {
                ApiError::internal(format!("Failed to persist completed_nodes: {error}"))
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

    Ok((
        StatusCode::OK,
        Json(PostHeartbeatResponse {
            message: "Heartbeat recorded".to_string(),
        }),
    ))
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
        post_heartbeat(
            State(state),
            Path(SESSION_ID.to_string()),
            Json(PostHeartbeatRequest {
                agent_id: agent_id.to_string(),
                status: status.to_string(),
                summary: Some(format!("{status} from handler test")),
                assignment_id,
                completed_nodes: completed_nodes
                    .iter()
                    .map(|task_id| (*task_id).to_string())
                    .collect(),
            }),
        )
        .await
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
}
