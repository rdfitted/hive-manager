use std::collections::HashSet;

use crate::domain::CellStatus;
use crate::pty::{AgentRole, AgentStatus};

use super::{AgentInfo, Session, SessionState, SessionType};

pub(crate) const PRIMARY_CELL_ID: &str = "primary";
pub(crate) const RESOLVER_CELL_ID: &str = "resolver";
const VARIANT_CELL_PREFIX: &str = "variant:";

pub(crate) fn variant_to_cell_id(variant: &str) -> String {
    let normalized = variant
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>();
    let trimmed = normalized.trim_matches('-');
    let slug = if trimmed.is_empty() { PRIMARY_CELL_ID } else { trimmed };
    format!("{VARIANT_CELL_PREFIX}{slug}")
}

pub(crate) fn session_cell_ids(session: &Session) -> Vec<String> {
    match &session.session_type {
        SessionType::Fusion { variants } if !variants.is_empty() => {
            let mut seen = HashSet::new();
            let mut cell_ids = variants
                .iter()
                .map(|variant| variant_to_cell_id(variant))
                .filter(|cell_id| seen.insert(cell_id.clone()))
                .collect::<Vec<_>>();
            cell_ids.push(RESOLVER_CELL_ID.to_string());
            cell_ids
        }
        _ => vec![PRIMARY_CELL_ID.to_string()],
    }
}

pub(crate) fn agent_in_cell(session: &Session, cell_id: &str, agent: &AgentInfo) -> bool {
    match &session.session_type {
        SessionType::Fusion { .. } if cell_id == RESOLVER_CELL_ID => {
            !matches!(&agent.role, AgentRole::Fusion { .. })
        }
        SessionType::Fusion { .. } if cell_id != PRIMARY_CELL_ID => matches!(
            &agent.role,
            AgentRole::Fusion { variant } if variant_to_cell_id(variant) == cell_id
        ),
        _ => true,
    }
}

pub(crate) fn session_state_to_cell_status(state: &SessionState) -> CellStatus {
    match state {
        SessionState::Planning | SessionState::PlanReady => CellStatus::Queued,
        SessionState::Starting
        | SessionState::SpawningWorker(_)
        | SessionState::SpawningPlanner(_)
        | SessionState::SpawningFusionVariant(_)
        | SessionState::SpawningJudge
        | SessionState::SpawningEvaluator => CellStatus::Launching,
        SessionState::WaitingForWorker(_)
        | SessionState::WaitingForPlanner(_)
        | SessionState::WaitingForFusionVariants
        | SessionState::Judging
        | SessionState::MergingWinner
        | SessionState::QaInProgress { .. }
        | SessionState::Running => CellStatus::Running,
        SessionState::AwaitingVerdictSelection | SessionState::Paused => CellStatus::WaitingInput,
        SessionState::QaPassed | SessionState::Completed | SessionState::Closed => CellStatus::Completed,
        SessionState::QaFailed { .. } | SessionState::QaMaxRetriesExceeded | SessionState::Failed(_) => {
            CellStatus::Failed
        }
        SessionState::Closing => CellStatus::Summarizing,
    }
}

pub(crate) fn derive_cell_status_name(session: &Session, cell_id: &str) -> String {
    match aggregate_cell_status(session, cell_id) {
        CellStatus::Queued => "queued",
        CellStatus::Preparing => "preparing",
        CellStatus::Launching => "launching",
        CellStatus::Running => "running",
        CellStatus::Summarizing => "summarizing",
        CellStatus::Completed => "completed",
        CellStatus::WaitingInput => "waiting_input",
        CellStatus::Failed => "failed",
        CellStatus::Killed => "killed",
    }
    .to_string()
}

