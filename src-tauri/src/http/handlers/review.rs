use std::{path::PathBuf, sync::Arc};
use axum::{extract::{Path, Query, State}, Json};
use serde::Deserialize;
use crate::{
    domain::WorkspaceStrategy,
    http::{error::ApiError, state::AppState},
    pty::AgentRole,
    review::diff::{self, ReviewDiff},
    session::{cell_status::{agent_in_cell, session_cell_ids}, AgentInfo, Session, SessionType},
};
use super::{validate_agent_id, validate_cell_id, validate_session_id};

#[derive(Deserialize)]
pub struct ReviewQuery {
    pub agent_id: String,
}

fn workspace(session: &Session, agent: &AgentInfo) -> Result<PathBuf, ApiError> {
    if session.no_git { return Err(ApiError::bad_request("Review requires a git workspace")); }
    if matches!(session.session_type, SessionType::Hive { .. })
        && session.execution_policy.workspace_strategy == WorkspaceStrategy::SharedCell
        && matches!(agent.role, AgentRole::Queen | AgentRole::Worker { .. })
    {
        return session.worktree_path.as_ref().map(PathBuf::from)
            .ok_or_else(|| ApiError::bad_request("Shared cell workspace is unavailable"));
    }
    let root = session.project_path.join(".hive-manager").join("worktrees").join(&session.id);
    match &agent.role {
        AgentRole::Fusion { variant } => {
            let debate = matches!(session.session_type, SessionType::Debate { .. });
            let metadata = session.project_path.join(".hive-manager").join(&session.id)
                .join(if debate { "debate-config.json" } else { "fusion-config.json" });
            let bytes = std::fs::read(metadata).map_err(|_| ApiError::bad_request("Cell workspace metadata is unavailable"))?;
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::bad_request("Invalid cell workspace metadata"))?;
            value[if debate { "debaters" } else { "variants" }].as_array()
                .and_then(|entries| entries.iter().find(|entry| entry["name"].as_str() == Some(variant.as_str())
                    || entry["agent_id"].as_str() == Some(agent.id.as_str())))
                .and_then(|entry| entry["worktree_path"].as_str()).map(PathBuf::from)
                .ok_or_else(|| ApiError::bad_request("Principal workspace is unavailable"))
        }
        AgentRole::Queen => Ok(root.join("queen")),
        AgentRole::Worker { index, .. } => Ok(root.join(format!("worker-{index}"))),
        _ => Err(ApiError::bad_request("Review is available for owning Queen and worker principals")),
    }
}

pub async fn get_review_diff(
    State(state): State<Arc<AppState>>,
    Path((session_id, cell_id)): Path<(String, String)>,
    Query(query): Query<ReviewQuery>,
) -> Result<Json<ReviewDiff>, ApiError> {
    validate_session_id(&session_id)?;
    validate_cell_id(&cell_id)?;
    validate_agent_id(&query.agent_id)?;
    let (root, base) = {
        let controller = state.session_controller.read();
        let session = controller.get_session(&session_id)
            .ok_or_else(|| ApiError::not_found("Session not found"))?;
        if !session_cell_ids(&session).contains(&cell_id) {
            return Err(ApiError::not_found("Cell not found"));
        }
        let agent = session.agents.iter().find(|a| a.id == query.agent_id && agent_in_cell(&session, &cell_id, a))
            .ok_or_else(|| ApiError::not_found("Principal does not belong to this cell"))?;
        (workspace(&session, agent)?, agent.base_commit_sha.clone())
    };
    diff::collect(&root, base.as_deref(), query.agent_id).await.map(Json).map_err(ApiError::internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_requires_an_explicit_principal() {
        assert!(serde_json::from_value::<ReviewQuery>(serde_json::json!({})).is_err());
        assert!(validate_session_id("../escape").is_err());
        assert!(validate_agent_id("../escape").is_err());
        assert!(validate_cell_id("../escape").is_err());
    }

    #[test]
    fn both_workspace_strategies_resolve_the_owning_principal() {
        let now = chrono::Utc::now();
        let mut session = Session {
            id: "session-1".into(), name: None, color: None,
            session_type: SessionType::Hive { worker_count: 1 },
            project_path: PathBuf::from("fixture"),
            state: crate::session::SessionState::Running,
            created_at: now, last_activity_at: now, agents: vec![],
            default_cli: "codex".into(), default_model: None,
            default_principal_cli: None, default_principal_model: None,
            default_principal_flags: vec![],
            execution_policy: crate::domain::HiveExecutionPolicy::default(),
            qa_workers: vec![], max_qa_iterations: 1, qa_timeout_secs: 300,
            qa_inconclusive_at: None, auth_strategy: crate::session::AuthStrategy::None,
            worktree_path: Some("shared-primary".into()), worktree_branch: None,
            no_git: false, resume_report: None,
        };
        let agent: AgentInfo = serde_json::from_value(serde_json::json!({
            "id": "worker-1", "role": {"Worker": {"index": 1, "parent": null}},
            "status": "Running", "config": {}, "parent_id": null
        })).unwrap();
        session.execution_policy.workspace_strategy = WorkspaceStrategy::SharedCell;
        assert_eq!(workspace(&session, &agent).ok().expect("principal workspace"), PathBuf::from("shared-primary"));
        session.execution_policy.workspace_strategy = WorkspaceStrategy::IsolatedCell;
        assert_eq!(workspace(&session, &agent).ok().expect("principal workspace"), PathBuf::from("fixture").join(".hive-manager/worktrees/session-1/worker-1"));
    }
}

