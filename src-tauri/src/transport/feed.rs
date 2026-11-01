//! Dedicated per-agent journals. This is deliberately independent of EventBus.
use super::{claude_transcript_source::ClaudeTranscriptSource, AgentEvent};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use tokio::sync::broadcast;

pub const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_PAGE_EVENTS: usize = 200;

pub(crate) fn safe_component(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid held identity".into());
    }
    Ok(())
}
pub(crate) fn validate_ids(session: &str, agent: &str) -> Result<(), String> {
    safe_component(session)?;
    safe_component(agent)
}
pub(crate) fn contained_file(root: &Path, path: &Path) -> Result<(), String> {
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(&canonical_root) {
        return Err("Feed path escapes root".into());
    }
    let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if fs::symlink_metadata(&current)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Feed symlinks are not allowed".into());
        }
    }
    if !fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("Feed is not a regular file".into());
    }
    Ok(())
}

fn journal_directory(root: &Path, session: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut directory = root.to_path_buf();
    for component in [session, "agent-feeds"] {
        directory.push(component);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err("Feed directory must be a real session directory".into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&directory).map_err(|e| e.to_string())?;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(directory)
}

fn validate_existing_file(root: &Path, path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            Err("Feed file must be a regular file, not a symlink".into())
        }
        Ok(_) => contained_file(root, path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cursor {
    pub generation: String,
    pub byte_offset: u64,
}
impl Cursor {
    pub fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).expect("cursor"))
    }
    pub fn decode(token: &str) -> Result<Self, String> {
        if token.len() > 256 {
            return Err("Invalid cursor".into());
        }
        let value: Self = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(token)
                .map_err(|_| "Invalid cursor")?,
        )
        .map_err(|_| "Invalid cursor")?;
        if uuid::Uuid::parse_str(&value.generation).is_err() {
            return Err("Invalid cursor generation".into());
        }
        Ok(value)
    }
}

pub type Checkpoint = BTreeMap<String, String>;
pub fn encode_checkpoint(checkpoint: &Checkpoint) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(checkpoint).expect("checkpoint"))
}
pub fn decode_checkpoint(token: &str) -> Result<Checkpoint, String> {
    if token.len() > 32 * 1024 {
        return Err("Checkpoint too large".into());
    }
    let checkpoint: Checkpoint = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| "Invalid checkpoint")?,
    )
    .map_err(|_| "Invalid checkpoint")?;
    for (agent, cursor) in &checkpoint {
        safe_component(agent)?;
        Cursor::decode(cursor)?;
    }
    Ok(checkpoint)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryPage {
    pub agent_id: String,
    /// Ascending inside a page, newest page first. Prepend earlier pages.
    pub events: Vec<AgentEvent>,
    pub next_before: Option<String>,
    pub latest_cursor: Option<String>,
    pub has_more: bool,
    pub reset: bool,
}

