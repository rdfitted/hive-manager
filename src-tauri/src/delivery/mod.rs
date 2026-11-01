//! Delivery evidence is independent of terminal activity.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use std::path::{Path, PathBuf};

pub mod codex_rollout;
pub mod claude_transcript;

/// Hive's receipt marker is a whole line, never a substring of operator text.
pub(crate) fn is_nonce_line(line: &str) -> bool {
    line.strip_prefix("[hive-msg:")
        .and_then(|value| value.strip_suffix(']'))
        .is_some_and(|hex| hex.len() == 8 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub(crate) fn user_message_payload(text: &str) -> serde_json::Value {
    let mut nonce = None;
    let visible: Vec<_> = text.lines().filter(|line| {
        if is_nonce_line(line) {
            nonce.get_or_insert(*line);
            false
        } else {
            true
        }
    }).collect();
    match nonce {
        Some(nonce) => serde_json::json!({"text": visible.join("\n"), "nonce": nonce}),
        None => serde_json::json!({"text": text}),
    }
}

/// Only human prompts are messages; task notifications use the same envelope.
pub(crate) fn queued_prompt(record: &serde_json::Value) -> Option<(&str, &str)> {
    let attachment = &record["attachment"];
    if record["type"] != "attachment" || record["isCompactSummary"] == true
        || attachment["type"] != "queued_command" || attachment["commandMode"] != "prompt"
        || attachment["origin"]["kind"] != "human" {
        return None;
    }
    Some((attachment["delivery_id"].as_str().filter(|id| !id.is_empty())?,
        attachment["prompt"].as_str()?))
}

pub const TRANSCRIPT_CONFIRMED: &str = "transcript-confirmed";
pub const TRANSCRIPT_PENDING: &str = "transcript-pending";
/// Pending while the receiver is mid-turn and the message was handed to its queue key;
/// delivery is expected when that turn ends, not within the synchronous window.
pub const TRANSCRIPT_PENDING_BUSY: &str = "transcript-pending-busy";
pub const TRANSCRIPT_UNAVAILABLE: &str = "transcript-unavailable";
pub const TRANSCRIPT_TIMEOUT: &str = "transcript-timeout";
pub const PROTOCOL_TURN_STARTED: &str = "protocol-turn-started";
pub const PROTOCOL_PENDING: &str = "protocol-pending";

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

    pub fn pending_busy() -> Self {
        Self { confirmed: None, basis: TRANSCRIPT_PENDING_BUSY.into(), source: "transcript".into(), turn_id: None }
    }

    /// Both pending bases keep the background watch and the flush-inheritable nonce alive.
    pub fn is_pending(&self) -> bool {
        self.basis == TRANSCRIPT_PENDING || self.basis == TRANSCRIPT_PENDING_BUSY || self.basis == PROTOCOL_PENDING
    }

    pub fn unavailable() -> Self {
        Self { confirmed: None, basis: TRANSCRIPT_UNAVAILABLE.into(), source: "none".into(), turn_id: None }
    }

    pub fn protocol_pending() -> Self {
        Self { confirmed: None, basis: PROTOCOL_PENDING.into(), source: "protocol".into(), turn_id: None }
    }

    pub fn protocol_turn_started(turn_id: String) -> Self {
        Self { confirmed: Some(true), basis: PROTOCOL_TURN_STARTED.into(), source: "protocol".into(), turn_id: Some(turn_id) }
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
    Protocol(ProtocolConfirmation),
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
            Self::Protocol(reader) => reader.delivery().confirmed == Some(true),
            Self::Unavailable => false,
        }
    }
}

/// A single protocol send, correlated after the response supplies its turn id.
#[derive(Clone)]
pub struct ProtocolConfirmation {
    thread_id: String,
    turn_id: String,
    started: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl ProtocolConfirmation {
    pub fn new(thread_id: String, turn_id: String) -> Self {
        Self { thread_id, turn_id, started: Default::default() }
    }
    pub fn observe_started(&self, thread_id: &str, turn_id: &str) -> bool {
        let matches = !self.turn_id.is_empty() && self.thread_id == thread_id && self.turn_id == turn_id;
        if matches { self.started.store(true, std::sync::atomic::Ordering::Release); }
        matches
    }
    pub fn delivery(&self) -> Delivery {
        if self.started.load(std::sync::atomic::Ordering::Acquire) {
            Delivery::protocol_turn_started(self.turn_id.clone())
        } else { Delivery::protocol_pending() }
    }
}

/// Poll the agent's own transcript evidence, never terminal output.
pub fn confirm(agent: &mut Confirmation, nonce: &str, since: SystemTime) -> Delivery {
    let turn = match agent {
        Confirmation::Codex(reader) => reader.poll(nonce, since),
        Confirmation::Claude(reader) => reader.poll(nonce, since),
        Confirmation::Protocol(reader) => return reader.delivery(),
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
        assert!(Delivery::pending().is_pending() && Delivery::pending_busy().is_pending());
        assert_eq!(Delivery::pending_busy().confirmed, None);
        assert!(!Delivery::timeout(true).is_pending() && !Delivery::unavailable().is_pending());
    }
}
