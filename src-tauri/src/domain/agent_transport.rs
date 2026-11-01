//! Explicit per-launch opt-ins. An empty set is the legacy PTY path.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTransportKind {
    CodexAppServer,
    ClaudeStreamJson,
}

pub type AgentTransports = BTreeSet<AgentTransportKind>;

pub fn selected_transport(
    enabled: &AgentTransports,
    cli: &str,
    role: &crate::pty::AgentRole,
) -> Option<AgentTransportKind> {
    // In particular, Queen and MasterPlanner never acquire a protocol transport.
    if matches!(role, crate::pty::AgentRole::Queen | crate::pty::AgentRole::MasterPlanner)
        || !matches!(role, crate::pty::AgentRole::Worker { .. })
    {
        return None;
    }
    let stem = std::path::Path::new(cli).file_stem()?.to_str()?;
    let kind = if stem.eq_ignore_ascii_case("codex") {
        AgentTransportKind::CodexAppServer
    } else if stem.eq_ignore_ascii_case("claude") {
        AgentTransportKind::ClaudeStreamJson
    } else {
        return None;
    };
    enabled.contains(&kind).then_some(kind)
}
