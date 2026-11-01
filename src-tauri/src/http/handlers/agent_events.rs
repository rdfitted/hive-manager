//! Agent feeds use their own persisted journals and one multiplexed SSE connection.
use super::{validate_agent_id, validate_session_id};
use crate::{
    http::{error::ApiError, state::AppState},
    transport::{
        feed::{self, Cursor, HistoryPage},
        Capabilities, EventSource,
    },
};
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, path::PathBuf, sync::Arc, time::Duration};
use tokio_stream::wrappers::ReceiverStream;

#[derive(Serialize, Deserialize)]
pub struct AgentFeed {
    pub agent_id: String,
    pub transcript_session_id: Option<String>,
    pub capabilities: Capabilities,
}
#[derive(Serialize, Deserialize)]
pub struct AgentFeeds {
    pub session_id: String,
    pub agents: Vec<AgentFeed>,
}

fn validate_session(id: &str) -> Result<(), ApiError> {
    validate_session_id(id)?;
    feed::safe_component(id).map_err(ApiError::bad_request)
}

fn configure(state: &AppState, session_id: &str) -> Result<Vec<AgentFeed>, ApiError> {
    validate_session(session_id)?;
    let controller = state.session_controller.read();
    let registry = state.transports();
    let agents: Vec<(String, String, Option<String>, PathBuf)> =
        if let Some(session) = controller.get_session(session_id) {
            session
                .agents
                .iter()
                .map(|agent| {
                    (
                        agent.id.clone(),
                        agent.config.cli.clone(),
                        agent.transcript_session_id.clone(),
                        transcript_cwd(&session, agent),
                    )
                })
                .collect()
        } else {
            let session = state
                .storage
                .load_session(session_id)
                .map_err(|_| ApiError::not_found("Session not found"))?;
            session
                .agents
                .iter()
                .map(|agent| {
                    let cwd = if session.no_git {
                        PathBuf::from(&session.project_path)
                    } else if session.execution_policy.workspace_strategy
                        == crate::domain::WorkspaceStrategy::SharedCell
                        && (agent.role == "Queen" || agent.role.starts_with("Worker("))
                    {
                        session
                            .worktree_path
                            .as_ref()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| PathBuf::from(&session.project_path))
                    } else if let Some(index) = agent
                        .role
                        .strip_prefix("Worker(")
                        .and_then(|s| s.split([',', ')']).next())
                        .and_then(|s| s.parse::<u8>().ok())
                    {
                        PathBuf::from(&session.project_path)
                            .join(".hive-manager/worktrees")
                            .join(session_id)
                            .join(format!("worker-{index}"))
                    } else if agent.role == "Queen" {
                        PathBuf::from(&session.project_path)
                            .join(".hive-manager/worktrees")
                            .join(session_id)
                            .join("queen")
                    } else {
                        session
                            .worktree_path
                            .as_ref()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| PathBuf::from(&session.project_path))
                    };
                    (
                        agent.id.clone(),
                        agent.config.cli.clone(),
                        agent.transcript_session_id.clone(),
                        cwd,
                    )
                })
                .collect()
        };
    drop(controller);
    let mut output = Vec::new();
    for (agent_id, cli, transcript_session_id, cwd) in agents {
        validate_agent_id(&agent_id)?;
        let capabilities = if let Some(entry) = registry.get(session_id, &agent_id) {
            entry.transport.capabilities()
        } else if registry
            .feeds
            .is_protocol(session_id, &agent_id)
            .map_err(ApiError::internal)?
        {
            // Saved protocol seats expose their journal without advertising live controls.
            Capabilities {
                has_terminal: false,
                can_interrupt: false,
                supports_approvals: false,
                event_source: EventSource::Protocol,
            }
        } else {
            let has_transcript = cli == "claude"
                && transcript_session_id
                    .as_ref()
                    .is_some_and(|id| feed::safe_component(id).is_ok());
            if has_transcript {
                registry
                    .feeds
                    .use_transcript(
                        session_id,
                        &agent_id,
                        &cwd,
                        transcript_session_id.as_deref().unwrap(),
                    )
                    .map_err(ApiError::internal)?;
            }
            Capabilities::pty(has_transcript)
        };
        output.push(AgentFeed {
            agent_id,
            transcript_session_id,
            capabilities,
        });
    }
    Ok(output)
}

fn transcript_cwd(session: &crate::session::Session, agent: &crate::session::AgentInfo) -> PathBuf {
    use crate::{domain::WorkspaceStrategy, pty::AgentRole};
    if session.no_git {
        return session.project_path.clone();
    }
    if session.execution_policy.workspace_strategy == WorkspaceStrategy::SharedCell
        && matches!(agent.role, AgentRole::Queen | AgentRole::Worker { .. })
    {
        return session
            .worktree_path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| session.project_path.clone());
    }
    let worktrees = session
        .project_path
        .join(".hive-manager/worktrees")
        .join(&session.id);
    match &agent.role {
        AgentRole::Worker { index, .. } => worktrees.join(format!("worker-{index}")),
        AgentRole::Queen
            if matches!(
                session.session_type,
                crate::session::SessionType::Hive { .. }
            ) =>
        {
            worktrees.join("queen")
        }
        _ => session
            .worktree_path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| session.project_path.clone()),
    }
}

