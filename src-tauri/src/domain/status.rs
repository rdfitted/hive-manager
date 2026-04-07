pub use crate::domain::agent::AgentStatus;
pub use crate::domain::cell::CellStatus;
pub use crate::domain::session::SessionStatus;

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;
    use serde::Serialize;

    use crate::domain::agent::{AgentRole, AgentStatus};
    use crate::domain::cell::{CellStatus, CellType};
    use crate::domain::event::{EventType, Severity};
    use crate::domain::session::{SessionMode, SessionStatus};
    use crate::domain::workspace::WorkspaceStrategy;

    fn assert_enum_round_trip<T>(value: T, expected_json: &str)
    where
        T: Serialize + DeserializeOwned + std::fmt::Debug + PartialEq,
    {
        let json = serde_json::to_string(&value).expect("serialize enum");
        assert_eq!(json, expected_json);

        let decoded: T = serde_json::from_str(&json).expect("deserialize enum");
        assert_eq!(decoded, value);
    }

    #[test]
    fn session_mode_round_trip() {
        assert_enum_round_trip(SessionMode::Hive, "\"hive\"");
        assert_enum_round_trip(SessionMode::Fusion, "\"fusion\"");
    }

    #[test]
    fn session_status_round_trip() {
        assert_enum_round_trip(SessionStatus::Drafting, "\"drafting\"");
        assert_enum_round_trip(SessionStatus::Preparing, "\"preparing\"");
        assert_enum_round_trip(SessionStatus::Launching, "\"launching\"");
        assert_enum_round_trip(SessionStatus::Active, "\"active\"");
        assert_enum_round_trip(SessionStatus::Resolving, "\"resolving\"");
        assert_enum_round_trip(SessionStatus::Completed, "\"completed\"");
        assert_enum_round_trip(SessionStatus::PartialFailure, "\"partial_failure\"");
        assert_enum_round_trip(SessionStatus::Failed, "\"failed\"");
        assert_enum_round_trip(SessionStatus::Cancelled, "\"cancelled\"");
    }

    #[test]
    fn cell_type_round_trip() {
        assert_enum_round_trip(CellType::Hive, "\"hive\"");
        assert_enum_round_trip(CellType::Resolver, "\"resolver\"");
    }

    #[test]
    fn cell_status_round_trip() {
        assert_enum_round_trip(CellStatus::Queued, "\"queued\"");
        assert_enum_round_trip(CellStatus::Preparing, "\"preparing\"");
        assert_enum_round_trip(CellStatus::Launching, "\"launching\"");
        assert_enum_round_trip(CellStatus::Running, "\"running\"");
        assert_enum_round_trip(CellStatus::Summarizing, "\"summarizing\"");
        assert_enum_round_trip(CellStatus::Completed, "\"completed\"");
        assert_enum_round_trip(CellStatus::WaitingInput, "\"waiting_input\"");
        assert_enum_round_trip(CellStatus::Failed, "\"failed\"");
        assert_enum_round_trip(CellStatus::Killed, "\"killed\"");
    }

    #[test]
    fn agent_role_round_trip() {
        assert_enum_round_trip(AgentRole::Queen, "\"queen\"");
        assert_enum_round_trip(AgentRole::Worker, "\"worker\"");
        assert_enum_round_trip(AgentRole::Resolver, "\"resolver\"");
        assert_enum_round_trip(AgentRole::Reviewer, "\"reviewer\"");
        assert_enum_round_trip(AgentRole::Tester, "\"tester\"");
    }

    #[test]
    fn agent_status_round_trip() {
        assert_enum_round_trip(AgentStatus::Queued, "\"queued\"");
        assert_enum_round_trip(AgentStatus::Launching, "\"launching\"");
        assert_enum_round_trip(AgentStatus::Running, "\"running\"");
        assert_enum_round_trip(AgentStatus::Completed, "\"completed\"");
        assert_enum_round_trip(AgentStatus::WaitingInput, "\"waiting_input\"");
        assert_enum_round_trip(AgentStatus::Failed, "\"failed\"");
        assert_enum_round_trip(AgentStatus::Killed, "\"killed\"");
    }

    #[test]
    fn workspace_strategy_round_trip() {
        assert_enum_round_trip(WorkspaceStrategy::SharedCell, "\"shared_cell\"");
        assert_enum_round_trip(WorkspaceStrategy::IsolatedCell, "\"isolated_cell\"");
    }

    #[test]
    fn event_type_round_trip() {
        assert_enum_round_trip(EventType::SessionCreated, "\"session_created\"");
        assert_enum_round_trip(EventType::SessionStatusChanged, "\"session_status_changed\"");
        assert_enum_round_trip(EventType::CellCreated, "\"cell_created\"");
        assert_enum_round_trip(EventType::CellStatusChanged, "\"cell_status_changed\"");
        assert_enum_round_trip(EventType::WorkspaceCreated, "\"workspace_created\"");
        assert_enum_round_trip(EventType::AgentLaunched, "\"agent_launched\"");
        assert_enum_round_trip(EventType::AgentCompleted, "\"agent_completed\"");
        assert_enum_round_trip(EventType::AgentWaitingInput, "\"agent_waiting_input\"");
        assert_enum_round_trip(EventType::AgentFailed, "\"agent_failed\"");
        assert_enum_round_trip(EventType::ArtifactUpdated, "\"artifact_updated\"");
        assert_enum_round_trip(
            EventType::ResolverSelectedCandidate,
            "\"resolver_selected_candidate\"",
        );
    }

    #[test]
    fn severity_round_trip() {
        assert_enum_round_trip(Severity::Info, "\"info\"");
        assert_enum_round_trip(Severity::Warning, "\"warning\"");
        assert_enum_round_trip(Severity::Error, "\"error\"");
    }
}
