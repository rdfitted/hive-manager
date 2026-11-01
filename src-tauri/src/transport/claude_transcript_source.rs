//! Reads only the main transcript named by Hive's held Claude session id.
use super::feed::MAX_READ_BYTES;
use super::{AgentEvent, AgentEventKind as Kind, MAX_TEXT_BYTES};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    hash::{Hash, Hasher},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub struct ClaudeTranscriptSource {
    root: PathBuf,
    path: PathBuf,
    agent_id: String,
    offset: u64,
    partial: Vec<u8>,
    discard: bool,
    messages: HashMap<String, String>,
    records: HashSet<u64>,
    deliveries: HashSet<String>,
    created: Option<SystemTime>,
    prefix: Vec<u8>,
}

impl ClaudeTranscriptSource {
    pub fn new(
        root: PathBuf,
        cwd: &Path,
        session_id: &str,
        agent_id: &str,
    ) -> Result<Self, String> {
        super::feed::safe_component(session_id)?;
        let slug: String = cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        let path = root.join(slug).join(format!("{session_id}.jsonl"));
        Ok(Self {
            root,
            path,
            agent_id: agent_id.into(),
            offset: 0,
            partial: Vec::new(),
            discard: false,
            messages: HashMap::new(),
            records: HashSet::new(),
            deliveries: HashSet::new(),
            created: None,
            prefix: Vec::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn offset(&self) -> u64 {
        self.offset
    }
    pub fn snapshot_len(&self) -> Result<u64, String> {
        if !self.path.exists() {
            return Ok(0);
        }
        super::feed::contained_file(&self.root, &self.path)?;
        std::fs::metadata(&self.path)
            .map(|m| m.len())
            .map_err(|e| e.to_string())
    }

    /// A window is at most 2 MiB, including a possibly hostile incomplete line.
    /// No directory enumeration, fallback identity matching or subagent traversal.
    pub fn poll(&mut self) -> Result<(Vec<AgentEvent>, bool, bool), String> {
        if !self.path.exists() {
            return Ok((Vec::new(), false, false));
        }
        super::feed::contained_file(&self.root, &self.path)?;
        let mut file = File::open(&self.path).map_err(|e| e.to_string())?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        let size = metadata.len();
        let created = metadata.created().ok();
        let mut prefix = Vec::new();
        (&mut file)
            .take(if self.prefix.is_empty() {
                64
            } else {
                self.prefix.len() as u64
            })
            .read_to_end(&mut prefix)
            .map_err(|e| e.to_string())?;
        let reset = size < self.offset
            || (self.created.is_some() && created != self.created)
            || (!self.prefix.is_empty() && prefix != self.prefix);
        if reset {
            self.offset = 0;
            self.partial.clear();
            self.discard = false;
            self.messages.clear();
            self.records.clear();
            self.deliveries.clear();
        }
        self.created = created;
        self.prefix = prefix;
        file.seek(SeekFrom::Start(self.offset))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(MAX_READ_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        self.offset += bytes.len() as u64;
        let more = self.offset < size;
        let mut events = Vec::new();
        for part in bytes.split_inclusive(|b| *b == b'\n') {
            let complete = part.last() == Some(&b'\n');
            if !self.discard {
                if self.partial.len() + part.len() > MAX_READ_BYTES as usize {
                    self.partial.clear();
                    self.discard = true;
                } else {
                    self.partial.extend_from_slice(part);
                }
            }
            if complete {
                if !self.discard {
                    if let Ok(record) = serde_json::from_slice::<Value>(&self.partial) {
                        events.extend(self.normalize(&record));
                    }
                }
                self.partial.clear();
                self.discard = false;
            }
        }
        Ok((events, more, reset))
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
        let record_id = record["uuid"].as_str().map(str::to_owned);
        let mut output = Vec::new();
        let typ = record["type"].as_str().unwrap_or("");
        if record["isCompactSummary"] == true {
            return output;
        }
        if typ == "system" && record["subtype"] == "turn_duration" {
            output.push(self.event(record, Kind::TurnEnd, record_id, json!({})));
            return output;
        }
        let queued = crate::delivery::queued_prompt(record);
        if typ != "user" && typ != "assistant" && queued.is_none() {
            return output;
        }
        // Dedupe a replayed record, never legitimate equal text in distinct records.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        record.to_string().hash(&mut hasher);
        let fingerprint = hasher.finish();
        if self.records.contains(&fingerprint) {
            return output;
        }
        if self.records.len() >= 4096 {
            self.records.clear();
        }
        self.records.insert(fingerprint);
        if let Some((delivery_id, text)) = queued {
            if !self.deliveries.insert(delivery_id.into()) {
                return output;
            }
            output.push(self.event(record, Kind::UserMessage, Some(delivery_id.into()),
                crate::delivery::user_message_payload(text)));
            return output;
        }
        let message = &record["message"];
        if message["role"].as_str() != Some(typ) {
            return output;
        }
        let message_id = message["id"]
            .as_str()
            .map(str::to_owned)
            .or(record_id.clone());
        let content = &message["content"];
        let mut text = String::new();
        if let Some(value) = content.as_str() {
            text.push_str(value);
        }
        if let Some(blocks) = content.as_array() {
            for (index, block) in blocks.iter().enumerate() {
                let block_id = block["id"]
                    .as_str()
                    .or_else(|| block["tool_use_id"].as_str())
                    .map(str::to_owned)
                    .or_else(|| record_id.as_ref().map(|id| format!("{id}:{index}")));
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(value) = block["text"].as_str() { text.push_str(value); }
                    }
                    Some("tool_use") if typ == "assistant" => output.push(self.event(record, Kind::ToolStart,
                        block_id, json!({"tool_id": block["id"], "name": block["name"], "input": block["input"]}))),
                    Some("tool_result") if typ == "user" => output.push(self.event(record, Kind::ToolResult,
                        block_id, json!({"tool_id": block["tool_use_id"], "result": block["content"], "is_error": block["is_error"]}))),
                    _ => {} // Thinking and all unknown block types are intentionally excluded.
                }
            }
        }
        if !text.is_empty() {
            if typ == "assistant" {
                if let Some(id) = &message_id {
                    // Claude can split a message over records. Publish replacement snapshots
                    // using the same source_id, so history and live use the same merge rule.
                    if self.messages.len() >= 128 && !self.messages.contains_key(id) {
                        self.messages.clear();
                    }
                    let previous = self.messages.entry(id.clone()).or_default();
                    if text.len() > previous.len() && text.starts_with(previous.as_str()) {
                        *previous = text;
                    } else {
                        previous.push_str(&text);
                    }
                    let mut end = previous.len().min(MAX_TEXT_BYTES);
                    while !previous.is_char_boundary(end) {
                        end -= 1;
                    }
                    let clipped = previous.len() > end;
                    previous.truncate(end);
                    text = previous.clone();
                    let mut event = self.event(
                        record,
                        Kind::AssistantMessage,
                        message_id.clone(),
                        json!({"text": text}),
                    );
                    event.truncated |= clipped;
                    output.push(event);
                } else {
                    output.push(self.event(
                        record,
                        Kind::AssistantMessage,
                        None,
                        json!({"text": text}),
                    ));
                }
            } else {
                output.push(self.event(
                    record,
                    Kind::UserMessage,
                    message_id.clone(),
                    crate::delivery::user_message_payload(&text),
                ));
            }
        }
        if typ == "assistant" && message["stop_reason"] == "end_turn" {
            output.push(self.event(record, Kind::TurnEnd, message_id, json!({})));
        }
        output
    }
}
