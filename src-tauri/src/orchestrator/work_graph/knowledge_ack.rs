//! Durable knowledge-reference acknowledgements from completed agent heartbeats.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

pub const KNOWLEDGE_ACK_LEDGER: &str = "state/knowledge-acks.jsonl";
pub const KNOWLEDGE_ACK_SCHEMA_VERSION: &str = "hive.knowledge-ack/v1";

static APPEND_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeAckRecord {
    pub schema_version: String,
    pub session_id: String,
    pub agent_id: String,
    pub knowledge_ack: Vec<String>,
    pub recorded_at: DateTime<Utc>,
}

impl KnowledgeAckRecord {
    fn new(session_id: &str, agent_id: &str, knowledge_ack: &[String]) -> Self {
        Self {
            schema_version: KNOWLEDGE_ACK_SCHEMA_VERSION.to_string(),
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            knowledge_ack: knowledge_ack.to_vec(),
            recorded_at: Utc::now(),
        }
    }
}

pub fn knowledge_ack_path(session_dir: &Path) -> PathBuf {
    session_dir.join(KNOWLEDGE_ACK_LEDGER)
}

pub fn is_valid_knowledge_ack_tag(tag: &str) -> bool {
    let bytes = tag.as_bytes();
    bytes.len() >= 2
        && bytes[0] == b'k'
        && matches!(bytes[1], b'1'..=b'9')
        && bytes[2..].iter().all(u8::is_ascii_digit)
}

/// Atomically append one explicit acknowledgement while preserving prior JSONL bytes.
pub fn append_knowledge_ack(
    session_dir: &Path,
    session_id: &str,
    agent_id: &str,
    knowledge_ack: &[String],
) -> Result<(), std::io::Error> {
    let _guard = APPEND_LOCK
        .lock()
        .map_err(|_| std::io::Error::other("knowledge-ack append lock was poisoned"))?;
    let path = knowledge_ack_path(session_dir);
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("knowledge-ack ledger has no parent directory"))?;
    fs::create_dir_all(parent)?;

    let existing = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(&existing)?;
    if !existing.is_empty() && existing.last() != Some(&b'\n') {
        temp.write_all(b"\n")?;
    }
    serde_json::to_writer(
        &mut temp,
        &KnowledgeAckRecord::new(session_id, agent_id, knowledge_ack),
    )
    .map_err(std::io::Error::other)?;
    temp.write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(&path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn validates_only_supported_knowledge_ack_tags() {
        for valid in ["k1", "k9", "k10", "k999"] {
            assert!(is_valid_knowledge_ack_tag(valid), "{valid}");
        }
        for invalid in ["", "k", "k0", "k01", "K1", "k-1", "k1a", " k1"] {
            assert!(!is_valid_knowledge_ack_tag(invalid), "{invalid}");
        }
    }

    #[test]
    fn atomic_append_preserves_explicit_empty_and_non_empty_acknowledgements() {
        let temp = TempDir::new().unwrap();
        append_knowledge_ack(temp.path(), "session", "session-worker-1", &[]).unwrap();
        append_knowledge_ack(
            temp.path(),
            "session",
            "session-worker-1",
            &["k1".to_string(), "k12".to_string()],
        )
        .unwrap();

        let contents = fs::read_to_string(knowledge_ack_path(temp.path())).unwrap();
        let records = contents
            .lines()
            .map(|line| serde_json::from_str::<KnowledgeAckRecord>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert!(records[0].knowledge_ack.is_empty());
        assert_eq!(records[1].knowledge_ack, ["k1", "k12"]);
        assert!(records
            .iter()
            .all(|record| record.schema_version == KNOWLEDGE_ACK_SCHEMA_VERSION));
    }
}
