use std::collections::{HashMap, HashSet};

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
    agent_in_cell_with_variant_cache(session, cell_id, agent, None)
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
    derive_cell_status_name_for_state(session, cell_id, &session.state)
}

pub(crate) fn derive_cell_status_name_for_state(
    session: &Session,
    cell_id: &str,
    state: &SessionState,
) -> String {
    match aggregate_cell_status_for_state(session, cell_id, state) {
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

fn is_fusion_scoped_cell(session: &Session, cell_id: &str) -> bool {
    matches!(session.session_type, SessionType::Fusion { .. }) && cell_id != PRIMARY_CELL_ID
}

pub(crate) fn aggregate_cell_status(session: &Session, cell_id: &str) -> CellStatus {
    aggregate_cell_status_for_state(session, cell_id, &session.state)
}

pub(crate) fn aggregate_cell_status_for_state(
    session: &Session,
    cell_id: &str,
    state: &SessionState,
) -> CellStatus {
    if matches!(state, SessionState::Closing) {
        return CellStatus::Summarizing;
    }

    if is_terminal_session_state(state) {
        return session_state_to_cell_status(state);
    }

    let mut variant_cell_cache = HashMap::new();
    let agent_statuses = session
        .agents
        .iter()
        .filter(|agent| {
            agent_in_cell_with_variant_cache(session, cell_id, agent, Some(&mut variant_cell_cache))
        })
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
    } else if agent_statuses.is_empty() && is_fusion_scoped_cell(session, cell_id) {
        CellStatus::Queued
    } else {
        session_state_to_cell_status(state)
    }
}

fn agent_in_cell_with_variant_cache(
    session: &Session,
    cell_id: &str,
    agent: &AgentInfo,
    variant_cell_cache: Option<&mut HashMap<String, String>>,
) -> bool {
    match &session.session_type {
        SessionType::Fusion { .. } if cell_id == RESOLVER_CELL_ID => {
            // Resolver cell contains all NON-Fusion agents (judge, planner, evaluator, QA)
            !matches!(&agent.role, AgentRole::Fusion { .. })
        }
        SessionType::Fusion { .. } if cell_id != PRIMARY_CELL_ID => {
            // Variant cells contain only Fusion agents matching that variant
            fusion_agent_matches_cell(cell_id, agent, variant_cell_cache)
        }
        SessionType::Fusion { .. } => {
            // PRIMARY_CELL_ID is not used in Fusion sessions
            false
        }
        _ => {
            // Non-Fusion sessions: all agents go to primary cell
            cell_id == PRIMARY_CELL_ID
        }
    }
}

fn fusion_agent_matches_cell(
    cell_id: &str,
    agent: &AgentInfo,
    variant_cell_cache: Option<&mut HashMap<String, String>>,
) -> bool {
    let AgentRole::Fusion { variant } = &agent.role else {
        return false;
    };

    if let Some(cache) = variant_cell_cache {
        if let Some(cached_cell_id) = cache.get(variant) {
            cached_cell_id == cell_id
        } else {
            let derived_cell_id = variant_to_cell_id(variant);
            cache.insert(variant.clone(), derived_cell_id.clone());
            derived_cell_id == cell_id
        }
    } else {
        variant_to_cell_id(variant) == cell_id
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

    #[test]
    fn empty_fusion_resolver_cell_stays_queued_while_session_runs() {
        let session = test_session(SessionState::Running, vec![]);

        assert_eq!(aggregate_cell_status(&session, RESOLVER_CELL_ID), CellStatus::Queued);
    }

    #[test]
    fn non_fusion_primary_cell_contains_all_agents() {
        // Non-Fusion sessions: all agents go to primary cell
        let session = Session {
            session_type: SessionType::Hive { worker_count: 1 },
            ..test_session(SessionState::Running, vec![AgentStatus::Running])
        };

        assert!(agent_in_cell(&session, PRIMARY_CELL_ID, &session.agents[0]));
    }

    #[test]
    fn fusion_primary_cell_contains_no_agents() {
        // Fusion sessions do NOT use primary cell
        let session = test_session(SessionState::Running, vec![AgentStatus::Running]);

        assert!(!agent_in_cell(&session, PRIMARY_CELL_ID, &session.agents[0]));
    }

    #[test]
    fn fusion_variant_cell_only_matches_fusion_agents() {
        // Fusion variant cells contain only Fusion agents matching that variant
        let fusion_agent = AgentInfo {
            id: "fusion-agent".to_string(),
            role: AgentRole::Fusion {
                variant: "Alpha".to_string(),
            },
            status: AgentStatus::Running,
            config: AgentConfig::default(),
            parent_id: None,
        };
        let judge_agent = AgentInfo {
            id: "judge-agent".to_string(),
            role: AgentRole::Judge {
                session_id: "session-1".to_string(),
            },
            status: AgentStatus::Running,
            config: AgentConfig::default(),
            parent_id: None,
        };

        let session = Session {
            agents: vec![fusion_agent.clone(), judge_agent.clone()],
            ..test_session(SessionState::Running, vec![])
        };

        // Fusion agent matches variant:alpha
        assert!(agent_in_cell(&session, "variant:alpha", &fusion_agent));
        // Judge agent does NOT match variant:alpha
        assert!(!agent_in_cell(&session, "variant:alpha", &judge_agent));
    }

    #[test]
    fn fusion_resolver_cell_contains_non_fusion_agents() {
        // Resolver cell contains non-Fusion agents (judge, planner, evaluator, QA)
        let fusion_agent = AgentInfo {
            id: "fusion-agent".to_string(),
            role: AgentRole::Fusion {
                variant: "Alpha".to_string(),
            },
            status: AgentStatus::Running,
            config: AgentConfig::default(),
            parent_id: None,
        };
        let judge_agent = AgentInfo {
            id: "judge-agent".to_string(),
            role: AgentRole::Judge {
                session_id: "session-1".to_string(),
            },
            status: AgentStatus::Running,
            config: AgentConfig::default(),
            parent_id: None,
        };

        let session = Session {
            agents: vec![fusion_agent.clone(), judge_agent.clone()],
            ..test_session(SessionState::Running, vec![])
        };

        // Fusion agent does NOT go to resolver
        assert!(!agent_in_cell(&session, RESOLVER_CELL_ID, &fusion_agent));
        // Judge agent goes to resolver
        assert!(agent_in_cell(&session, RESOLVER_CELL_ID, &judge_agent));
    }
}
