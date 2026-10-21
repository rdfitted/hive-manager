//! Incremental, nonce-scoped delivery evidence from Codex's plain rollout files.
use chrono::{Days, Local, NaiveDate};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::SystemTime;

const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PARTIAL_BYTES: usize = 2 * 1024 * 1024;

#[derive(Default)]
struct Tail {
    offset: u64,
    partial: Vec<u8>,
    discard_line: bool,
    matched_user: bool,
    pending: bool,
    confirmed: Option<String>,
}

/// One reader belongs to one active confirmation watch. Changing the query
/// rewinds its tails, so a concurrent watch must use its own reader.
pub struct CodexRolloutReader {
    root: PathBuf,
    spawned_at: SystemTime,
    query: Option<(String, SystemTime)>,
    files: HashMap<PathBuf, Tail>,
}

impl CodexRolloutReader {
    pub fn new(root: PathBuf, spawned_at: SystemTime) -> Self {
        Self { root, spawned_at, query: None, files: HashMap::new() }
    }

    pub fn poll(&mut self, nonce: &str, since: SystemTime) -> Option<String> {
        self.poll_on(nonce, since, Local::now().date_naive())
    }

    /// User acceptance is already evidence against an Enter retry, even while
    /// Codex queues the message and has not emitted task_started yet.
    pub fn message_recorded(&self) -> bool {
        self.files.values().any(|tail| tail.matched_user)
    }

    fn poll_on(&mut self, nonce: &str, since: SystemTime, today: NaiveDate) -> Option<String> {
        if nonce.is_empty() {
            return None;
        }
        let query = (nonce.to_owned(), since);
        if self.query.as_ref() != Some(&query) {
            self.files.clear();
            self.query = Some(query);
        }
        // A long-lived process keeps appending to the rollout in its original
        // local spawn-date bucket, even when that bucket is older than yesterday.
        let spawn_date = chrono::DateTime::<Local>::from(self.spawned_at).date_naive();
        let mut dates = vec![today];
        for date in [today.checked_sub_days(Days::new(1)), Some(spawn_date)].into_iter().flatten() {
            if !dates.contains(&date) { dates.push(date); }
        }
        for date in dates {
            let directory = self.root.join(date.format("%Y/%m/%d").to_string());
            let Ok(entries) = fs::read_dir(directory) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
                if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                    continue;
                }
                // Do not follow symlinks outside the supplied sessions root.
                let Ok(kind) = entry.file_type() else { continue };
                if !kind.is_file() { continue; }
                let Ok(metadata) = entry.metadata() else { continue };
                if metadata.modified().map(|m| m < self.spawned_at).unwrap_or(true) {
                    continue;
                }
                let Ok(mut file) = File::open(&path) else { continue };
                let tail = self.files.entry(path).or_default();
                if metadata.len() < tail.offset {
                    let matched_user = tail.matched_user;
                    *tail = Tail { matched_user, ..Tail::default() };
                }
                if file.seek(SeekFrom::Start(tail.offset)).is_err() { continue; }
                let mut bytes = Vec::new();
                if file.take(MAX_READ_BYTES).read_to_end(&mut bytes).is_err() { continue; }
                tail.offset += bytes.len() as u64;
                if tail.discard_line {
                    if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
                        bytes.drain(..=newline);
                        tail.discard_line = false;
                    } else {
                        continue;
                    }
                }
                tail.partial.extend(bytes);
                let mut consumed = 0;
                for (index, byte) in tail.partial.iter().enumerate() {
                    if *byte != b'\n' { continue; }
                    if let Ok(record) = serde_json::from_slice::<Value>(&tail.partial[consumed..index]) {
                        observe(&record, nonce, since, &mut tail.pending, &mut tail.matched_user, &mut tail.confirmed);
                    }
                    consumed = index + 1;
                }
                tail.partial.drain(..consumed);
                if tail.partial.len() > MAX_PARTIAL_BYTES {
                    tail.partial.clear();
                    tail.discard_line = true;
                }
                if let Some(turn) = &tail.confirmed { return Some(turn.clone()); }
            }
        }
        None
    }
}

