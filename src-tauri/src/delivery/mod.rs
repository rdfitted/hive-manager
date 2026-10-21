//! Delivery evidence is independent of terminal activity.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use std::path::{Path, PathBuf};

pub mod codex_rollout;
pub mod claude_transcript;

pub const TRANSCRIPT_CONFIRMED: &str = "transcript-confirmed";
pub const TRANSCRIPT_PENDING: &str = "transcript-pending";
pub const TRANSCRIPT_UNAVAILABLE: &str = "transcript-unavailable";
pub const TRANSCRIPT_TIMEOUT: &str = "transcript-timeout";
#[allow(dead_code)] // Reserved I1 basis for the protocol follow-up.
pub const PROTOCOL_TURN_STARTED: &str = "protocol-turn-started";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Delivery {
    pub confirmed: Option<bool>,
    pub basis: String,
    pub source: String,
    pub turn_id: Option<String>,
}

impl Delivery {
    pub fn pending() -> Self {
        Self { confirmed: None, basis: TRANSCRIPT_PENDING.into(), source: "transcript".into(), turn_id: None }
    }

    pub fn unavailable() -> Self {
        Self { confirmed: None, basis: TRANSCRIPT_UNAVAILABLE.into(), source: "none".into(), turn_id: None }
    }

    pub fn confirmed(turn_id: String) -> Self {
        Self { confirmed: Some(true), basis: TRANSCRIPT_CONFIRMED.into(), source: "transcript".into(), turn_id: Some(turn_id) }
    }

    pub fn timeout(staged_text_visible: bool) -> Self {
        Self { confirmed: staged_text_visible.then_some(false), basis: TRANSCRIPT_TIMEOUT.into(), source: "transcript".into(), turn_id: None }
    }
}

/// A confirmation transaction owns its readers and offsets; transactions never share attribution.
pub enum Confirmation {
    Codex(codex_rollout::CodexRolloutReader),
    Claude(claude_transcript::ClaudeTranscriptReader),
    Unavailable,
}

impl Confirmation {
    pub fn new(cli: &str, session_id: Option<&str>, cwd: &Path, spawned_at: SystemTime,
        codex_root: PathBuf, claude_root: PathBuf) -> Self {
        match cli {
            "codex" => Self::Codex(codex_rollout::CodexRolloutReader::new(codex_root, spawned_at)),
            "claude" => session_id.map(|id| Self::Claude(
                claude_transcript::ClaudeTranscriptReader::new(claude_root, cwd, id)
            )).unwrap_or(Self::Unavailable),
            _ => Self::Unavailable,
        }
    }

    pub fn message_recorded(&self, nonce: &str) -> bool {
        match self {
            Self::Codex(reader) => reader.message_recorded(),
            Self::Claude(reader) => reader.message_recorded(nonce),
            Self::Unavailable => false,
        }
    }
}

/// Poll the agent's own transcript evidence, never terminal output.
pub fn confirm(agent: &mut Confirmation, nonce: &str, since: SystemTime) -> Delivery {
    let turn = match agent {
        Confirmation::Codex(reader) => reader.poll(nonce, since),
        Confirmation::Claude(reader) => reader.poll(nonce, since),
        Confirmation::Unavailable => return Delivery::unavailable(),
    };
    turn.map(Delivery::confirmed).unwrap_or_else(Delivery::pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_cli_is_unknown_and_timeout_needs_visible_staged_text() {
        let mut agent = Confirmation::Unavailable;
        assert_eq!(confirm(&mut agent, "[hive-msg:00000000]", SystemTime::now()), Delivery::unavailable());
        assert_eq!(Delivery::timeout(false).confirmed, None);
        assert_eq!(Delivery::timeout(true).confirmed, Some(false));
        assert_eq!(Delivery::confirmed("turn".into()).source, "transcript");
    }
}
