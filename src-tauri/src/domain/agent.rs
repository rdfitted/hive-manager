use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Agent {
    pub id: String,
    pub cell_id: String,
    pub role: AgentRole,
    pub label: String,
    pub cli: String,
    pub model: Option<String>,
    pub status: AgentStatus,
    pub process_ref: Option<String>,
    pub terminal_ref: Option<String>,
    pub last_event_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Queen,
    Worker,
    Resolver,
    Reviewer,
    Tester,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Queued,
    Launching,
    Running,
    Completed,
    WaitingInput,
    Failed,
    Killed,
}
