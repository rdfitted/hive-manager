use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Event {
    pub id: String,
    pub session_id: String,
    pub cell_id: Option<String>,
    pub agent_id: Option<String>,
    pub event_type: EventType,
    pub timestamp: DateTime<Utc>,
    pub payload: serde_json::Value,
    pub severity: Severity,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    SessionCreated,
    SessionStatusChanged,
    CellCreated,
    CellStatusChanged,
    ConversationMessage,
    WorkspaceCreated,
    AgentLaunched,
    AgentCompleted,
    AgentWaitingInput,
    AgentFailed,
    ArtifactUpdated,
    ResolverSelectedCandidate,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}
