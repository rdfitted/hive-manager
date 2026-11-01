//! Dedicated per-agent journals. This is deliberately independent of EventBus.
use super::{
    claude_transcript_source::ClaudeTranscriptSource,
    codex_rollout_source::{CodexBinder, CodexBinding, CodexRolloutSource},
    AgentEvent,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Instant,
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

enum TranscriptSource {
    Claude(ClaudeTranscriptSource),
    Codex(CodexRolloutSource),
}
impl TranscriptSource {
    fn path(&self) -> &Path {
        match self {
            Self::Claude(source) => source.path(),
            Self::Codex(source) => source.path(),
        }
    }
    fn offset(&self) -> u64 {
        match self {
            Self::Claude(source) => source.offset(),
            Self::Codex(source) => source.offset(),
        }
    }
    fn snapshot_len(&self) -> Result<u64, String> {
        match self {
            Self::Claude(source) => source.snapshot_len(),
            Self::Codex(source) => source.snapshot_len(),
        }
    }
    fn poll(&mut self) -> Result<(Vec<AgentEvent>, bool, bool), String> {
        match self {
            Self::Claude(source) => source.poll(),
            Self::Codex(source) => source.poll(),
        }
    }
    fn is_codex(&self) -> bool {
        matches!(self, Self::Codex(_))
    }
    fn matches_binding(&self, binding: &CodexBinding) -> bool {
        matches!(self, Self::Codex(source) if source.matches_binding(binding))
    }
}

fn event_fingerprint(event: &AgentEvent) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(&(
        &event.ts,
        event.kind,
        &event.source_id,
        event.truncated,
        &event.payload,
    ))
    .expect("event fingerprint")
    .hash(&mut hasher);
    hasher.finish()
}