struct Journal {
    path: PathBuf,
    generation: String,
    size: u64,
    source: Option<ClaudeTranscriptSource>,
    protocol: bool,
}
pub struct FeedStore {
    sessions_root: PathBuf,
    claude_root: Option<PathBuf>,
    journals: Mutex<HashMap<(String, String), Journal>>,
    sender: broadcast::Sender<(String, AgentEvent)>,
}
impl FeedStore {
    pub fn new(sessions_root: PathBuf, claude_root: Option<PathBuf>) -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            sessions_root,
            claude_root,
            journals: Mutex::new(HashMap::new()),
            sender,
        }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<(String, AgentEvent)> {
        self.sender.subscribe()
    }

    fn journal(&self, session: &str, agent: &str) -> Result<Journal, String> {
        validate_ids(session, agent)?;
        let directory = journal_directory(&self.sessions_root, session)?;
        let canonical_root = self
            .sessions_root
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if !directory
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(canonical_root)
        {
            return Err("Feed directory escapes session storage".into());
        }
        let path = directory.join(format!("{agent}.jsonl"));
        validate_existing_file(&self.sessions_root, &path)?;
        let mode_path = path.with_extension("source");
        validate_existing_file(&self.sessions_root, &mode_path)?;
        let protocol = if mode_path.is_file() {
            let mut bytes = Vec::new();
            File::open(&mode_path)
                .map_err(|e| e.to_string())?
                .take(16)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            bytes == b"protocol"
        } else {
            false
        };
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let mut generation = uuid::Uuid::new_v4().to_string();
        if size > 0 {
            for event in read_window(&self.sessions_root, &path, 0, MAX_READ_BYTES)?.0 {
                if let Ok(cursor) = Cursor::decode(&event.cursor) {
                    generation = cursor.generation;
                    break;
                }
            }
        }
        Ok(Journal {
            path,
            generation,
            size,
            source: None,
            protocol,
        })
    }
    pub fn use_protocol(&self, session: &str, agent: &str) -> Result<(), String> {
        let mut journals = self.journals.lock();
        let key = (session.into(), agent.into());
        if !journals.contains_key(&key) {
            journals.insert(key.clone(), self.journal(session, agent)?);
        }
        let journal = journals.get_mut(&key).unwrap();
        if journal.source.is_some() || (!journal.protocol && journal.size > 0) {
            // Switching an already-read PTY agent must not expose both sources.
            journal_directory(&self.sessions_root, session)?;
            validate_existing_file(&self.sessions_root, &journal.path)?;
            File::create(&journal.path).map_err(|e| e.to_string())?;
            journal.size = 0;
            journal.generation = uuid::Uuid::new_v4().to_string();
        }
        journal.source = None;
        journal.protocol = true;
        let mode_path = journal.path.with_extension("source");
        validate_existing_file(&self.sessions_root, &mode_path)?;
        fs::write(mode_path, b"protocol").map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn use_transcript(
        &self,
        session: &str,
        agent: &str,
        cwd: &Path,
        transcript_id: &str,
    ) -> Result<(), String> {
        let Some(root) = &self.claude_root else {
            return Ok(());
        };
        let mut journals = self.journals.lock();
        let key = (session.into(), agent.into());
        let source = ClaudeTranscriptSource::new(root.clone(), cwd, transcript_id, agent)?;
        if let Some(journal) = journals.get(&key) {
            if journal.protocol
                || journal
                    .source
                    .as_ref()
                    .is_some_and(|held| held.path() == source.path())
            {
                return Ok(());
            }
        } // Protocol wins; a changed held transcript rotates the derived source.
        let mut journal = self.journal(session, agent)?;
        journal.source = Some(source);
        // Rebuild derived transcript materialization with a fresh generation on app restart.
        File::create(&journal.path).map_err(|e| e.to_string())?;
        journal.size = 0;
        journal.generation = uuid::Uuid::new_v4().to_string();
        fs::write(journal.path.with_extension("source"), b"transcript")
            .map_err(|e| e.to_string())?;
        journals.insert(key, journal);
        Ok(())
    }
    pub fn refresh_transcript(&self, session: &str, agent: &str) -> Result<(), String> {
        let key = (session.into(), agent.into());
        let target = self
            .journals
            .lock()
            .get(&key)
            .and_then(|j| j.source.as_ref())
            .map(|source| source.snapshot_len())
            .transpose()?
            .unwrap_or(0);
        // Import only the finite snapshot present at entry. Release the mutex after
        // every 2 MiB window so another agent/protocol append can make progress.
        loop {
            let mut journals = self.journals.lock();
            let Some(journal) = journals.get_mut(&key) else {
                break;
            };
            let Some(source) = journal.source.as_mut() else {
                break;
            };
            let (events, more, reset) = source.poll()?;
            let reached_snapshot = source.offset() >= target;
            if reset {
                journal_directory(&self.sessions_root, session)?;
                validate_existing_file(&self.sessions_root, &journal.path)?;
                File::create(&journal.path).map_err(|e| e.to_string())?;
                journal.size = 0;
                journal.generation = uuid::Uuid::new_v4().to_string();
            }
            for event in events {
                self.append(journal, session, agent, event)?;
            }
            drop(journals);
            if !more || reached_snapshot {
                break;
            }
        }
        Ok(())
    }
    fn append(
        &self,
        journal: &mut Journal,
        session: &str,
        agent: &str,
        mut event: AgentEvent,
    ) -> Result<AgentEvent, String> {
        if event.agent_id != agent {
            return Err("Event agent does not match registered identity".into());
        }
        journal_directory(&self.sessions_root, session)?;
        validate_existing_file(&self.sessions_root, &journal.path)?;
        event = event.normalize();
        event.cursor = Cursor {
            generation: journal.generation.clone(),
            byte_offset: journal.size,
        }
        .encode();
        let mut bytes = serde_json::to_vec(&event).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&journal.path)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        journal.size += bytes.len() as u64;
        let _ = self.sender.send((session.into(), event.clone()));
        Ok(event)
    }
    pub fn append_protocol(
        &self,
        session: &str,
        agent: &str,
        event: AgentEvent,
    ) -> Result<AgentEvent, String> {
        validate_ids(session, agent)?;
        let mut journals = self.journals.lock();
        let journal = journals
            .get_mut(&(session.into(), agent.into()))
            .ok_or("Feed not registered")?;
        if !journal.protocol {
            return Err("Agent does not have a protocol source".into());
        }
        self.append(journal, session, agent, event)
    }
    pub fn history(
        &self,
        session: &str,
        agent: &str,
        before: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<HistoryPage, String> {
        validate_ids(session, agent)?;
        let before = before.map(Cursor::decode).transpose()?;
        let after = after.map(Cursor::decode).transpose()?;
        let journals = self.journals.lock();
        let Some(journal) = journals.get(&(session.into(), agent.into())) else {
            return Ok(HistoryPage {
                agent_id: agent.into(),
                events: vec![],
                next_before: None,
                latest_cursor: None,
                has_more: false,
                reset: before.is_some() || after.is_some(),
            });
        };
        let reset = before
            .as_ref()
            .or(after.as_ref())
            .is_some_and(|c| c.generation != journal.generation);
        let end = before
            .as_ref()
            .filter(|_| !reset)
            .map(|c| c.byte_offset)
            .unwrap_or(journal.size)
            .min(journal.size);
        let start = if let Some(cursor) = after.as_ref().filter(|_| !reset) {
            cursor.byte_offset.min(journal.size)
        } else {
            end.saturating_sub(MAX_READ_BYTES)
        };
        let (mut events, scanned_end) = read_window(
            &self.sessions_root,
            &journal.path,
            start,
            (end - start).min(MAX_READ_BYTES),
        )?;
        events.retain(|event| {
            event.agent_id == agent
                && Cursor::decode(&event.cursor).is_ok_and(|cursor| {
                    cursor.generation == journal.generation
                        && cursor.byte_offset < end
                        && (reset
                            || after
                                .as_ref()
                                .is_none_or(|a| cursor.byte_offset > a.byte_offset))
                })
        });
        let limit = limit.clamp(1, MAX_PAGE_EVENTS);
        let has_more;
        if after.is_some() && !reset {
            has_more = events.len() > limit || scanned_end < end;
            events.truncate(limit);
        } else {
            has_more = events.len() > limit || start > 0;
            if events.len() > limit {
                events.drain(..events.len() - limit);
            }
        }
        let next_before = if has_more {
            events.first().map(|e| e.cursor.clone())
        } else {
            None
        };
        let latest_cursor = events.last().map(|e| e.cursor.clone());
        Ok(HistoryPage {
            agent_id: agent.into(),
            events,
            next_before,
            latest_cursor,
            has_more,
            reset,
        })
    }
}

/// A shared bounded reader for every normalized source. A window never holds >2 MiB.
fn read_window(
    root: &Path,
    path: &Path,
    start: u64,
    count: u64,
) -> Result<(Vec<AgentEvent>, u64), String> {
    if !path.exists() {
        return Ok((vec![], start));
    }
    contained_file(root, path)?;
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let boundary = if start == 0 {
        true
    } else {
        file.seek(SeekFrom::Start(start - 1))
            .map_err(|e| e.to_string())?;
        let mut previous = [0];
        file.read_exact(&mut previous).map_err(|e| e.to_string())?;
        previous[0] == b'\n'
    };
    file.seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(count.min(MAX_READ_BYTES))
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let mut events = Vec::new();
    let mut offset = start;
    let mut scanned_end = start;
    for (index, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
        if line.last() != Some(&b'\n') {
            break;
        }
        if boundary || index > 0 {
            if let Ok(event) = serde_json::from_slice::<AgentEvent>(line) {
                if Cursor::decode(&event.cursor).is_ok_and(|c| c.byte_offset == offset) {
                    events.push(event.normalize());
                }
            }
        }
        offset += line.len() as u64;
        scanned_end = offset;
    }
    Ok((events, scanned_end))
}