pub async fn get_agent_feeds(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<AgentFeeds>, ApiError> {
    validate_session(&id)?;
    tokio::task::spawn_blocking(move || {
        let agents = configure(&state, &id)?;
        Ok(Json(AgentFeeds {
            session_id: id,
            agents,
        }))
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
}

#[derive(Default, Deserialize)]
pub struct HistoryQuery {
    pub before: Option<String>,
    pub after: Option<String>,
    pub limit: Option<usize>,
}
pub async fn get_agent_events(
    State(state): State<Arc<AppState>>,
    Path((id, aid)): Path<(String, String)>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<HistoryPage>, ApiError> {
    validate_session(&id)?;
    validate_agent_id(&aid)?;
    if query.before.is_some() && query.after.is_some() {
        return Err(ApiError::bad_request("Use before or after, not both"));
    }
    for cursor in [&query.before, &query.after].into_iter().flatten() {
        Cursor::decode(cursor).map_err(ApiError::bad_request)?;
    }
    tokio::task::spawn_blocking(move || {
        let agents = configure(&state, &id)?;
        if !agents.iter().any(|a| a.agent_id == aid) {
            return Err(ApiError::not_found("Agent not found in session"));
        }
        state
            .transports()
            .feeds
            .refresh_transcript(&id, &aid)
            .map_err(ApiError::internal)?;
        state
            .transports()
            .feeds
            .history(
                &id,
                &aid,
                query.before.as_deref(),
                query.after.as_deref(),
                query.limit.unwrap_or(100),
            )
            .map(Json)
            .map_err(ApiError::internal)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
}

#[derive(Default, Deserialize)]
pub struct StreamQuery {
    pub cursors: Option<String>,
}
pub async fn stream_agent_events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    validate_session(&id)?;
    let token = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or(query.cursors);
    let mut checkpoint = token
        .as_deref()
        .map(feed::decode_checkpoint)
        .transpose()
        .map_err(ApiError::bad_request)?
        .unwrap_or_default();
    let agents = configure(&state, &id)?;
    // Removed roster members cannot be read, but must not poison browser reconnects.
    checkpoint.retain(|key, _| agents.iter().any(|agent| &agent.agent_id == key));
    // Subscribe BEFORE replay. Broadcasts only wake the durable reader; lag cannot
    // lose data, and a live event raced with replay is deduplicated by its cursor.
    let mut wake = state.transports().feeds.subscribe();
    let (sender, receiver) = tokio::sync::mpsc::channel(32);
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_millis(250));
        loop {
            let state_copy = state.clone();
            let sid = id.clone();
            let result = tokio::task::spawn_blocking(move || {
                let agents = configure(&state_copy, &sid)?;
                for agent in &agents {
                    if agent.capabilities.event_source != EventSource::None {
                        state_copy
                            .transports()
                            .feeds
                            .refresh_transcript(&sid, &agent.agent_id)
                            .map_err(ApiError::internal)?;
                    }
                }
                Ok::<_, ApiError>(agents)
            })
            .await;
            let agents = match result {
                Ok(Ok(agents)) => agents,
                _ => {
                    let _ = sender
                        .send(Ok(Event::default()
                            .event("feed_error")
                            .data("{\"error\":\"Agent feed unavailable\"}")))
                        .await;
                    break;
                }
            };
            checkpoint.retain(|key, _| agents.iter().any(|agent| &agent.agent_id == key));
            for agent in agents {
                if agent.capabilities.event_source == EventSource::None {
                    continue;
                }
                loop {
                    let registry = state.transports().clone();
                    let sid = id.clone();
                    let aid = agent.agent_id.clone();
                    let after = checkpoint.get(&aid).cloned();
                    let page = tokio::task::spawn_blocking(move || {
                        registry
                            .feeds
                            .history(&sid, &aid, None, after.as_deref(), 200)
                    })
                    .await;
                    let page = match page {
                        Ok(Ok(page)) => page,
                        _ => return,
                    };
                    if page.reset {
                        checkpoint.remove(&agent.agent_id);
                        let data = serde_json::json!({"agent_id": agent.agent_id}).to_string();
                        if sender
                            .send(Ok(Event::default().event("feed_reset").data(data)))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    let empty = page.events.is_empty();
                    for event in page.events {
                        if checkpoint
                            .get(&agent.agent_id)
                            .is_some_and(|old| !is_newer(&event.cursor, old))
                        {
                            continue;
                        }
                        checkpoint.insert(agent.agent_id.clone(), event.cursor.clone());
                        let frame = Event::default()
                            .event("agent_event")
                            .id(feed::encode_checkpoint(&checkpoint))
                            .data(serde_json::to_string(&event).expect("event"));
                        if sender.send(Ok(frame)).await.is_err() {
                            return;
                        }
                    }
                    if empty || !page.has_more {
                        break;
                    }
                }
            }
            tokio::select! {
                _ = sender.closed() => break,
                _ = timer.tick() => {},
                result = wake.recv() => { if matches!(result, Err(tokio::sync::broadcast::error::RecvError::Closed)) { break; } },
            }
        }
    });
    Ok(Sse::new(ReceiverStream::new(receiver)).keep_alive(KeepAlive::default()))
}

fn is_newer(new: &str, old: &str) -> bool {
    match (Cursor::decode(new), Cursor::decode(old)) {
        (Ok(new), Ok(old)) => new.generation != old.generation || new.byte_offset > old.byte_offset,
        _ => false,
    }
}