struct Journal {
    path: PathBuf,
    generation: String,
    size: u64,
    source: Option<TranscriptSource>,
    codex_seen: HashSet<u64>,
    protocol: bool,
}
pub struct FeedStore {
    sessions_root: PathBuf,
    claude_root: Option<PathBuf>,
    codex: Mutex<CodexBinder>,
    #[cfg(test)]
    discovery_observer: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    journals: Mutex<HashMap<(String, String), Journal>>,
    sender: broadcast::Sender<(String, AgentEvent)>,
}
impl FeedStore {
    pub fn new(sessions_root: PathBuf, claude_root: Option<PathBuf>) -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            sessions_root,
            claude_root,
            codex: Mutex::new(CodexBinder::new(None)),
            #[cfg(test)]
            discovery_observer: Mutex::new(None),
            journals: Mutex::new(HashMap::new()),
            sender,
        }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<(String, AgentEvent)> {
        self.sender.subscribe()
    }
    pub fn set_codex_root(&self, root: Option<PathBuf>) {
        self.codex.lock().set_root(root);
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
            codex_seen: HashSet::new(),
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
            journal.codex_seen.clear();
        }
        journal.source = None;
        journal.protocol = true;
        let mode_path = journal.path.with_extension("source");
        validate_existing_file(&self.sessions_root, &mode_path)?;
        fs::write(mode_path, b"protocol").map_err(|e| e.to_string())?;
        Ok(())
    }
    /// Load the durable source even when no child has been registered after restart.
    pub fn is_protocol(&self, session: &str, agent: &str) -> Result<bool, String> {
        let mut journals = self.journals.lock();
        let key = (session.into(), agent.into());
        if !journals.contains_key(&key) {
            journals.insert(key.clone(), self.journal(session, agent)?);
        }
        Ok(journals.get(&key).unwrap().protocol)
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
        if journal.protocol {
            journals.insert(key, journal);
            return Ok(());
        }
        journal.source = Some(TranscriptSource::Claude(source));
        // Rebuild derived transcript materialization with a fresh generation on app restart.
        File::create(&journal.path).map_err(|e| e.to_string())?;
        journal.size = 0;
        journal.generation = uuid::Uuid::new_v4().to_string();
        fs::write(journal.path.with_extension("source"), b"transcript")
            .map_err(|e| e.to_string())?;
        journals.insert(key, journal);
        Ok(())
    }

    /// Discovery and cold replay preparation happen outside the journals lock.
    /// A readable, identity-checked first window is required before any write.
    pub fn use_codex_rollout(
        &self,
        session: &str,
        agent: &str,
        cwd: &Path,
        transcript_key: Option<&str>,
        spawned_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<bool, String> {
        validate_ids(session, agent)?;
        if self.is_protocol(session, agent)? {
            return Ok(false);
        }
        let directory = journal_directory(&self.sessions_root, session)?;
        let sidecar = directory.join(format!("{agent}.binding"));
        validate_existing_file(&self.sessions_root, &sidecar)?;
        let (binding, root) = {
            let mut binder = self.codex.lock();
            let binding = binder.resolve(
                session,
                agent,
                transcript_key,
                spawned_at,
                cwd,
                &sidecar,
                Instant::now(),
                || {
                    #[cfg(test)]
                    if let Some(observe) = self.discovery_observer.lock().as_ref() {
                        observe();
                    }
                },
            );
            (binding, binder.root().map(Path::to_path_buf))
        };
        let key = (session.into(), agent.into());
        let Some((binding, root)) = binding.zip(root) else {
            if let Some(journal) = self.journals.lock().get_mut(&key) {
                if journal
                    .source
                    .as_ref()
                    .is_some_and(TranscriptSource::is_codex)
                {
                    journal.source = None;
                }
            }
            return Ok(false);
        };
        {
            let journals = self.journals.lock();
            if let Some(source) = journals
                .get(&key)
                .and_then(|journal| journal.source.as_ref())
            {
                if source.matches_binding(&binding) {
                    return Ok(source.snapshot_len().is_ok());
                }
            }
        }
        let Ok(mut source) = CodexRolloutSource::bound(root, &binding, cwd, agent) else {
            return Ok(false);
        };
        let Ok((events, _, _)) = source.poll() else {
            return Ok(false);
        };
        let mut journal = self.journal(session, agent)?;
        let marker = format!(
            "codex:{}",
            uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_DNS,
                &serde_json::to_vec(&binding).map_err(|error| error.to_string())?
            )
        );
        let mode_path = journal.path.with_extension("source");
        let mut old_marker = Vec::new();
        if mode_path.is_file() {
            File::open(&mode_path)
                .map_err(|error| error.to_string())?
                .take(128)
                .read_to_end(&mut old_marker)
                .map_err(|error| error.to_string())?;
        }
        let reuse = old_marker == marker.as_bytes();
        if reuse {
            let mut offset = 0;
            while offset < journal.size {
                let (events, end) =
                    read_window(&self.sessions_root, &journal.path, offset, MAX_READ_BYTES)?;
                journal
                    .codex_seen
                    .extend(events.iter().map(event_fingerprint));
                if end <= offset {
                    break;
                }
                offset = end;
            }
        }
        let mut journals = self.journals.lock();
        if let Some(held) = journals.get(&key) {
            if held.protocol {
                return Ok(false);
            }
            if held
                .source
                .as_ref()
                .is_some_and(|source| source.matches_binding(&binding))
            {
                return Ok(true);
            }
        }
        if !reuse {
            validate_existing_file(&self.sessions_root, &journal.path)?;
            File::create(&journal.path).map_err(|error| error.to_string())?;
            journal.size = 0;
            journal.generation = uuid::Uuid::new_v4().to_string();
        }
        journal.source = Some(TranscriptSource::Codex(source));
        fs::write(mode_path, marker).map_err(|error| error.to_string())?;
        for event in events {
            self.append_codex(&mut journal, session, agent, event)?;
        }
        journals.insert(key, journal);
        Ok(true)
    }

    fn append_codex(
        &self,
        journal: &mut Journal,
        session: &str,
        agent: &str,
        event: AgentEvent,
    ) -> Result<(), String> {
        let fingerprint = event_fingerprint(&event);
        if !journal.codex_seen.contains(&fingerprint) {
            self.append(journal, session, agent, event)?;
            journal.codex_seen.insert(fingerprint);
        }
        Ok(())
    }
    pub fn refresh_transcript(&self, session: &str, agent: &str) -> Result<(), String> {
        let key = (session.into(), agent.into());
        let target = {
            let journals = self.journals.lock();
            match journals
                .get(&key)
                .and_then(|journal| journal.source.as_ref())
            {
                Some(source) => match source.snapshot_len() {
                    Ok(size) => size,
                    Err(_) if source.is_codex() => return Ok(()),
                    Err(error) => return Err(error),
                },
                None => 0,
            }
        };
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
            let codex = source.is_codex();
            let (events, more, reset) = match source.poll() {
                Ok(window) => window,
                Err(_) if codex => break, // Keep every durable byte on source failure.
                Err(error) => return Err(error),
            };
            let reached_snapshot = source.offset() >= target;
            if reset && !codex {
                journal_directory(&self.sessions_root, session)?;
                validate_existing_file(&self.sessions_root, &journal.path)?;
                File::create(&journal.path).map_err(|e| e.to_string())?;
                journal.size = 0;
                journal.generation = uuid::Uuid::new_v4().to_string();
            }
            for event in events {
                if codex {
                    self.append_codex(journal, session, agent, event)?;
                } else {
                    self.append(journal, session, agent, event)?;
                }
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

#[cfg(test)]
mod discovery_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn g16b_directory_discovery_runs_outside_the_journals_lock() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("codex-sessions");
        fs::create_dir_all(root.join("2026/11/01")).unwrap();
        let feeds = Arc::new(FeedStore::new(temp.path().join("sessions"), None));
        feeds.set_codex_root(Some(root));
        let observed = Arc::new(AtomicBool::new(false));
        let signal = observed.clone();
        let weak = Arc::downgrade(&feeds);
        *feeds.discovery_observer.lock() = Some(Box::new(move || {
            assert!(
                weak.upgrade().unwrap().journals.try_lock().is_some(),
                "G16b directory scans must not hold the journals lock"
            );
            signal.store(true, Ordering::SeqCst);
        }));
        assert!(!feeds
            .use_codex_rollout(
                "session",
                "worker-a",
                Path::new("fixture"),
                Some("codex-prompt-fixture"),
                Some("2026-11-01T12:00:00Z".parse().unwrap())
            )
            .unwrap());
        assert!(
            observed.load(Ordering::SeqCst),
            "G16b exercised a discovery scan"
        );
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
