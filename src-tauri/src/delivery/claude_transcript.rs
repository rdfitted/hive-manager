//! Incremental reads of one explicitly assigned Claude session transcript.
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

const MAX_PARTIAL_BYTES: usize = 2 * 1024 * 1024;

pub struct ClaudeTranscriptReader {
    root: PathBuf,
    direct: PathBuf,
    filename: Option<String>,
    fallback_checked: bool,
    fallback: Option<PathBuf>,
    path: Option<PathBuf>,
    offset: u64,
    partial: Vec<u8>,
    discard_line: bool,
    users: HashMap<String, (String, SystemTime)>,
    confirmed: HashMap<String, (String, SystemTime)>,
}

impl ClaudeTranscriptReader {
    /// `root` is the projects directory, supplied by the caller (never a test home).
    pub fn new(root: PathBuf, cwd: &Path, session_id: &str) -> Self {
        let slug: String = cwd.to_string_lossy().chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
        let filename = (!session_id.is_empty()
            && session_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'))
            .then(|| format!("{session_id}.jsonl"));
        let direct = root.join(slug).join(filename.as_deref().unwrap_or("invalid-session"));
        Self {
            root, direct, filename, fallback_checked: false, fallback: None,
            path: None, offset: 0, partial: Vec::new(), discard_line: false,
            users: HashMap::new(), confirmed: HashMap::new(),
        }
    }

    fn locate(&mut self) -> Option<PathBuf> {
        let filename = self.filename.as_ref()?;
        if self.direct.is_file() {
            return Some(self.direct.clone());
        }
        if !self.fallback_checked {
            self.fallback_checked = true;
            if let Ok(projects) = fs::read_dir(&self.root) {
                // Only one project level and this session's exact filename; never recurse.
                for project in projects.flatten() {
                    if project.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                        let candidate = project.path().join(filename);
                        if candidate.is_file() {
                            self.fallback = Some(candidate);
                            break;
                        }
                    }
                }
            }
        }
        self.fallback.clone()
    }

