//! Durable, node-addressed completion facts for work-graph projection.
//!
//! The effect ledger in the run journal records side effects of execution steps. This
//! separate JSONL ledger records the first-class fact that a specific work-graph node was
//! completed by a resolved agent identity.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::domain::event::{Event, EventType, Severity};
use crate::http::handlers::workers::ExecutedAs;

use super::TaskId;

pub const NODE_COMPLETION_LEDGER: &str = "state/work-graph-completions.jsonl";

static APPEND_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeCompletionProvenance {
    QueueFinalize,
    Heartbeat,
    EvaluatorVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeCompletionFact {
    pub id: String,
    pub task_id: TaskId,
    pub agent_id: String,
    pub provenance: NodeCompletionProvenance,
    pub completed_at: DateTime<Utc>,
    #[serde(default)]
    pub(crate) executed_as: Option<ExecutedAs>,
}

impl NodeCompletionFact {
    pub fn new(
        task_id: impl Into<TaskId>,
        agent_id: impl Into<String>,
        provenance: NodeCompletionProvenance,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: task_id.into(),
            agent_id: agent_id.into(),
            provenance,
            completed_at: Utc::now(),
            executed_as: None,
        }
    }

    pub(crate) fn with_executed_as(mut self, executed_as: ExecutedAs) -> Self {
        self.executed_as = Some(executed_as);
        self
    }

    pub fn source_ref(&self) -> String {
        format!("completion-fact:{}", self.id)
    }

    pub fn event(&self, session_id: &str) -> Event {
        let mut payload = serde_json::json!({
            "task_id": self.task_id,
            "fact_id": self.id,
            "provenance": self.provenance,
        });
        if let Some(executed_as) = &self.executed_as {
            payload["executed_as"] = serde_json::json!(executed_as);
        }
        Event {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            cell_id: None,
            agent_id: Some(self.agent_id.clone()),
            event_type: EventType::WorkNodeCompleted,
            timestamp: self.completed_at,
            payload,
            severity: Severity::Info,
        }
    }
}

pub fn ledger_path(session_dir: &Path) -> PathBuf {
    session_dir.join(NODE_COMPLETION_LEDGER)
}

/// Append facts without ever exposing a partially written JSONL file. The existing bytes and
/// new records are written to a sibling temporary file, then atomically persisted over the old
/// path. Existing records are preserved byte-for-byte.
pub fn append_node_completion_facts(
    session_dir: &Path,
    facts: &[NodeCompletionFact],
) -> Result<(), std::io::Error> {
    if facts.is_empty() {
        return Ok(());
    }
    let _guard = APPEND_LOCK
        .lock()
        .map_err(|_| std::io::Error::other("completion-ledger append lock was poisoned"))?;
    let path = ledger_path(session_dir);
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("completion ledger has no parent directory"))?;
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
    for fact in facts {
        serde_json::to_writer(&mut temp, fact).map_err(std::io::Error::other)?;
        temp.write_all(b"\n")?;
    }
    temp.as_file().sync_all()?;
    temp.persist(&path).map_err(|error| error.error)?;
    Ok(())
}

pub fn read_node_completion_facts(
    session_dir: &Path,
) -> Result<Vec<NodeCompletionFact>, std::io::Error> {
    let path = ledger_path(session_dir);
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    BufReader::new(file)
        .lines()
        .enumerate()
        .filter_map(|(index, line)| match line {
            Ok(line) if line.trim().is_empty() => None,
            other => Some((index, other)),
        })
        .map(|(index, line)| {
            let line = line?;
            serde_json::from_str(&line).map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid completion fact at line {}: {error}", index + 1),
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn atomic_append_preserves_prior_completion_facts() {
        let temp = TempDir::new().unwrap();
        let first = NodeCompletionFact::new(
            "T1",
            "session-worker-1",
            NodeCompletionProvenance::Heartbeat,
        );
        let second = NodeCompletionFact::new(
            "T2",
            "session-worker-2",
            NodeCompletionProvenance::QueueFinalize,
        );

        append_node_completion_facts(temp.path(), std::slice::from_ref(&first)).unwrap();
        append_node_completion_facts(temp.path(), std::slice::from_ref(&second)).unwrap();

        assert_eq!(
            read_node_completion_facts(temp.path()).unwrap(),
            vec![first, second]
        );
    }

    #[test]
    fn legacy_jsonl_without_executed_as_deserializes_as_none() {
        let temp = TempDir::new().unwrap();
        let path = ledger_path(temp.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            r#"{"id":"legacy-fact","task_id":"T1","agent_id":"session-worker-1","provenance":"heartbeat","completed_at":"2026-01-02T03:04:05Z"}
"#,
        )
        .unwrap();

        let facts = read_node_completion_facts(temp.path()).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].id, "legacy-fact");
        assert!(facts[0].executed_as.is_none());
    }

    #[test]
    fn completion_event_conditionally_includes_executed_as() {
        let legacy = NodeCompletionFact::new(
            "T1",
            "session-worker-1",
            NodeCompletionProvenance::Heartbeat,
        );
        let legacy_event = legacy.event("session");
        assert_eq!(legacy_event.payload.as_object().unwrap().len(), 3);
        assert!(legacy_event.payload.get("executed_as").is_none());

        let executed_as_json = serde_json::json!({
            "provider": "codex",
            "tier": "high",
            "model": "gpt-5.6-sol",
            "flags": ["-c", "model_reasoning_effort=\"high\""],
            "channel": "native",
            "source": "node"
        });
        let executed_as = serde_json::from_value(executed_as_json.clone()).unwrap();
        let detailed = NodeCompletionFact::new(
            "T2",
            "session-worker-1",
            NodeCompletionProvenance::Heartbeat,
        )
        .with_executed_as(executed_as);

        assert_eq!(
            detailed.event("session").payload["executed_as"],
            executed_as_json
        );
    }
}
