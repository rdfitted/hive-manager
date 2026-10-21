use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Weak};

use super::validate_agent_id;
use super::validate_session_id;
use crate::coordination::StateManager;
use crate::domain::event::{Event, EventType, Severity};
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::orchestrator::org_graph::retrieval_ledger::record_ack_outcomes;
use crate::orchestrator::work_graph::completion_ledger::{
    NodeCompletionFact, NodeCompletionProvenance,
};
use crate::orchestrator::work_graph::knowledge_ack::{
    append_knowledge_ack, is_valid_knowledge_ack_tag,
};
use crate::orchestrator::work_graph::{BindingRef, NodeStatus};

use super::workers::ExecutedAs;

#[derive(Debug, Clone, Deserialize, Serialize)]
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
#[derive(Debug, Deserialize, Serialize)]
pub struct PostHeartbeatRequest {
    pub agent_id: String,
    /// Explicit Queen-only cross-agent status mark; agent_id remains the authenticated Queen.
    #[serde(default)]
    pub mark_agent_id: Option<String>,
    /// Explicit Queen direction; never inferred from a worker's completion claim.
    #[serde(default)]
    pub reassign_nodes: Vec<NodeReassignment>,
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

#[derive(Debug, Deserialize, Serialize)]
pub struct NodeReassignment {
    pub node_id: String,
    pub agent_id: String,
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
// Keep binding authorization and durable completion effects in the same critical section.
static HEARTBEAT_MUTATIONS: LazyLock<parking_lot::Mutex<BTreeMap<String, Weak<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| parking_lot::Mutex::new(BTreeMap::new()));

fn heartbeat_mutation_lock(session_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = HEARTBEAT_MUTATIONS.lock();
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(session_id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(session_id.to_string(), Arc::downgrade(&lock));
    lock
}

/// Neutral audit only: unknown callers stay unknown and never infer a completion.
async fn heartbeat_security_event(
    state: &AppState, session_id: &str, caller_id: Option<&str>, claimed_id: &str,
    resolved_id: Option<&str>, token: Option<&str>, reason: &'static str, rejected: bool,
) -> Result<(), ApiError> {
    let safe = |value: &str| {
        if token.is_some_and(|token| !token.is_empty() && value.contains(token)) {
            "redacted-credential".to_string()
        } else { value.to_string() }
    };
    let known_session = state.session_controller.read().get_session(session_id).is_some();
    let safe_session_id = safe(session_id);
    let may_persist = known_session && safe_session_id == session_id;
    let session_id = if may_persist { safe_session_id } else { "unattributed-session".to_string() };
    let caller_id = caller_id.map(safe);
    let claimed_id = safe(claimed_id);
    let resolved_id = resolved_id.map(safe);
    tracing::warn!(session_id = %session_id, caller_agent_id = ?caller_id,
        claimed_agent_id = %claimed_id, resolved_agent_id = ?resolved_id, reason,
        rejected, "heartbeat authorization audit");
    // Unknown or credential-bearing session paths must never create a token-named directory.
    if !may_persist { return Ok(()); }
    state.event_bus.publish(Event {
        id: uuid::Uuid::new_v4().to_string(), session_id, cell_id: None,
        agent_id: caller_id.clone(), event_type: EventType::ArtifactUpdated,
        timestamp: chrono::Utc::now(), severity: Severity::Warning,
        payload: serde_json::json!({
            "kind": if rejected { "heartbeat_rejected" } else { "heartbeat_unauthenticated" },
            "caller_agent_id": caller_id, "claimed_agent_id": claimed_id,
            "resolved_agent_id": resolved_id, "reason": reason,
        }),
    }).await.map_err(|_| ApiError::internal("Failed to persist heartbeat authorization audit"))
}

async fn reject_heartbeat(
    state: &AppState, session_id: &str, caller_id: Option<&str>, claimed_id: &str,
    resolved_id: Option<&str>, token: Option<&str>, reason: &'static str,
) {
    if heartbeat_security_event(state, session_id, caller_id, claimed_id, resolved_id, token, reason, true).await.is_err() {
        tracing::warn!("Failed to persist rejected heartbeat audit; request remains rejected");
    }
}

/// POST /api/sessions/{id}/heartbeat
pub async fn post_heartbeat(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(mut req): Json<PostHeartbeatRequest>,
) -> Result<(StatusCode, Json<PostHeartbeatResponse>), ApiError> {
    validate_session_id(&session_id)?;
    validate_agent_id(&req.agent_id)?;

    if !VALID_HEARTBEAT_STATUSES.contains(&req.status.as_str()) {
        return Err(ApiError::bad_request(
            "Status must be one of: working, idle, completed",
        ));
    }

    let claimed_id = req.agent_id.clone();
    let token_headers = headers.get_all("X-Hive-Agent-Token");
    let token = headers.get("X-Hive-Agent-Token").and_then(|header| header.to_str().ok());
    let verify_when_present = state.pty_manager.read().heartbeat_verifies_when_present(&session_id);
    // Codex can forward an explicitly empty header after filtering its tool environment.
    // Only one decodable empty/whitespace value is absent; duplicates/non-UTF8 stay invalid.
    let usable_token_absent = !headers.contains_key("X-Hive-Agent-Token")
        || (token_headers.iter().count() == 1 && token.is_some_and(|token| token.trim().is_empty()));
    let authenticated = if !usable_token_absent {
        let authenticated = if token_headers.iter().count() == 1 {
            token.and_then(|token| state.pty_manager.read().authenticate_heartbeat(&session_id, token))
        } else { None };
        if authenticated.is_none() {
            let audit_claimed_id = if token_headers.iter().count() == 1 && token.is_some() {
                claimed_id.as_str()
            } else { "redacted-untrusted-identity" };
            reject_heartbeat(&state, &session_id, None, audit_claimed_id, None, token, "invalid_token").await;
            return Err(ApiError::new(StatusCode::UNAUTHORIZED,
                "Missing or invalid X-Hive-Agent-Token; restart this agent to receive its spawn credential"));
        }
        authenticated
    } else if !verify_when_present {
        reject_heartbeat(&state, &session_id, None, &claimed_id, None, None, "missing_token").await;
        return Err(ApiError::new(StatusCode::UNAUTHORIZED,
            "Missing or invalid X-Hive-Agent-Token; restart this agent to receive its spawn credential"));
    } else { None };
    let caller_id = authenticated.as_ref().map(|(id, _)| id.clone()).unwrap_or_else(|| claimed_id.clone());
    let caller_is_queen = authenticated.as_ref().is_some_and(|(_, queen)| *queen);
    // A confused agent must not persist its credential in a summary, provenance or target ID.
    let payload = serde_json::to_string(&req).map_err(|_| ApiError::bad_request("Invalid heartbeat payload"))?;
    if !usable_token_absent && token.is_some_and(|token| !token.is_empty() && payload.contains(token)) {
        reject_heartbeat(&state, &session_id, authenticated.as_ref().map(|(id, _)| id.as_str()),
            &claimed_id, req.mark_agent_id.as_deref(), token, "credential_in_payload").await;
        return Err(ApiError::bad_request("Heartbeat payload must not contain a credential value"));
    }
    if authenticated.is_some() && req.agent_id != caller_id
        && super::canonical_agent_id(&session_id, &req.agent_id) != caller_id
    {
        reject_heartbeat(&state, &session_id, Some(&caller_id), &claimed_id, None, token, "token_owner_mismatch").await;
        return Err(ApiError::new(StatusCode::FORBIDDEN,
            "X-Hive-Agent-Token may update only its owning agent; Queen cross-agent marks require mark_agent_id"));
    }
    if let Some(target_id) = req.mark_agent_id.as_ref() {
        if !caller_is_queen {
            reject_heartbeat(&state, &session_id, authenticated.as_ref().map(|(id, _)| id.as_str()),
                &claimed_id, Some(target_id), token, "queen_mark_requires_credential").await;
            return Err(ApiError::new(StatusCode::FORBIDDEN,
                "mark_agent_id requires a Queen-scoped X-Hive-Agent-Token"));
        }
        if req.knowledge_ack.is_some() {
            return Err(ApiError::bad_request("A Queen cross-agent mark cannot acknowledge another agent's knowledge"));
        }
        validate_agent_id(target_id)?;
        req.agent_id = target_id.clone();
    }
    let mutation_lock = heartbeat_mutation_lock(&session_id);
    let _mutation_guard = mutation_lock.lock().await;
    if !req.reassign_nodes.is_empty() {
        if !caller_is_queen {
            reject_heartbeat(&state, &session_id, authenticated.as_ref().map(|(id, _)| id.as_str()),
                &claimed_id, None, token, "reassignment_requires_credential").await;
            return Err(ApiError::new(StatusCode::FORBIDDEN, "reassign_nodes requires a Queen-scoped X-Hive-Agent-Token"));
        }
        if req.mark_agent_id.is_some() || !req.completed_nodes.is_empty()
            || req.assignment_id.is_some() || req.knowledge_ack.is_some() || req.status == "completed"
        {
            return Err(ApiError::bad_request("reassign_nodes cannot be combined with completion, marking, assignment_id or knowledge_ack"));
        }
        reassign_nodes(&state, &session_id, &caller_id, &req.reassign_nodes).await?;
        return Ok((StatusCode::OK, Json(PostHeartbeatResponse { message: "Node bindings reassigned".to_string() })));
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

    // Raw roster IDs take precedence over aliases. Check the resolved target too, so an
    // alias collision cannot turn an owning credential into a cross-agent write.
    if authenticated.is_some() && req.mark_agent_id.is_none() && agent_id != caller_id {
        reject_heartbeat(&state, &session_id, Some(&caller_id), &claimed_id, Some(&agent_id), token,
            "resolved_token_owner_mismatch").await;
        return Err(ApiError::new(StatusCode::FORBIDDEN,
            "X-Hive-Agent-Token does not own the resolved agent"));
    }

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
        let agent_principal = hierarchy
            .iter()
            .find(|node| node.id == agent_id)
            .and_then(|node| node.principal.as_deref())
            .map(str::trim)
            .filter(|principal| !principal.is_empty());
        if agent_principal.is_none() && !(caller_is_queen && agent_id == caller_id) {
            reject_heartbeat(&state, &session_id, authenticated.as_ref().map(|(id, _)| id.as_str()),
                &claimed_id, Some(&agent_id), token, "missing_principal_binding").await;
            return Err(ApiError::new(StatusCode::FORBIDDEN,
                "completed_nodes requires an authenticated agent with a current principal binding"));
        }
        if let Some(agent_principal) = agent_principal {
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
                reject_heartbeat(&state, &session_id, authenticated.as_ref().map(|(id, _)| id.as_str()),
                    &claimed_id, Some(&agent_id), token, "principal_binding_mismatch").await;
                return Err(ApiError::bad_request(format!(
                    "completed_nodes ownership mismatch: {}",
                    ownership_conflicts.join("; ")
                )));
            }
        }
        // An authenticated Queen may retain legacy attestation facts, but a row-less
        // queue prerequisite still requires actual ownership of the node below.
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

    if authenticated.is_none() {
        heartbeat_security_event(&state, &session_id, None, &claimed_id, Some(&agent_id), None,
            "verify_when_present", false).await?;
    }

    // Neutral audit events do not infer work-graph completion, unlike AgentCompleted.
    // Persist before changing status so a failed audit write cannot silently erase evidence.
    let previous = state.session_controller.read().get_heartbeat_info(&session_id)
        .get(&agent_id).cloned();
    if req.mark_agent_id.is_some() || req.status == "completed"
        || previous.as_ref().is_some_and(|heartbeat| heartbeat.status == "completed")
    {
        state.event_bus.publish(Event {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.clone(),
            cell_id: None,
            agent_id: authenticated.as_ref().map(|(id, _)| id.clone()),
            event_type: EventType::ArtifactUpdated,
            timestamp: chrono::Utc::now(),
            payload: serde_json::json!({
                "kind": "heartbeat_status_requested",
                "actor_agent_id": authenticated.as_ref().map(|(id, _)| id.as_str()),
                "target_agent_id": agent_id,
                "queen_mark": req.mark_agent_id.is_some(),
                "status": req.status,
                "summary": req.summary,
                "completed_nodes": req.completed_nodes,
                "previous_status": previous.as_ref().map(|heartbeat| &heartbeat.status),
                "previous_summary": previous.as_ref().and_then(|heartbeat| heartbeat.summary.as_ref()),
            }),
            severity: Severity::Info,
        }).await.map_err(ApiError::internal)?;
    }

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

/// Rebind incomplete, row-less work. Queue-backed transfers require their assignment lifecycle,
/// rather than silently retaining an old worker's durable fence.
async fn reassign_nodes(
    state: &AppState,
    session_id: &str,
    caller_id: &str,
    requests: &[NodeReassignment],
) -> Result<(), ApiError> {
    reassign_nodes_with_persistence(state, session_id, caller_id, requests,
        &ReassignmentPersistence::default()).await
}

/// Local persistence seam: production always uses the real atomic file writes.
#[derive(Default)]
struct ReassignmentPersistence {
    #[cfg(test)]
    fail_composition: bool,
    #[cfg(test)]
    fail_restore: bool,
    #[cfg(test)]
    fail_outcome_audit: bool,
    #[cfg(test)]
    outcome_attempts: std::sync::atomic::AtomicUsize,
}

impl ReassignmentPersistence {
    fn write_composition(&self, manager: &StateManager,
        composition: &crate::orchestrator::work_graph::runtime::GraphCompositionState,
    ) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_composition {
            assert!(manager.read_work_graph().unwrap().is_some(), "standalone write must precede injected failure");
            return Err("injected composition persistence failure".to_string());
        }
        manager.write_graph_composition_state(composition).map_err(|error| error.to_string())
    }

    fn restore_graph(&self, manager: &StateManager, path: &std::path::Path,
        original: Option<&crate::orchestrator::work_graph::TaskGraph>,
    ) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_restore {
            return Err("injected graph rollback failure".to_string());
        }
        if let Some(original) = original {
            manager.write_work_graph(original).map_err(|error| error.to_string())
        } else {
            // This operation created the standalone projection; absence is its prior state.
            match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }
    }

    async fn publish_outcome(&self, state: &AppState, event: Event) -> Result<(), String> {
        #[cfg(test)]
        {
            self.outcome_attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if self.fail_outcome_audit {
                return Err("injected reassignment outcome audit failure".to_string());
            }
        }
        state.event_bus.publish(event).await
    }
}

async fn audit_reassignment_outcome(
    state: &AppState,
    persistence: &ReassignmentPersistence,
    session_id: &str,
    caller_id: &str,
    changes: &[serde_json::Value],
    applied: bool,
    rolled_back: bool,
) {
    let kind = if applied { "node_reassignment_applied" } else { "node_reassignment_failed" };
    let event = Event {
        id: uuid::Uuid::new_v4().to_string(), session_id: session_id.to_string(), cell_id: None,
        agent_id: Some(caller_id.to_string()), event_type: EventType::ArtifactUpdated,
        timestamp: chrono::Utc::now(), severity: if applied { Severity::Info } else { Severity::Warning },
        payload: serde_json::json!({"kind": kind, "actor_agent_id": caller_id,
            "changes": changes, "rolled_back": rolled_back}),
    };
    // An outcome audit cannot turn a committed mutation into an ambiguous HTTP failure.
    if let Err(error) = persistence.publish_outcome(state, event).await {
        tracing::warn!(session_id, caller_id, kind, "Failed to audit reassignment outcome: {error}");
    }
}

async fn reassign_nodes_with_persistence(
    state: &AppState,
    session_id: &str,
    caller_id: &str,
    requests: &[NodeReassignment],
    persistence: &ReassignmentPersistence,
) -> Result<(), ApiError> {
    let manager = StateManager::new(state.storage.session_dir(session_id));
    let mut composition = manager.read_graph_composition_state()
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let original = manager.read_work_graph().map_err(|error| ApiError::internal(error.to_string()))?;
    let mut graph = composition.as_ref().map(|value| value.graph.clone()).or_else(|| original.clone())
        .ok_or_else(|| ApiError::not_found("No work graph to reassign"))?;
    let hierarchy = manager.read_hierarchy().map_err(|error| ApiError::internal(error.to_string()))?;
    let rows = state.queue_manager.repo().rows_for_session(session_id)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let facts = manager.read_node_completion_facts().map_err(|error| ApiError::internal(error.to_string()))?;
    let external = state.queue_manager.repo().external_completion_task_ids(session_id)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let roster = state.session_controller.read().get_session(session_id)
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    let mut seen = BTreeSet::new();
    let mut changes = Vec::new();
    // Validate the whole set before touching either graph representation or the audit log.
    for request in requests {
        validate_agent_id(&request.agent_id)?;
        if !seen.insert(&request.node_id) {
            return Err(ApiError::bad_request("Duplicate node in reassign_nodes"));
        }
        let node = graph.nodes.iter_mut().find(|node| node.id == request.node_id)
            .ok_or_else(|| ApiError::bad_request(format!("Unknown reassignment node {}", request.node_id)))?;
        if node.status == NodeStatus::Completed || facts.iter().any(|fact| fact.task_id == node.id)
            || external.contains(&node.id)
        {
            return Err(ApiError::new(StatusCode::CONFLICT, "Reassignment does not reopen completed work"));
        }
        if rows.iter().any(|row| row.task_id.as_deref() == Some(node.id.as_str())) {
            return Err(ApiError::new(StatusCode::CONFLICT,
                "Queue-backed reassignment must transfer its durable assignment before rebinding"));
        }
        let target_id = if roster.agents.iter().any(|agent| agent.id == request.agent_id) {
            request.agent_id.clone()
        } else {
            super::canonical_agent_id(session_id, &request.agent_id)
        };
        if !roster.agents.iter().any(|agent| agent.id == target_id) {
            return Err(ApiError::bad_request("Reassignment target is not a session agent"));
        }
        let principal = hierarchy.iter().find(|node| node.id == target_id)
            .and_then(|node| node.principal.as_deref()).map(str::trim)
            .filter(|principal| !principal.is_empty())
            .ok_or_else(|| ApiError::bad_request("Reassignment target has no current principal binding"))?;
        let previous = node.binding.clone();
        node.binding = match &previous {
            BindingRef::Role(_) => BindingRef::Role(principal.to_string()),
            BindingRef::Zone(_) => BindingRef::Zone(principal.to_string()),
        };
        changes.push(serde_json::json!({"node_id": node.id, "agent_id": target_id,
            "previous_binding": previous, "binding": node.binding}));
    }
    state.event_bus.publish(Event {
        id: uuid::Uuid::new_v4().to_string(), session_id: session_id.to_string(), cell_id: None,
        agent_id: Some(caller_id.to_string()), event_type: EventType::ArtifactUpdated,
        timestamp: chrono::Utc::now(), severity: Severity::Info,
        payload: serde_json::json!({"kind": "node_reassignment_requested", "actor_agent_id": caller_id, "changes": changes}),
    }).await.map_err(ApiError::internal)?;
    if let Err(error) = manager.write_work_graph(&graph) {
        audit_reassignment_outcome(state, persistence, session_id, caller_id, &changes, false, true).await;
        return Err(ApiError::internal(error.to_string()));
    }
    if let Some(composition) = composition.as_mut() {
        composition.graph = graph;
        if let Err(error) = persistence.write_composition(&manager, composition) {
            let rollback = persistence.restore_graph(&manager,
                &state.storage.session_dir(session_id).join("state/work-graph.json"), original.as_ref());
            audit_reassignment_outcome(state, persistence, session_id, caller_id, &changes, false, rollback.is_ok()).await;
            return Err(ApiError::internal(match rollback {
                Ok(()) => format!("Reassignment failed: {error}; graph rolled back"),
                Err(rollback) => format!("Reassignment failed: {error}; graph rollback failed: {rollback}"),
            }));
        }
    }
    audit_reassignment_outcome(state, persistence, session_id, caller_id, &changes, true, false).await;
    Ok(())
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
            transcript_spawned_at: None,
            transcript_session_id: None,
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
        let headers = authenticated_headers(&state, agent_id);
        post_heartbeat(
            State(state),
            Path(SESSION_ID.to_string()),
            headers,
            Json(PostHeartbeatRequest {
                agent_id: agent_id.to_string(),
                mark_agent_id: None, reassign_nodes: Vec::new(),
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
        let headers = authenticated_headers(&state, agent_id);
        post_heartbeat(
            State(state),
            Path(SESSION_ID.to_string()),
            headers,
            Json(PostHeartbeatRequest {
                agent_id: agent_id.to_string(),
                mark_agent_id: None, reassign_nodes: Vec::new(),
                status: status.to_string(),
                summary: Some(format!("{status} knowledge acknowledgement")),
                assignment_id: None,
                completed_nodes: Vec::new(),
                knowledge_ack,
            }),
        )
        .await
    }

    fn authenticated_headers(state: &AppState, agent_id: &str) -> HeaderMap {
        let id = super::super::canonical_agent_id(SESSION_ID, agent_id);
        let token = state.pty_manager.read().mint_heartbeat_token_for_test(
            &id, SESSION_ID, id == QUEEN_ID,
        );
        let mut headers = HeaderMap::new();
        headers.insert("X-Hive-Agent-Token", token.parse().expect("header-safe credential"));
        headers
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
    async fn missing_agent_principal_rejects_a_bound_node_completion() {
        let fixture = fixture();
        let manager = write_graph_and_worker_principal(&fixture,
            TaskGraph::new(vec![work_node("T-bound", "P2")], Vec::new()), None);
        let result = heartbeat_with_completed_nodes(Arc::clone(&fixture.state), "worker-1",
            "completed", None, &["T-bound"]).await;
        assert_eq!(result.err().expect("missing principal cannot prove ownership").status, StatusCode::FORBIDDEN);
        assert!(manager.read_node_completion_facts().unwrap().is_empty());
        assert!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID).is_empty());
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

    fn auth_request(agent_id: &str, status: &str) -> PostHeartbeatRequest {
        PostHeartbeatRequest {
            agent_id: agent_id.to_string(), mark_agent_id: None, reassign_nodes: Vec::new(), status: status.to_string(),
            summary: Some("verified gate evidence".to_string()), assignment_id: None,
            completed_nodes: Vec::new(), knowledge_ack: None,
        }
    }

    #[tokio::test]
    async fn forged_heartbeat_cannot_mutate_queue_liveness_or_external_completion() {
        for verify_when_present in [false, true] {
            let fixture = fixture();
            fixture.state.pty_manager.read().set_heartbeat_verify_when_present(SESSION_ID, verify_when_present);
            fixture.state.queue_manager.repo().enqueue(&running_row()).expect("queue row");
            let manager = write_graph_and_worker_principal(&fixture,
                TaskGraph::new(vec![work_node("T-review-3", "P1")], Vec::new()), Some("P1"));
            let mut request = auth_request(WORKER_ID, "completed");
            request.assignment_id = Some(ASSIGNMENT_ID);
            request.completed_nodes = vec![CompletedNodeEntry::Legacy("T-review-3".to_string())];
            let wrong_headers = authenticated_headers(&fixture.state, "worker-2");
            let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()),
                wrong_headers, Json(request)).await;
            let error = result.err().expect("forgery must be rejected");
            assert_eq!(error.status, StatusCode::FORBIDDEN);
            let row = fixture.state.queue_manager.repo().get_row(ROW_ID).unwrap().unwrap();
            assert_eq!(row.status, QueueStatus::Running);
            assert!(row.heartbeat_at.is_none());
            assert!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID).is_empty());
            assert!(fixture.state.queue_manager.repo().external_completion_task_ids(SESSION_ID).unwrap().is_empty());
            assert!(manager.read_node_completion_facts().unwrap().is_empty());
            let events = security_events(&fixture);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["event_type"], "artifact_updated");
            assert_eq!(events[0]["payload"]["kind"], "heartbeat_rejected");
            assert_eq!(events[0]["payload"]["claimed_agent_id"], WORKER_ID);
        }
    }

    #[tokio::test]
    async fn tokenless_heartbeat_names_required_header_without_mutation() {
        let fixture = fixture();
        fixture.state.pty_manager.read().set_heartbeat_verify_when_present(SESSION_ID, false);
        let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()),
            HeaderMap::new(), Json(auth_request(WORKER_ID, "working"))).await;
        let error = result.err().expect("legacy tokenless request must be rejected");
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert!(error.message.contains("X-Hive-Agent-Token"));
        assert!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID).is_empty());
    }

    #[tokio::test]
    async fn explicit_queen_mark_is_authorized_and_audited_but_worker_mark_is_forbidden() {
        let fixture = fixture();
        let mut forbidden = auth_request(WORKER_ID, "completed");
        forbidden.mark_agent_id = Some(QUEEN_ID.to_string());
        let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()),
            authenticated_headers(&fixture.state, WORKER_ID), Json(forbidden)).await;
        assert_eq!(result.err().expect("worker cannot cross-mark").status, StatusCode::FORBIDDEN);
        let mut request = auth_request("queen", "completed");
        request.mark_agent_id = Some(WORKER_ID.to_string());
        let _ = expect_success(post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()),
            authenticated_headers(&fixture.state, QUEEN_ID), Json(request)).await, "Queen explicit mark");
        assert_eq!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID)[WORKER_ID].status, "completed");
        let events = security_events(&fixture);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["payload"]["kind"], "heartbeat_rejected");
        assert_eq!(events[0]["payload"]["caller_agent_id"], WORKER_ID);
        assert_eq!(events[0]["payload"]["resolved_agent_id"], QUEEN_ID);
        let audit = events.iter().find(|event| event["payload"]["kind"] == "heartbeat_status_requested")
            .expect("Queen status audit after rejected worker mark");
        assert_eq!(audit["event_type"], "artifact_updated");
        assert_eq!(audit["payload"]["actor_agent_id"], QUEEN_ID);
        assert_eq!(audit["payload"]["target_agent_id"], WORKER_ID);
        assert_eq!(audit["payload"]["queen_mark"], true);
        assert_eq!(audit["payload"]["summary"], "verified gate evidence");
    }

    #[tokio::test]
    async fn overwritten_completed_summary_preserves_previous_evidence() {
        let fixture = fixture();
        let _ = expect_success(heartbeat(Arc::clone(&fixture.state), WORKER_ID, "completed", None).await, "completed");
        let _ = expect_success(heartbeat(Arc::clone(&fixture.state), WORKER_ID, "working", None).await, "overwrite");
        let events = std::fs::read_to_string(fixture.state.event_bus.data_dir().join(SESSION_ID).join("events.jsonl")).unwrap();
        let last: serde_json::Value = serde_json::from_str(events.lines().last().unwrap()).unwrap();
        assert_eq!(last["payload"]["previous_status"], "completed");
        assert_eq!(last["payload"]["previous_summary"], "completed from handler test");
        assert_eq!(last["payload"]["summary"], "working from handler test");
    }


    #[tokio::test]
    async fn queen_reassignment_accepts_new_principal_and_rejects_old_principal() {
        let fixture = fixture();
        let new_id = format!("{SESSION_ID}-worker-2");
        let manager = write_graph_and_worker_principal(&fixture,
            TaskGraph::new(vec![work_node("T-transfer", "P3")], Vec::new()), Some("P3"));
        let composition = serde_json::from_value::<crate::orchestrator::work_graph::runtime::GraphCompositionState>(serde_json::json!({
            "graph": manager.read_work_graph().unwrap().unwrap(),
            "codegraph": {"available": false, "touches": {}, "unresolved_task_ids": [], "module_node_count": 0, "touch_edge_count": 0},
            "context": {"gotchas": [], "hub_lints": [], "source_fingerprints": [], "knowledge_available": false, "touches_available": false, "knowledge_edge_count": 0},
            "reviews": crate::orchestrator::work_graph::review::ReviewExpansionSidecar::default(),
        })).unwrap();
        manager.write_graph_composition_state(&composition).unwrap();
        manager.update_hierarchy(&[
            HierarchyNode { id: WORKER_ID.to_string(), role: "Worker-1".to_string(),
                principal: Some("P3".to_string()), parent_id: Some(QUEEN_ID.to_string()), children: Vec::new() },
            HierarchyNode { id: new_id.clone(), role: "Worker-2".to_string(),
                principal: Some("P1".to_string()), parent_id: Some(QUEEN_ID.to_string()), children: Vec::new() },
        ]).unwrap();
        let controller = fixture.state.session_controller.read();
        let mut session = controller.get_session(SESSION_ID).unwrap();
        session.agents.push(agent(&new_id, AgentRole::Worker { index: 2, parent: Some(QUEEN_ID.to_string()) }, Some(QUEEN_ID)));
        controller.insert_test_session(session);
        drop(controller);
        let mut request = auth_request(QUEEN_ID, "working");
        request.reassign_nodes.push(NodeReassignment { node_id: "T-transfer".to_string(), agent_id: new_id.clone() });
        let _ = expect_success(post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()),
            authenticated_headers(&fixture.state, QUEEN_ID), Json(request)).await, "Queen transfer");
        let graph = manager.read_work_graph().unwrap().unwrap();
        assert_eq!(graph.nodes[0].binding, BindingRef::Role("P1".to_string()));
        assert_eq!(manager.read_graph_composition_state().unwrap().unwrap().graph, graph);
        let old_result = heartbeat_with_completed_nodes(Arc::clone(&fixture.state), WORKER_ID,
            "completed", None, &["T-transfer"]).await;
        assert_eq!(old_result.err().expect("old principal must be rejected").status, StatusCode::BAD_REQUEST);
        let _ = expect_success(heartbeat_with_completed_nodes(Arc::clone(&fixture.state), &new_id,
            "completed", None, &["T-transfer"]).await, "new principal completion");
        let facts = manager.read_node_completion_facts().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].agent_id, new_id);
        assert!(fixture.state.queue_manager.repo().external_completion_task_ids(SESSION_ID).unwrap().contains("T-transfer"));
        let events = std::fs::read_to_string(fixture.state.event_bus.data_dir().join(SESSION_ID).join("events.jsonl")).unwrap();
        assert!(events.contains("node_reassignment_requested"));
        assert!(events.contains("P3"));
        assert!(events.contains("P1"));
    }


    #[tokio::test]
    async fn unknown_token_cannot_complete_its_claimed_owner_or_release_dependents() {
        for verify_when_present in [true, false] {
            let fixture = fixture();
            fixture.state.pty_manager.read().set_heartbeat_verify_when_present(SESSION_ID, verify_when_present);
            fixture.state.queue_manager.repo().enqueue(&running_row()).unwrap();
            let manager = write_graph_and_worker_principal(&fixture,
                TaskGraph::new(vec![work_node("T-review-3", "P1")], Vec::new()), Some("P1"));
            let _registered_headers = authenticated_headers(&fixture.state, WORKER_ID);
            let unknown = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
            let mut headers = HeaderMap::new();
            headers.insert("X-Hive-Agent-Token", unknown.parse().unwrap());
            let mut request = auth_request(WORKER_ID, "completed");
            request.assignment_id = Some(ASSIGNMENT_ID);
            request.completed_nodes = vec![CompletedNodeEntry::Legacy("T-review-3".to_string())];
            let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()), headers, Json(request)).await;
            assert_eq!(result.err().expect("unregistered token must be rejected").status, StatusCode::UNAUTHORIZED);
            let row = fixture.state.queue_manager.repo().get_row(ROW_ID).unwrap().unwrap();
            assert_eq!(row.status, QueueStatus::Running);
            assert!(row.heartbeat_at.is_none());
            assert!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID).is_empty());
            assert!(manager.read_node_completion_facts().unwrap().is_empty());
            assert!(fixture.state.queue_manager.repo().external_completion_task_ids(SESSION_ID).unwrap().is_empty());
            let events = security_events(&fixture);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["event_type"], "artifact_updated");
            assert_eq!(events[0]["payload"]["kind"], "heartbeat_rejected");
            assert_eq!(events[0]["payload"]["claimed_agent_id"], WORKER_ID);
        }
    }

    #[tokio::test]
    async fn heartbeat_rejects_credential_in_summary_before_any_persistence() {
        let fixture = fixture();
        let headers = authenticated_headers(&fixture.state, WORKER_ID);
        let mut request = auth_request(WORKER_ID, "completed");
        let secret = headers["X-Hive-Agent-Token"].to_str().unwrap().to_string();
        request.summary = Some(secret.clone());
        let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()), headers, Json(request)).await;
        assert_eq!(result.err().expect("credential-bearing payload must be rejected").status, StatusCode::BAD_REQUEST);
        assert!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID).is_empty());
        let events = security_events(&fixture);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["payload"]["kind"], "heartbeat_rejected");
        assert_eq!(events[0]["payload"]["reason"], "credential_in_payload");
        assert!(!serde_json::to_string(&events).unwrap().contains(&secret));
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "live WSL Ubuntu integration; run explicitly after C I1 and H authorization gates"]
    async fn live_wsl_environment_and_authenticated_heartbeat() {
        let fixture = fixture();
        let token = fixture.state.pty_manager.read().mint_heartbeat_token_for_test(WORKER_ID, SESSION_ID, false);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = crate::http::routes::create_router(Arc::clone(&fixture.state));
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let injected = vec![("HIVE_SESSION_ID".to_string(), SESSION_ID.to_string()),
            ("HIVE_AGENT_ID".to_string(), WORKER_ID.to_string()), ("HIVE_AGENT_TOKEN".to_string(), token),
            ("HIVE_WRONG_AGENT_TOKEN".to_string(), format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()))];
        let inherited = std::env::vars_os().collect::<Vec<_>>();
        let environment = crate::adapters::wsl::launch_env("wsl.exe", None, &injected, &inherited).unwrap();
        let script = format!(r#"test -n "$HIVE_AGENT_TOKEN" || exit 21
with_header=$(cat <<'JSON' | curl.exe -sS -o NUL -w '%{{http_code}}' -X POST 'http://localhost:{port}/api/sessions/{SESSION_ID}/heartbeat' -H 'Content-Type: application/json' -H "X-Hive-Agent-Token: $HIVE_AGENT_TOKEN" --data-binary @-
{{"agent_id":"{WORKER_ID}","status":"completed","summary":"WSL live development-server check"}}
JSON
) || exit 22
without_header=$(cat <<'JSON' | curl.exe -sS -o NUL -w '%{{http_code}}' -X POST 'http://localhost:{port}/api/sessions/{SESSION_ID}/heartbeat' -H 'Content-Type: application/json' --data-binary @-
{{"agent_id":"{WORKER_ID}","status":"completed"}}
JSON
) || exit 23
wrong_token=$(cat <<'JSON' | curl.exe -sS -o NUL -w '%{{http_code}}' -X POST 'http://localhost:{port}/api/sessions/{SESSION_ID}/heartbeat' -H 'Content-Type: application/json' -H "X-Hive-Agent-Token: $HIVE_WRONG_AGENT_TOKEN" --data-binary @-
{{"agent_id":"{WORKER_ID}","status":"idle"}}
JSON
) || exit 24
printf 'env_present=yes with_header=%s without_header=%s wrong_token=%s\n' "$with_header" "$without_header" "$wrong_token"
test "$with_header" = 200 && test "$without_header" = 200 && test "$wrong_token" = 401
"#);
        let mut command = tokio::process::Command::new("wsl.exe");
        command.args(["-d", "Ubuntu", "bash", "--noprofile", "--norc", "-s"]).envs(environment);
        command.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        command.kill_on_drop(true);
        let mut child = command.spawn().expect("WSL process launch");
        {
            use tokio::io::AsyncWriteExt;
            let mut stdin = child.stdin.take().expect("WSL script input");
            stdin.write_all(script.as_bytes()).await.expect("WSL script write");
        }
        let output = tokio::time::timeout(std::time::Duration::from_secs(45), child.wait_with_output()).await;
        server.abort();
        let output = output.expect("WSL check timeout").expect("WSL process launch");
        // Only the fixed presence/status summary is printed; stderr/command arguments remain private.
        assert!(output.status.success(), "WSL live check failed (no command/environment output printed)");
        let summary = String::from_utf8(output.stdout).unwrap();
        assert!(summary.contains("env_present=yes with_header=200 without_header=200 wrong_token=401"));
        println!("WSL new-code ephemeral server: env_present=yes with_header=200 without_header=200 wrong_token=401");
        assert_eq!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID)[WORKER_ID].status, "completed");
        let events = security_events(&fixture);
        assert!(events.iter().any(|event| event["payload"]["kind"] == "heartbeat_unauthenticated"));
        assert!(events.iter().any(|event| event["payload"]["kind"] == "heartbeat_rejected"
            && event["payload"]["reason"] == "invalid_token"));
    }


    #[tokio::test]
    async fn queen_queue_backed_reassignment_is_explicit_conflict_without_mutation() {
        let fixture = fixture();
        fixture.state.queue_manager.repo().enqueue(&running_row()).unwrap();
        let manager = write_graph_and_worker_principal(&fixture,
            TaskGraph::new(vec![work_node("T-review-3", "P3")], Vec::new()), Some("P3"));
        let before = manager.read_work_graph().unwrap();
        let mut request = auth_request(QUEEN_ID, "working");
        request.reassign_nodes.push(NodeReassignment { node_id: "T-review-3".to_string(), agent_id: WORKER_ID.to_string() });
        let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()),
            authenticated_headers(&fixture.state, QUEEN_ID), Json(request)).await;
        let error = result.err().expect("queue-backed transfer must retain its assignment fence");
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(error.message.contains("durable assignment"));
        assert_eq!(manager.read_work_graph().unwrap(), before);
        assert!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID).is_empty());
    }


    fn security_events(fixture: &HeartbeatFixture) -> Vec<serde_json::Value> {
        std::fs::read_to_string(fixture.state.event_bus.data_dir().join(SESSION_ID).join("events.jsonl"))
            .unwrap_or_default().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    #[tokio::test]
    async fn verify_when_present_accepts_absent_header_with_unattributed_audit_only_for_that_session() {
        let fixture = fixture();
        assert!(fixture.state.pty_manager.read().heartbeat_verifies_when_present(SESSION_ID));
        fixture.state.pty_manager.read().set_heartbeat_verify_when_present(SESSION_ID, false);
        assert!(!fixture.state.pty_manager.read().heartbeat_verifies_when_present(SESSION_ID));
        assert!(fixture.state.pty_manager.read().heartbeat_verifies_when_present("another-session"));
        fixture.state.pty_manager.read().set_heartbeat_verify_when_present(SESSION_ID, true);
        assert!(fixture.state.pty_manager.read().heartbeat_verifies_when_present("another-session"));
        let _ = expect_success(post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()),
            HeaderMap::new(), Json(auth_request(WORKER_ID, "working"))).await, "compatible tokenless beat");
        let events = security_events(&fixture);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event_type"], "artifact_updated");
        assert!(events[0]["agent_id"].is_null());
        assert!(events[0]["payload"]["caller_agent_id"].is_null());
        assert_eq!(events[0]["payload"]["claimed_agent_id"], WORKER_ID);
        assert_eq!(events[0]["payload"]["resolved_agent_id"], WORKER_ID);
        assert_eq!(events[0]["payload"]["kind"], "heartbeat_unauthenticated");
        assert!(events[0]["payload"].get("worker_id").is_none());
        fixture.state.pty_manager.read().set_heartbeat_verify_when_present(SESSION_ID, false);
        assert!(!fixture.state.pty_manager.read().heartbeat_verifies_when_present(SESSION_ID));
    }

    #[tokio::test]
    async fn compatible_mode_never_treats_present_invalid_or_duplicate_headers_as_absent() {
        for kind in ["malformed", "duplicate", "duplicate_empty", "wrong_session"] {
            let fixture = fixture();
            fixture.state.pty_manager.read().set_heartbeat_verify_when_present(SESSION_ID, true);
            let mut headers = HeaderMap::new();
            match kind {
                "malformed" => { headers.insert("X-Hive-Agent-Token", axum::http::HeaderValue::from_bytes(&[0xff]).unwrap()); }
                "duplicate" => {
                    headers = authenticated_headers(&fixture.state, WORKER_ID);
                    let duplicate = headers["X-Hive-Agent-Token"].clone();
                    headers.append("X-Hive-Agent-Token", duplicate);
                }
                "duplicate_empty" => {
                    headers.insert("X-Hive-Agent-Token", "".parse().unwrap());
                    headers.append("X-Hive-Agent-Token", "".parse().unwrap());
                }
                _ => {
                    let token = fixture.state.pty_manager.read().mint_heartbeat_token_for_test(WORKER_ID, "another-session", false);
                    headers.insert("X-Hive-Agent-Token", token.parse().unwrap());
                }
            }
            let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()),
                headers, Json(auth_request(WORKER_ID, "completed"))).await;
            assert_eq!(result.err().expect("present invalid header must reject").status, StatusCode::UNAUTHORIZED);
            assert!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID).is_empty());
            let events = security_events(&fixture);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["payload"]["kind"], "heartbeat_rejected");
            assert!(events[0]["agent_id"].is_null());
        }
    }

    #[tokio::test]
    async fn tokenless_queen_claim_cannot_acquire_mark_or_reassignment_authority() {
        for reassign in [false, true] {
            let fixture = fixture();
            fixture.state.pty_manager.read().set_heartbeat_verify_when_present(SESSION_ID, true);
            let mut request = auth_request(QUEEN_ID, "working");
            if reassign {
                request.reassign_nodes.push(NodeReassignment { node_id: "T-transfer".to_string(), agent_id: WORKER_ID.to_string() });
            } else { request.mark_agent_id = Some(WORKER_ID.to_string()); }
            let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()), HeaderMap::new(), Json(request)).await;
            assert_eq!(result.err().expect("tokenless Queen claim is not authority").status, StatusCode::FORBIDDEN);
            assert!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID).is_empty());
            let events = security_events(&fixture);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["payload"]["kind"], "heartbeat_rejected");
            assert!(events[0]["payload"]["caller_agent_id"].is_null());
        }
    }

    #[tokio::test]
    async fn rejection_audit_redacts_a_presented_credential_used_as_a_claimed_identity() {
        let fixture = fixture();
        let unknown = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let mut headers = HeaderMap::new();
        headers.insert("X-Hive-Agent-Token", unknown.parse().unwrap());
        let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()),
            headers, Json(auth_request(&unknown, "working"))).await;
        assert_eq!(result.err().expect("unknown credential").status, StatusCode::UNAUTHORIZED);
        let events = security_events(&fixture);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["payload"]["claimed_agent_id"], "redacted-credential");
        assert!(!serde_json::to_string(&events).unwrap().contains(&unknown));
    }

    #[tokio::test]
    async fn duplicate_header_audit_cannot_echo_a_credential_in_the_second_value() {
        let fixture = fixture();
        let token = fixture.state.pty_manager.read().mint_heartbeat_token_for_test(WORKER_ID, SESSION_ID, false);
        let mut headers = HeaderMap::new();
        headers.insert("X-Hive-Agent-Token", "not-a-credential".parse().unwrap());
        headers.append("X-Hive-Agent-Token", token.parse().unwrap());
        let mut request = auth_request(&token, "working");
        request.mark_agent_id = Some(token.clone());
        let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()), headers, Json(request)).await;
        assert_eq!(result.err().expect("duplicate header").status, StatusCode::UNAUTHORIZED);
        let events = security_events(&fixture);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["payload"]["claimed_agent_id"], "redacted-untrusted-identity");
        assert!(!serde_json::to_string(&events).unwrap().contains(&token));
    }

    #[tokio::test]
    async fn valid_credential_in_claimed_or_mark_identity_is_rejected_with_sanitized_audit() {
        for mark in [false, true] {
            let fixture = fixture();
            let headers = authenticated_headers(&fixture.state, WORKER_ID);
            let secret = headers["X-Hive-Agent-Token"].to_str().unwrap().to_string();
            let mut request = auth_request(WORKER_ID, "completed");
            if mark { request.mark_agent_id = Some(secret.clone()); }
            else { request.agent_id = secret.clone(); }
            let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()), headers, Json(request)).await;
            assert_eq!(result.err().expect("credential-bearing target").status, StatusCode::BAD_REQUEST);
            assert!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID).is_empty());
            let events = security_events(&fixture);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["payload"]["kind"], "heartbeat_rejected");
            assert_eq!(events[0]["payload"]["reason"], "credential_in_payload");
            assert!(!serde_json::to_string(&events).unwrap().contains(&secret));
        }
    }

    #[tokio::test]
    async fn rejected_unknown_session_never_creates_an_audit_directory() {
        let fixture = fixture();
        let unknown = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let mut headers = HeaderMap::new();
        headers.insert("X-Hive-Agent-Token", unknown.parse().unwrap());
        let directory = fixture.state.event_bus.data_dir();
        let before = std::fs::read_dir(directory).unwrap().count();
        let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(unknown.clone()), headers,
            Json(auth_request(WORKER_ID, "working"))).await;
        assert_eq!(result.err().expect("unknown session").status, StatusCode::UNAUTHORIZED);
        assert_eq!(std::fs::read_dir(directory).unwrap().count(), before);
        assert!(!directory.join(&unknown).exists());
        assert!(!directory.join("redacted-credential").exists());
        assert!(!directory.join("unattributed-session").exists());
    }

    #[tokio::test]
    async fn compatibility_retains_membership_assignment_and_principal_fences() {
        for case in ["ghost", "stale", "binding", "queen_no_principal"] {
            let fixture = fixture();
            fixture.state.pty_manager.read().set_heartbeat_verify_when_present(SESSION_ID, true);
            fixture.state.queue_manager.repo().enqueue(&running_row()).unwrap();
            let manager = write_graph_and_worker_principal(&fixture,
                TaskGraph::new(vec![work_node("T-review-3", "P1")], Vec::new()), Some("P2"));
            let mut request = auth_request(if case == "ghost" { "unknown-worker" }
                else if case == "queen_no_principal" { QUEEN_ID } else { WORKER_ID }, "completed");
            let expected = match case {
                "ghost" => StatusCode::NOT_FOUND,
                "stale" => { request.assignment_id = Some(ASSIGNMENT_ID + 1); StatusCode::CONFLICT }
                "binding" => {
                    request.completed_nodes.push(CompletedNodeEntry::Legacy("T-review-3".to_string()));
                    StatusCode::BAD_REQUEST
                }
                _ => {
                    request.completed_nodes.push(CompletedNodeEntry::Legacy("T-review-3".to_string()));
                    StatusCode::FORBIDDEN
                }
            };
            let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()), HeaderMap::new(), Json(request)).await;
            assert_eq!(result.err().expect("compatibility must preserve domain fences").status, expected);
            let row = fixture.state.queue_manager.repo().get_row(ROW_ID).unwrap().unwrap();
            assert_eq!(row.status, QueueStatus::Running);
            assert!(row.heartbeat_at.is_none());
            assert!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID).is_empty());
            assert!(manager.read_node_completion_facts().unwrap().is_empty());
            assert!(fixture.state.queue_manager.repo().external_completion_task_ids(SESSION_ID).unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn failed_audit_rejects_unauthenticated_acceptance_and_preserves_auth_rejection_status() {
        for verify_when_present in [false, true] {
            let fixture = fixture();
            fixture.state.pty_manager.read().set_heartbeat_verify_when_present(SESSION_ID, verify_when_present);
            std::fs::create_dir_all(fixture.state.event_bus.data_dir().join(SESSION_ID).join("events.jsonl")).unwrap();
            let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()),
                HeaderMap::new(), Json(auth_request(WORKER_ID, "working"))).await;
            let expected = if verify_when_present { StatusCode::INTERNAL_SERVER_ERROR } else { StatusCode::UNAUTHORIZED };
            assert_eq!(result.err().expect("audit failure").status, expected);
            assert!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID).is_empty());
        }
    }

    #[tokio::test]
    async fn heartbeat_mutation_locks_serialize_one_session_without_blocking_another() {
        let first = heartbeat_mutation_lock("lock-test-a");
        let same = heartbeat_mutation_lock("lock-test-a");
        let other = heartbeat_mutation_lock("lock-test-b");
        assert!(Arc::ptr_eq(&first, &same));
        assert!(!Arc::ptr_eq(&first, &other));
        let _held = first.lock().await;
        assert!(same.try_lock().is_err());
        assert!(other.try_lock().is_ok());
    }

    #[tokio::test]
    async fn codex_empty_and_whitespace_token_headers_are_accepted_and_logged_as_unauthenticated() {
        for value in ["", "   ", " \t "] {
            let fixture = fixture();
            assert_eq!(fixture.state.session_controller.read().get_session(SESSION_ID).unwrap().default_cli, "codex");
            let mut headers = HeaderMap::new();
            headers.insert("X-Hive-Agent-Token", value.parse().unwrap());
            let _ = expect_success(post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()),
                headers, Json(auth_request(WORKER_ID, "working"))).await, "Codex no usable token");
            assert_eq!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID)[WORKER_ID].status, "working");
            let events = security_events(&fixture);
            assert_eq!(events.len(), 1);
            assert!(events[0]["agent_id"].is_null());
            assert_eq!(events[0]["payload"]["kind"], "heartbeat_unauthenticated");
            assert!(events[0]["payload"]["caller_agent_id"].is_null());
            assert_eq!(events[0]["payload"]["claimed_agent_id"], WORKER_ID);
        }
    }

    #[tokio::test]
    async fn claude_queen_with_valid_scoped_credential_can_mark_but_an_empty_header_has_no_authority() {
        let fixture = fixture();
        {
            let controller = fixture.state.session_controller.read();
            let mut session = controller.get_session(SESSION_ID).unwrap();
            session.default_cli = "claude".to_string();
            controller.insert_test_session(session);
        }
        for value in ["", "   "] {
            let mut headers = HeaderMap::new();
            headers.insert("X-Hive-Agent-Token", value.parse().unwrap());
            let mut request = auth_request(QUEEN_ID, "completed");
            request.mark_agent_id = Some(WORKER_ID.to_string());
            let result = post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()), headers, Json(request)).await;
            assert_eq!(result.err().expect("empty Queen claim cannot mark").status, StatusCode::FORBIDDEN);
            assert!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID).is_empty());
        }
        let headers = authenticated_headers(&fixture.state, QUEEN_ID);
        assert!(fixture.state.pty_manager.read().authenticate_heartbeat(SESSION_ID,
            headers["X-Hive-Agent-Token"].to_str().unwrap()).is_some_and(|(id, queen)| id == QUEEN_ID && queen));
        let mut request = auth_request(QUEEN_ID, "completed");
        request.mark_agent_id = Some(WORKER_ID.to_string());
        let _ = expect_success(post_heartbeat(State(Arc::clone(&fixture.state)), Path(SESSION_ID.to_string()),
            headers, Json(request)).await, "authenticated Claude Queen mark");
        let events = security_events(&fixture);
        assert_eq!(events.iter().filter(|event| event["payload"]["kind"] == "heartbeat_rejected").count(), 2);
        let mark = events.iter().find(|event| event["payload"]["kind"] == "heartbeat_status_requested").unwrap();
        assert_eq!(mark["agent_id"], QUEEN_ID);
        assert_eq!(mark["payload"]["actor_agent_id"], QUEEN_ID);
        assert_eq!(mark["payload"]["target_agent_id"], WORKER_ID);
        assert_eq!(mark["payload"]["queen_mark"], true);
        assert_eq!(fixture.state.session_controller.read().get_heartbeat_info(SESSION_ID)[WORKER_ID].status, "completed");
    }

    fn reassignment_persistence_fixture(standalone: bool) -> (HeartbeatFixture, StateManager) {
        let fixture = fixture();
        let manager = write_graph_and_worker_principal(&fixture,
            TaskGraph::new(vec![work_node("T-transfer", "P3")], Vec::new()), Some("P1"));
        let composition = serde_json::from_value::<crate::orchestrator::work_graph::runtime::GraphCompositionState>(serde_json::json!({
            "graph": manager.read_work_graph().unwrap().unwrap(),
            "codegraph": {"available": false, "touches": {}, "unresolved_task_ids": [], "module_node_count": 0, "touch_edge_count": 0},
            "context": {"gotchas": [], "hub_lints": [], "source_fingerprints": [], "knowledge_available": false, "touches_available": false, "knowledge_edge_count": 0},
            "reviews": crate::orchestrator::work_graph::review::ReviewExpansionSidecar::default(),
        })).unwrap();
        manager.write_graph_composition_state(&composition).unwrap();
        if standalone {
            // The standalone projection may differ from the authoritative composition.
            manager.write_work_graph(&TaskGraph::new(vec![work_node("T-transfer", "P2")], Vec::new())).unwrap();
        } else {
            std::fs::remove_file(fixture.state.storage.session_dir(SESSION_ID).join("state/work-graph.json")).unwrap();
        }
        (fixture, manager)
    }

    async fn run_reassignment(fixture: &HeartbeatFixture, persistence: &ReassignmentPersistence) -> Result<(), ApiError> {
        reassign_nodes_with_persistence(&fixture.state, SESSION_ID, QUEEN_ID,
            &[NodeReassignment { node_id: "T-transfer".to_string(), agent_id: WORKER_ID.to_string() }], persistence).await
    }

    #[tokio::test]
    async fn reassignment_composition_failure_removes_new_standalone_graph() {
        let (fixture, manager) = reassignment_persistence_fixture(false);
        let path = fixture.state.storage.session_dir(SESSION_ID).join("state/work-graph-composition.json");
        let before = std::fs::read(&path).unwrap();
        let persistence = ReassignmentPersistence { fail_composition: true, ..Default::default() };
        let error = run_reassignment(&fixture, &persistence).await.unwrap_err();
        assert!(error.message.contains("injected composition persistence failure"));
        assert!(manager.read_work_graph().unwrap().is_none(), "failed reassignment must remove its new standalone graph");
        assert_eq!(std::fs::read(path).unwrap(), before);
        let events = security_events(&fixture);
        let failure = events.iter().find(|event| event["payload"]["kind"] == "node_reassignment_failed").expect("failed outcome audit");
        assert_eq!(failure["payload"]["rolled_back"], true);
        assert!(!events.iter().any(|event| event["payload"]["kind"] == "node_reassignment_applied"));
    }

    #[tokio::test]
    async fn reassignment_composition_failure_restores_prior_graph_and_audits_failure() {
        let (fixture, manager) = reassignment_persistence_fixture(true);
        let original = manager.read_work_graph().unwrap();
        let path = fixture.state.storage.session_dir(SESSION_ID).join("state/work-graph-composition.json");
        let before = std::fs::read(&path).unwrap();
        let persistence = ReassignmentPersistence { fail_composition: true, ..Default::default() };
        assert!(run_reassignment(&fixture, &persistence).await.is_err());
        assert_eq!(manager.read_work_graph().unwrap(), original);
        assert_eq!(std::fs::read(path).unwrap(), before);
        let events = security_events(&fixture);
        assert!(events.iter().any(|event| event["payload"]["kind"] == "node_reassignment_failed"
            && event["payload"]["rolled_back"] == true), "restored mutation needs a failed outcome audit");
        assert!(!events.iter().any(|event| event["payload"]["kind"] == "node_reassignment_applied"));
    }

    #[tokio::test]
    async fn reassignment_restore_failure_preserves_both_errors() {
        for standalone in [false, true] {
            let (fixture, _) = reassignment_persistence_fixture(standalone);
            let persistence = ReassignmentPersistence { fail_composition: true, fail_restore: true, ..Default::default() };
            let error = run_reassignment(&fixture, &persistence).await.unwrap_err();
            assert!(error.message.contains("injected composition persistence failure"));
            assert!(error.message.contains("injected graph rollback failure"), "both persistence and rollback failures must survive");
            let events = security_events(&fixture);
            assert!(events.iter().any(|event| event["payload"]["kind"] == "node_reassignment_failed"
                && event["payload"]["rolled_back"] == false));
        }
    }

    #[tokio::test]
    async fn reassignment_applied_outcome_follows_committed_bindings() {
        let (fixture, manager) = reassignment_persistence_fixture(false);
        run_reassignment(&fixture, &ReassignmentPersistence::default()).await.unwrap_or_else(|error| panic!("{}", error.message));
        let graph = manager.read_work_graph().unwrap().unwrap();
        assert_eq!(graph.nodes[0].binding, BindingRef::Role("P1".to_string()));
        assert_eq!(manager.read_graph_composition_state().unwrap().unwrap().graph, graph);
        let events = security_events(&fixture);
        let kinds: Vec<_> = events.iter().map(|event| event["payload"]["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["node_reassignment_requested", "node_reassignment_applied"]);
    }

    #[tokio::test]
    async fn reassignment_outcome_audit_failure_does_not_report_committed_mutation_as_failed() {
        let (fixture, manager) = reassignment_persistence_fixture(false);
        let persistence = ReassignmentPersistence { fail_outcome_audit: true, ..Default::default() };
        run_reassignment(&fixture, &persistence).await.unwrap_or_else(|error| panic!("committed binding must remain successful: {}", error.message));
        assert_eq!(persistence.outcome_attempts.load(std::sync::atomic::Ordering::Relaxed), 1, "exercise the injected outcome-audit failure");
        let graph = manager.read_work_graph().unwrap().unwrap();
        assert_eq!(graph.nodes[0].binding, BindingRef::Role("P1".to_string()));
        assert_eq!(manager.read_graph_composition_state().unwrap().unwrap().graph, graph);
        assert!(!security_events(&fixture).iter().any(|event| event["payload"]["kind"] == "node_reassignment_failed"));
    }

}