    /// Confirms only an assistant response linked to a nonce-bearing user record.
    /// The returned identifier is Claude's assistant message id.
    pub fn poll(&mut self, nonce: &str, since: SystemTime) -> Option<String> {
        let path = self.locate()?;
        let mut file = File::open(&path).ok()?;
        let size = file.metadata().ok()?.len();
        if self.path.as_ref() != Some(&path) || size < self.offset {
            self.path = Some(path);
            self.offset = 0;
            self.partial.clear();
            self.discard_line = false;
            self.users.clear();
            self.confirmed.clear();
        }
        file.seek(SeekFrom::Start(self.offset)).ok()?;
        // Bound each poll while retaining offsets for the next one.
        let mut bytes = Vec::new();
        file.take(2 * 1024 * 1024).read_to_end(&mut bytes).ok()?;
        self.offset += bytes.len() as u64;
        if self.discard_line {
            if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
                bytes.drain(..=newline);
                self.discard_line = false;
            } else {
                return self.confirmed.get(nonce)
                    .filter(|(_, timestamp)| *timestamp >= since)
                    .map(|(turn_id, _)| turn_id.clone());
            }
        }
        self.partial.extend_from_slice(&bytes);
        if let Some(last) = self.partial.iter().rposition(|&b| b == b'\n') {
            let complete: Vec<u8> = self.partial.drain(..=last).collect();
            for line in complete.split(|&b| b == b'\n') {
                if let Ok(record) = serde_json::from_slice::<Value>(line) {
                    self.record(&record);
                }
            }
        }
        if self.partial.len() > MAX_PARTIAL_BYTES {
            self.partial.clear();
            self.discard_line = true;
        }
        self.confirmed.get(nonce)
            .filter(|(_, timestamp)| *timestamp >= since)
            .map(|(turn_id, _)| turn_id.clone())
    }

    /// True once the receiver has recorded the nonce, even before an assistant turn.
    pub fn message_recorded(&self, nonce: &str) -> bool {
        self.users.values().any(|(recorded, _)| recorded == nonce)
    }

    fn record(&mut self, record: &Value) {
        let Some(timestamp) = record["timestamp"].as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(SystemTime::from) else { return };
        match record["type"].as_str() {
            Some("user") if record["message"]["role"] == "user" => {
                let Some(uuid) = record["uuid"].as_str() else { return };
                let content = &record["message"]["content"];
                let texts: Vec<&str> = if let Some(text) = content.as_str() {
                    vec![text]
                } else {
                    content.as_array().map(|items| items.iter()
                        .filter(|item| item["type"] == "text")
                        .filter_map(|item| item["text"].as_str()).collect()).unwrap_or_default()
                };
                for text in texts {
                    for line in text.lines() {
                        if let Some(hex) = line.strip_prefix("[hive-msg:").and_then(|s| s.strip_suffix(']')) {
                            if hex.len() == 8 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                                self.users.insert(uuid.to_owned(), (line.to_owned(), timestamp));
                            }
                        }
                    }
                }
            }
            Some("assistant") if record["message"]["role"] == "assistant" => {
                let Some(parent) = record["parentUuid"].as_str() else { return };
                let Some(turn_id) = record["message"]["id"].as_str().filter(|id| !id.is_empty()) else { return };
                if let Some((nonce, user_time)) = self.users.get(parent) {
                    if timestamp >= *user_time {
                        self.confirmed.insert(nonce.clone(), (turn_id.to_owned(), *user_time));
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{Duration, UNIX_EPOCH};

    const FIXTURE: &str = include_str!("fixtures/claude-success.jsonl");
    const NONCE: &str = "[hive-msg:1234abcd]";

    fn setup() -> (tempfile::TempDir, PathBuf, ClaudeTranscriptReader) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("D--fixture-worktree").join("session-a.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let reader = ClaudeTranscriptReader::new(temp.path().to_owned(), Path::new("D:/fixture/worktree"), "session-a");
        (temp, path, reader)
    }

    #[test]
    fn confirms_linked_assistant_and_rejects_other_nonce() {
        let (_temp, path, mut reader) = setup();
        fs::write(path, FIXTURE).unwrap();
        assert_eq!(reader.poll(NONCE, UNIX_EPOCH), Some("msg_fixture_turn".to_owned()));
        assert_eq!(reader.poll("[hive-msg:ffffffff]", UNIX_EPOCH), None);
        assert_eq!(reader.poll(NONCE, SystemTime::now() + Duration::from_secs(60)), None);
    }

    #[test]
    fn partial_lines_are_retained_until_complete() {
        let (_temp, path, mut reader) = setup();
        let split = FIXTURE.len() - 8;
        fs::write(&path, &FIXTURE[..split]).unwrap();
        assert_eq!(reader.poll(NONCE, UNIX_EPOCH), None);
        assert!(reader.message_recorded(NONCE));
        assert!(!reader.message_recorded("[hive-msg:ffffffff]"));
        File::options().append(true).open(path).unwrap().write_all(FIXTURE[split..].as_bytes()).unwrap();
        assert_eq!(reader.poll(NONCE, UNIX_EPOCH), Some("msg_fixture_turn".to_owned()));
    }

    #[test]
    fn no_nonce_or_wrong_parent_never_confirms() {
        for fixture in [FIXTURE.replace(NONCE, "ordinary fixture text"),
            FIXTURE.replace("\"parentUuid\":\"user_fixture\"", "\"parentUuid\":\"another_user\"")] {
            let (_temp, path, mut reader) = setup();
            fs::write(path, fixture).unwrap();
            assert_eq!(reader.poll(NONCE, UNIX_EPOCH), None);
        }
    }

    #[test]
    fn fallback_is_cached_and_reads_only_assigned_session() {
        let (temp, direct, mut reader) = setup();
        let fallback = temp.path().join("other-slug").join("session-a.jsonl");
        fs::create_dir_all(fallback.parent().unwrap()).unwrap();
        fs::write(&fallback, FIXTURE).unwrap();
        fs::write(direct.with_file_name("other-session.jsonl"), FIXTURE).unwrap();
        assert_eq!(reader.poll(NONCE, UNIX_EPOCH), Some("msg_fixture_turn".to_owned()));
        assert!(reader.fallback_checked);
        assert_eq!(reader.fallback.as_ref(), Some(&fallback));
        fs::remove_file(fallback).unwrap();
        let later = temp.path().join("new-slug");
        fs::create_dir_all(&later).unwrap();
        fs::write(later.join("session-a.jsonl"), FIXTURE).unwrap();
        assert_eq!(reader.poll(NONCE, UNIX_EPOCH), None);
    }

    #[test]
    fn missing_direct_file_can_appear_after_fallback_miss() {
        let (_temp, path, mut reader) = setup();
        assert_eq!(reader.poll(NONCE, UNIX_EPOCH), None);
        fs::write(path, FIXTURE).unwrap();
        assert_eq!(reader.poll(NONCE, UNIX_EPOCH), Some("msg_fixture_turn".to_owned()));
    }

    #[test]
    fn refuses_session_path_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let mut reader = ClaudeTranscriptReader::new(temp.path().to_owned(), Path::new("fixture"), "../secret");
        assert_eq!(reader.poll(NONCE, UNIX_EPOCH), None);
        assert!(!reader.fallback_checked);
    }

    #[test]
    fn oversized_partial_line_is_bounded_and_discarded_until_newline() {
        let (_temp, path, mut reader) = setup();
        fs::write(&path, vec![b'x'; MAX_PARTIAL_BYTES + 1]).unwrap();
        assert_eq!(reader.poll(NONCE, UNIX_EPOCH), None);
        assert!(reader.partial.len() <= MAX_PARTIAL_BYTES);
        assert_eq!(reader.poll(NONCE, UNIX_EPOCH), None);
        assert!(reader.partial.is_empty());
        assert!(reader.discard_line);
        // A valid-looking suffix still belongs to the oversized line and must be ignored.
        File::options().append(true).open(&path).unwrap().write_all(FIXTURE.as_bytes()).unwrap();
        assert_eq!(reader.poll(NONCE, UNIX_EPOCH), None);
        assert!(!reader.message_recorded(NONCE));
        assert!(!reader.discard_line);
        File::options().append(true).open(path).unwrap().write_all(FIXTURE.as_bytes()).unwrap();
        assert_eq!(reader.poll(NONCE, UNIX_EPOCH), Some("msg_fixture_turn".to_owned()));
        assert!(reader.partial.is_empty());
    }
}
