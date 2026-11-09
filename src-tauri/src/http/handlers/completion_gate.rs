//! Completion gate: what the app checks itself before recording an agent as completed.
use std::collections::HashMap;
use std::path::Path;

use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::pty::AgentRole;
use crate::session::maintain_enforce::{self, Check, Decision, Outcome, Sender};
use crate::session::maintain_gate::{self, MaintainGateMode};
use crate::session::AgentInfo;

/// Coding workers are gated. Planners, evaluators, QA workers and the Queen change
/// no product code, and a researcher's contract forbids it.
pub(crate) fn applies_to(agent: &AgentInfo) -> bool {
    let researcher = agent.config.role.as_ref()
        .is_some_and(|role| role.role_type.eq_ignore_ascii_case("researcher"));
    matches!(agent.role, AgentRole::Worker { .. }) && !researcher
}

/// True when two paths name the same directory, tolerating separators and case.
fn same_directory(left: &Path, right: &Path) -> bool {
    let normal = |path: &Path| {
        let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        resolved.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
    };
    normal(left) == normal(right)
}

/// Build the check for one agent, or None when this completion is not gated.
fn plan(state: &AppState, session_id: &str, agent_id: &str, mode: MaintainGateMode) -> Option<Check> {
    let (project_path, base) = {
        let controller = state.session_controller.read();
        let session = controller.get_session(session_id)?;
        let agent = session.agents.iter().find(|agent| agent.id == agent_id)?;
        if !applies_to(agent) {
            return None;
        }
        (session.project_path.clone(), agent.base_commit_sha.clone())
    };
    let workspace = state.pty_manager.read().spawn_cwd(agent_id)?;
    // In a workspace shared with other agents the diff is not this agent's alone,
    // and gating a co-tenant's edits would be wrong. Only an isolated cell is gated.
    if same_directory(Path::new(&workspace), Path::new(&project_path)) {
        return None;
    }
    Some(Check {
        workspace: workspace.into(),
        session_root: Path::new(&project_path).join(".hive-manager").join(session_id),
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
        base,
        mode,
    })
}

/// Run the gate for a `completed` heartbeat. Report mode never delays or refuses it.
pub(crate) async fn check(
    state: &AppState, session_id: &str, agent_id: &str, queen_mark: bool,
) -> Result<(), ApiError> {
    let mode = maintain_gate::configured_modes(Some(state.storage.as_ref())).maintain;
    if mode == MaintainGateMode::Off {
        return Ok(());
    }
    let Some(check) = plan(state, session_id, agent_id, mode) else {
        return Ok(());
    };
    if mode == MaintainGateMode::Report {
        // Score it for the record without holding the heartbeat.
        tokio::task::spawn_blocking(move || {
            let outcome = maintain_enforce::run(&check);
            tracing::info!(agent_id = %check.agent_id, ?outcome, "maintainability gate (report)");
        });
        return Ok(());
    }
    let evidence = check.evidence_path();
    let outcome = tokio::task::spawn_blocking(move || maintain_enforce::run(&check)).await
        .unwrap_or_else(|error| Outcome::Unavailable { reason: format!("the checker task failed: {error}") });
    let sender = if queen_mark { Sender::QueenMark } else { Sender::Agent };
    match maintain_enforce::decide(mode, sender, &outcome, &evidence) {
        Decision::Accept => Ok(()),
        Decision::Reject { message } => {
            tracing::warn!(session_id = %session_id, agent_id = %agent_id, ?outcome, "maintainability gate refused a completion");
            let mut details = HashMap::new();
            details.insert("gate".to_string(), serde_json::json!("maintain-check"));
            details.insert("verdict_path".to_string(), serde_json::json!(evidence.to_string_lossy().replace('\\', "/")));
            if let Outcome::Block { findings, score } = &outcome {
                details.insert("findings".to_string(), serde_json::json!(findings));
                details.insert("score".to_string(), serde_json::json!(score));
            }
            Err(ApiError::conflict_with_details(message, details))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_gate_same_directory_ignores_separators_case_and_trailing_slash() {
        assert!(same_directory(Path::new("D:\\Missing\\Project"), Path::new("d:/missing/project/")));
        assert!(!same_directory(Path::new("D:/missing/project"), Path::new("D:/missing/project/worktrees/w1")));
    }
}
