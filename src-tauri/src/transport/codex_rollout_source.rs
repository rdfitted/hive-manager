//! Normalizes one already-attributed Codex rollout. Discovery never occurs here.
use super::{feed, AgentEvent, AgentEventKind as Kind};
use crate::delivery::codex_rollout::read_rollout_window;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs::File,
    hash::{Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Default)]
struct Window {
    offset: u64,
    partial: Vec<u8>,
    discard: bool,
}

pub struct CodexRolloutSource {
    root: PathBuf,
    path: PathBuf,
    agent_id: String,
    window: Window,
    created: Option<SystemTime>,
    prefix: Vec<u8>,
    records: HashSet<u64>,
}

/// The two known injected prefixes; ordinary operator Markdown and XML survive.
pub(crate) fn injected_user(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("<environment_context") || text.starts_with("# AGENTS.md instructions")
}

fn message_text(payload: &Value) -> String {
    payload["content"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    matches!(
                        item["type"].as_str(),
                        Some("input_text" | "output_text" | "text")
                    )
                })
                .filter_map(|item| item["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

impl CodexRolloutSource {
    pub fn new(root: PathBuf, path: PathBuf, agent_id: &str) -> Result<Self, String> {
        feed::contained_file(&root, &path)?;
        File::open(&path).map_err(|error| error.to_string())?;
        Ok(Self {
            root,
            path,
            agent_id: agent_id.into(),
            window: Window::default(),
            created: None,
            prefix: Vec::new(),
            records: HashSet::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn offset(&self) -> u64 {
        self.window.offset
    }
    pub fn snapshot_len(&self) -> Result<u64, String> {
        feed::contained_file(&self.root, &self.path)?;
        File::open(&self.path)
            .and_then(|file| file.metadata())
            .map(|meta| meta.len())
            .map_err(|error| error.to_string())
    }

    pub fn poll(&mut self) -> Result<(Vec<AgentEvent>, bool, bool), String> {
        feed::contained_file(&self.root, &self.path)?;
        let mut file = File::open(&self.path).map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        let created = metadata.created().ok();
        let mut prefix = Vec::new();
        (&mut file)
            .take(if self.prefix.is_empty() {
                64
            } else {
                self.prefix.len() as u64
            })
            .read_to_end(&mut prefix)
            .map_err(|error| error.to_string())?;
        let reset = metadata.len() < self.window.offset
            || (self.created.is_some() && created != self.created)
            || (!self.prefix.is_empty() && prefix != self.prefix);
        if reset {
            self.window = Window::default();
            self.records.clear();
        }
        let records = read_rollout_window(
            &mut file,
            &mut self.window.offset,
            &mut self.window.partial,
            &mut self.window.discard,
        )
        .map_err(|error| error.to_string())?;
        self.created = created;
        self.prefix = prefix;
        let events = records
            .iter()
            .flat_map(|record| self.normalize(record))
            .collect();
        Ok((events, self.window.offset < metadata.len(), reset))
    }

    fn event(
        &self,
        record: &Value,
        kind: Kind,
        source_id: Option<String>,
        payload: Value,
    ) -> AgentEvent {
        AgentEvent {
            agent_id: self.agent_id.clone(),
            cursor: String::new(),
            ts: record["timestamp"]
                .as_str()
                .unwrap_or("1970-01-01T00:00:00Z")
                .into(),
            kind,
            source_id,
            truncated: false,
            payload,
        }
        .normalize()
    }

    fn normalize(&mut self, record: &Value) -> Vec<AgentEvent> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        record.to_string().hash(&mut hasher);
        if !self.records.insert(hasher.finish()) {
            return Vec::new();
        }
        let payload = &record["payload"];
        let id = payload["id"].as_str().map(str::to_owned);
        let mut output = Vec::new();
        if record["type"] == "response_item" {
            match payload["type"].as_str() {
                Some("message") => {
                    let text = message_text(payload);
                    match payload["role"].as_str() {
                        Some("user") if !text.is_empty() && !injected_user(&text) => {
                            output.push(self.event(
                                record,
                                Kind::UserMessage,
                                id,
                                crate::delivery::user_message_payload(&text),
                            ))
                        }
                        Some("assistant") if !text.is_empty() => output.push(self.event(
                            record,
                            Kind::AssistantMessage,
                            id,
                            json!({"text":text}),
                        )),
                        _ => {}
                    }
                }
                Some(kind @ ("custom_tool_call" | "function_call")) => {
                    let input = if kind == "custom_tool_call" {
                        payload["input"].clone()
                    } else {
                        payload["arguments"]
                            .as_str()
                            .and_then(|args| serde_json::from_str::<Value>(args).ok())
                            .unwrap_or(Value::Null)
                    };
                    output.push(self.event(
                        record,
                        Kind::ToolStart,
                        id.clone(),
                        json!({"tool_id":payload["call_id"],"name":payload["name"],"input":input}),
                    ));
                    if payload["name"] == "request_user_input_async" {
                        output.push(self.event(
                            record,
                            Kind::Status,
                            id,
                            json!({"status":"needs_input"}),
                        ));
                    }
                }
                Some("custom_tool_call_output" | "function_call_output") => {
                    output.push(self.event(
                        record,
                        Kind::ToolResult,
                        id,
                        json!({"tool_id":payload["call_id"],"result":payload["output"]}),
                    ))
                }
                _ => {}
            }
        } else if record["type"] == "event_msg" {
            match payload["type"].as_str() {
                Some("task_started") => {
                    output.push(self.event(record, Kind::TurnStart, id, json!({})))
                }
                Some("task_complete") => {
                    output.push(self.event(record, Kind::TurnEnd, id, json!({})))
                }
                Some("turn_aborted") => {
                    output.push(self.event(record, Kind::TurnEnd, id, json!({"status":"aborted"})))
                }
                Some("item_completed")
                    if payload["item"]["type"] == "CommandExecution"
                        && payload["item"]["status"] == "failed" =>
                {
                    output.push(self.event(
                        record,
                        Kind::Status,
                        id,
                        json!({"status":"tool_failed","exit_code":payload["item"]["exit_code"]}),
                    ))
                }
                _ => {}
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};

    const FIXTURE: &str = include_str!("../delivery/fixtures/codex-feed.jsonl");

    fn source(text: &str) -> (tempfile::TempDir, CodexRolloutSource) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("rollout-fixture.jsonl");
        fs::write(&path, text).unwrap();
        let source = CodexRolloutSource::new(temp.path().into(), path, "worker-a").unwrap();
        (temp, source)
    }

    #[test]
    fn g18_record_mapping_skips_every_noise_kind_and_item_completed_overlap() {
        let (_temp, mut source) = source(FIXTURE);
        let (events, more, reset) = source.poll().unwrap();
        assert!(!more && !reset);
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            [
                Kind::TurnStart,
                Kind::UserMessage,
                Kind::AssistantMessage,
                Kind::ToolStart,
                Kind::ToolResult,
                Kind::ToolStart,
                Kind::Status,
                Kind::ToolResult,
                Kind::Status,
                Kind::UserMessage,
                Kind::TurnEnd,
                Kind::TurnStart,
                Kind::UserMessage,
                Kind::TurnEnd,
            ],
            "G18 one source of truth and no noise events"
        );
        let serialized = serde_json::to_string(&events).unwrap();
        assert!(!serialized.contains("NOISE_"));
        assert!(events.iter().all(|event| event.agent_id == "worker-a"));
        assert_eq!(events[1].source_id.as_deref(), Some("user-one"));
        assert_eq!(
            events[1].payload,
            json!({"text":"# Keep operator markdown","nonce":"[hive-msg:1234abcd]"})
        );
        assert!(events[3].payload["input"].is_string());
        assert!(events[5].payload["input"].is_object());
        assert_eq!(events[6].payload, json!({"status":"needs_input"}));
        assert_eq!(
            events[8].payload,
            json!({"status":"tool_failed","exit_code":7})
        );
        assert!(!events[4]
            .payload
            .as_object()
            .unwrap()
            .contains_key("is_error"));
        assert_eq!(events[13].payload, json!({"status":"aborted"}));
        assert!(source.poll().unwrap().0.is_empty());
        // Repeated bytes do not manufacture another message or failure signal.
        fs::OpenOptions::new()
            .append(true)
            .open(source.path())
            .unwrap()
            .write_all(FIXTURE.as_bytes())
            .unwrap();
        assert!(source.poll().unwrap().0.is_empty());
    }

    #[test]
    fn g18_partial_utf8_oversized_lines_and_windows_are_bounded() {
        let record = format!(
            "{}\n",
            json!({"type":"response_item","payload":{"type":"message","role":"assistant",
            "id":"utf8","content":[{"type":"output_text","text":"fixture 🦀"}]}})
        );
        let split = record.find('🦀').unwrap() + 2;
        let (_temp, mut source) = source("");
        fs::write(source.path(), &record.as_bytes()[..split]).unwrap();
        assert!(source.poll().unwrap().0.is_empty());
        fs::OpenOptions::new()
            .append(true)
            .open(source.path())
            .unwrap()
            .write_all(&record.as_bytes()[split..])
            .unwrap();
        assert_eq!(source.poll().unwrap().0[0].payload["text"], "fixture 🦀");
        let text = format!(
            "{}\n{record}",
            "x".repeat(feed::MAX_READ_BYTES as usize + 1)
        );
        fs::write(source.path(), text).unwrap();
        let (events, more, reset) = source.poll().unwrap();
        assert!(events.is_empty() && more && reset);
        assert_eq!(source.offset(), feed::MAX_READ_BYTES);
        assert!(source.window.partial.len() <= feed::MAX_READ_BYTES as usize);
        let (events, more, _) = source.poll().unwrap();
        assert!(!more);
        assert_eq!(events.len(), 1);
        fs::remove_file(source.path()).unwrap();
        let offset = source.offset();
        assert!(source.poll().is_err());
        assert_eq!(
            source.offset(),
            offset,
            "unreadable source never advances a cursor"
        );
    }
}