fn observe(record: &Value, nonce: &str, since: SystemTime, pending: &mut bool, matched_user: &mut bool, confirmed: &mut Option<String>) {
    let payload = &record["payload"];
    if record["type"] == "response_item" && payload["role"] == "user" {
        let recent = record["timestamp"].as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| SystemTime::from(t.with_timezone(&chrono::Utc)) >= since)
            .unwrap_or(false);
        let matches = payload["content"].as_array().map(|content| content.iter().any(|item| {
            item["text"].as_str().map(|text| text.lines().any(|line| line.trim() == nonce)).unwrap_or(false)
        })).unwrap_or(false);
        // A different user item breaks attribution; a later worker's turn must
        // never borrow an earlier user's nonce.
        *pending = recent && matches;
        *matched_user |= *pending;
    } else if *pending && record["type"] == "event_msg" && payload["type"] == "task_started" {
        if let Some(turn) = payload["turn_attribution"]["turn_id"].as_str().filter(|id| !id.is_empty()) {
            *confirmed = Some(turn.to_owned());
            *pending = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    const NONCE: &str = "[hive-msg:a1b2c3d4]";
    const FIXTURE: &str = include_str!("fixtures/codex-rollout.jsonl");

    fn write(root: &std::path::Path, date: NaiveDate, name: &str, text: &str) -> PathBuf {
        let dir = root.join(date.format("%Y/%m/%d").to_string());
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn g_nonce_isolation_shared_cwd_different_nonce() {
        let temp = tempfile::tempdir().unwrap();
        let day = Local::now().date_naive();
        write(temp.path(), day, "rollout-other.jsonl", &FIXTURE.replace(NONCE, "[hive-msg:deadbeef]"));
        let mut reader = CodexRolloutReader::new(temp.path().into(), SystemTime::UNIX_EPOCH);
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), None);
        write(temp.path(), day, "rollout-target.jsonl", FIXTURE);
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), Some("scrubbed-turn".into()));
    }

    #[test]
    fn split_lines_are_buffered_and_offsets_only_read_appends() {
        let temp = tempfile::tempdir().unwrap();
        let day = Local::now().date_naive();
        let split = FIXTURE.rfind("turn_id").unwrap() + 3;
        let path = write(temp.path(), day, "rollout-split.jsonl", &FIXTURE[..split]);
        let mut reader = CodexRolloutReader::new(temp.path().into(), SystemTime::UNIX_EPOCH);
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), None);
        let first_offset = reader.files[&path].offset;
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), None);
        assert_eq!(reader.files[&path].offset, first_offset);
        File::options().append(true).open(&path).unwrap().write_all(FIXTURE[split..].as_bytes()).unwrap();
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), Some("scrubbed-turn".into()));
    }

    #[test]
    fn local_today_and_yesterday_only_plain_rollouts() {
        let temp = tempfile::tempdir().unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
        write(temp.path(), day, "rollout-compressed.jsonl.zst", FIXTURE);
        write(temp.path(), day, "unrelated.jsonl", FIXTURE);
        write(temp.path(), day - Days::new(2), "rollout-old.jsonl", FIXTURE);
        let mut reader = CodexRolloutReader::new(temp.path().into(), SystemTime::UNIX_EPOCH);
        assert_eq!(reader.poll_on(NONCE, SystemTime::UNIX_EPOCH, day), None);
        write(temp.path(), day - Days::new(1), "rollout-yesterday.jsonl", FIXTURE);
        assert_eq!(reader.poll_on(NONCE, SystemTime::UNIX_EPOCH, day), Some("scrubbed-turn".into()));
    }

    #[test]
    fn long_lived_reader_finds_current_turn_in_original_spawn_date_bucket() {
        let temp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let spawned_at = now - Duration::from_secs(3 * 24 * 60 * 60);
        let spawn_date = chrono::DateTime::<Local>::from(spawned_at).date_naive();
        assert!(spawn_date < Local::now().date_naive() - Days::new(1));
        let timestamp = chrono::DateTime::<chrono::Utc>::from(now).to_rfc3339();
        let fixture = FIXTURE
            .replace("2026-10-21T12:00:00.000Z", &timestamp)
            .replace("2026-10-21T12:00:01.000Z", &timestamp)
            .replace("2026-10-21T12:00:02.000Z", &timestamp);
        write(temp.path(), spawn_date, "rollout-long-lived.jsonl", &fixture);
        let mut reader = CodexRolloutReader::new(temp.path().into(), spawned_at);
        assert_eq!(reader.poll(NONCE, now - Duration::from_secs(1)), Some("scrubbed-turn".into()));
    }

    #[test]
    fn stale_user_and_pre_spawn_file_cannot_confirm() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), Local::now().date_naive(), "rollout-stale.jsonl", FIXTURE);
        let mut reader = CodexRolloutReader::new(temp.path().into(), SystemTime::UNIX_EPOCH);
        let after_fixture = SystemTime::from(chrono::DateTime::parse_from_rfc3339("2026-10-21T12:00:03Z").unwrap());
        assert_eq!(reader.poll(NONCE, after_fixture), None);
        let mut future_spawn = CodexRolloutReader::new(temp.path().into(), SystemTime::now() + Duration::from_secs(60));
        assert_eq!(future_spawn.poll(NONCE, SystemTime::UNIX_EPOCH), None);
        assert!(future_spawn.files.is_empty());
    }

    #[test]
    fn user_and_turn_must_be_in_the_same_rollout() {
        let temp = tempfile::tempdir().unwrap();
        let day = Local::now().date_naive();
        let lines: Vec<_> = FIXTURE.lines().collect();
        write(temp.path(), day, "rollout-user.jsonl", &format!("{}\n", lines[1]));
        write(temp.path(), day, "rollout-turn.jsonl", &format!("{}\n", lines[2]));
        let mut reader = CodexRolloutReader::new(temp.path().into(), SystemTime::UNIX_EPOCH);
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), None);
    }

    #[test]
    fn pending_user_record_is_retained_as_evidence_against_retry() {
        let temp = tempfile::tempdir().unwrap();
        let day = Local::now().date_naive();
        let lines: Vec<_> = FIXTURE.lines().collect();
        let path = write(temp.path(), day, "rollout-pending.jsonl", &format!("{}\n", lines[1]));
        let mut reader = CodexRolloutReader::new(temp.path().into(), SystemTime::UNIX_EPOCH);
        assert!(!reader.message_recorded());
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), None);
        assert!(reader.message_recorded());
        let other_user = lines[1].replace(NONCE, "[hive-msg:deadbeef]");
        File::options().append(true).open(&path).unwrap().write_all(format!("{other_user}\n").as_bytes()).unwrap();
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), None);
        assert!(reader.message_recorded());
        assert_eq!(reader.poll("[hive-msg:00000000]", SystemTime::UNIX_EPOCH), None);
        assert!(!reader.message_recorded());
    }

    #[test]
    fn oversized_lines_and_reads_are_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let day = Local::now().date_naive();
        let text = format!("{}\n{FIXTURE}", "x".repeat(MAX_PARTIAL_BYTES + 100));
        let path = write(temp.path(), day, "rollout-bounded.jsonl", &text);
        let mut reader = CodexRolloutReader::new(temp.path().into(), SystemTime::UNIX_EPOCH);
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), None);
        assert_eq!(reader.files[&path].offset, MAX_READ_BYTES);
        assert!(reader.files[&path].partial.len() <= MAX_PARTIAL_BYTES);
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), Some("scrubbed-turn".into()));
    }

    #[test]
    fn assistant_nonce_substring_and_turn_before_user_do_not_confirm() {
        let temp = tempfile::tempdir().unwrap();
        let day = Local::now().date_naive();
        let mut reader = CodexRolloutReader::new(temp.path().into(), SystemTime::UNIX_EPOCH);
        let path = write(temp.path(), day, "rollout-negative.jsonl", &FIXTURE.replace("\"role\":\"user\"", "\"role\":\"assistant\""));
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), None);
        fs::remove_file(path).unwrap();
        write(temp.path(), day, "rollout-substring.jsonl", &FIXTURE.replace(NONCE, &format!("{NONCE}extra")));
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), None);
        let lines: Vec<_> = FIXTURE.lines().collect();
        write(temp.path(), day, "rollout-reverse.jsonl", &format!("{}\n{}\n", lines[2], lines[1]));
        assert_eq!(reader.poll(NONCE, SystemTime::UNIX_EPOCH), None);
    }
}
