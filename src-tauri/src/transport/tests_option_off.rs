use crate::domain::agent_transport::{selected_transport, AgentTransportKind, AgentTransports};
use crate::pty::AgentRole;

#[test]
fn option_off_and_ineligible_seats_stay_on_pty() {
    let worker = AgentRole::Worker { index: 1, parent: None };
    for cli in ["claude", "codex", "cursor"] {
        assert_eq!(selected_transport(&AgentTransports::new(), cli, &worker), None);
    }
    let enabled = [AgentTransportKind::CodexAppServer, AgentTransportKind::ClaudeStreamJson]
        .into_iter().collect();
    for role in [AgentRole::Queen, AgentRole::MasterPlanner] {
        for cli in ["codex", "claude"] {
            assert_eq!(selected_transport(&enabled, cli, &role), None);
        }
    }
    assert_eq!(selected_transport(&enabled, "codex", &worker), Some(AgentTransportKind::CodexAppServer));
    assert_eq!(selected_transport(&enabled, "cursor", &worker), None);
}