pub(crate) fn aggregate_cell_status(session: &Session, cell_id: &str) -> CellStatus {
    if matches!(session.state, SessionState::Closing) {
        return CellStatus::Summarizing;
    }

    if is_terminal_session_state(&session.state) {
        return session_state_to_cell_status(&session.state);
    }

    let agent_statuses = session
        .agents
        .iter()
        .filter(|agent| agent_in_cell(session, cell_id, agent))
        .map(|agent| agent_status_to_cell_status(&agent.status))
        .collect::<Vec<_>>();

    if agent_statuses.iter().any(|status| *status == CellStatus::Failed) {
        CellStatus::Failed
    } else if agent_statuses
        .iter()
        .any(|status| *status == CellStatus::WaitingInput)
    {
        CellStatus::WaitingInput
    } else if agent_statuses.iter().any(|status| *status == CellStatus::Launching) {
        CellStatus::Launching
    } else if agent_statuses.iter().any(|status| *status == CellStatus::Running) {
        CellStatus::Running
    } else if !agent_statuses.is_empty()
        && agent_statuses.iter().all(|status| *status == CellStatus::Completed)
    {
        CellStatus::Completed
    } else {
        session_state_to_cell_status(&session.state)
    }
}

fn is_terminal_session_state(state: &SessionState) -> bool {
    matches!(
        state,
        SessionState::QaPassed
            | SessionState::Completed
            | SessionState::Closed
            | SessionState::QaFailed { .. }
            | SessionState::QaMaxRetriesExceeded
            | SessionState::Failed(_)
    )
}

fn agent_status_to_cell_status(status: &AgentStatus) -> CellStatus {
    match status {
        AgentStatus::Starting => CellStatus::Launching,
        AgentStatus::Running | AgentStatus::Idle => CellStatus::Running,
        AgentStatus::WaitingForInput(_) => CellStatus::WaitingInput,
        AgentStatus::Completed => CellStatus::Completed,
        AgentStatus::Error(_) => CellStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::Utc;

    use crate::pty::{AgentConfig, AgentRole};
    use crate::session::AuthStrategy;

    use super::*;

    fn test_session(state: SessionState, agent_statuses: Vec<AgentStatus>) -> Session {
        Session {
            id: "session-1".to_string(),
            name: None,
            color: None,
            session_type: SessionType::Fusion {
                variants: vec!["Alpha".to_string()],
            },
            project_path: PathBuf::from("."),
            state,
            created_at: Utc::now(),
            agents: agent_statuses
                .into_iter()
                .enumerate()
                .map(|(idx, status)| AgentInfo {
                    id: format!("agent-{idx}"),
                    role: AgentRole::Fusion {
                        variant: "Alpha".to_string(),
                    },
                    status,
                    config: AgentConfig::default(),
                    parent_id: None,
                })
                .collect(),
            default_cli: "claude".to_string(),
            default_model: None,
            max_qa_iterations: 3,
            qa_timeout_secs: 300,
            auth_strategy: AuthStrategy::None,
        }
    }

    #[test]
    fn variant_cell_ids_are_namespaced() {
        assert_eq!(variant_to_cell_id("Resolver"), "variant:resolver");
        assert_eq!(variant_to_cell_id("A/B"), "variant:a-b");
    }

    #[test]
    fn session_cell_ids_dedupe_normalized_variants() {
        let session = Session {
            session_type: SessionType::Fusion {
                variants: vec!["A/B".to_string(), "A B".to_string()],
            },
            ..test_session(SessionState::Running, vec![])
        };

        assert_eq!(
            session_cell_ids(&session),
            vec!["variant:a-b".to_string(), RESOLVER_CELL_ID.to_string()]
        );
    }

    #[test]
    fn terminal_session_state_overrides_running_agent_status() {
        let session = test_session(SessionState::Completed, vec![AgentStatus::Running]);

        assert_eq!(aggregate_cell_status(&session, "variant:alpha"), CellStatus::Completed);
    }

    #[test]
    fn launching_agents_win_before_running_agents() {
        let session = test_session(
            SessionState::Running,
            vec![AgentStatus::Starting, AgentStatus::Running],
        );

        assert_eq!(aggregate_cell_status(&session, "variant:alpha"), CellStatus::Launching);
    }

    #[test]
    fn closing_session_overrides_running_agent_status() {
        let session = test_session(SessionState::Closing, vec![AgentStatus::Running]);

        assert_eq!(
            aggregate_cell_status(&session, "variant:alpha"),
            CellStatus::Summarizing
        );
    }
}
