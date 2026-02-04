mod state;
mod injection;

pub use state::*;
pub use injection::*;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Types of coordination messages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageType {
    Task,
    Progress,
    Completion,
    Error,
    System,
}

/// A coordination message between agents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinationMessage {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub from: String,
    pub to: String,
    pub content: String,
    pub message_type: MessageType,
}

impl CoordinationMessage {
    pub fn new(from: &str, to: &str, content: &str, message_type: MessageType) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            from: from.to_string(),
            to: to.to_string(),
            content: content.to_string(),
            message_type,
        }
    }

    pub fn system(to: &str, content: &str) -> Self {
        Self::new("SYSTEM", to, content, MessageType::System)
    }

    pub fn task(from: &str, to: &str, content: &str) -> Self {
        Self::new(from, to, content, MessageType::Task)
    }

    #[allow(dead_code)]
    pub fn progress(from: &str, content: &str) -> Self {
        Self::new(from, "LOG", content, MessageType::Progress)
    }
}
