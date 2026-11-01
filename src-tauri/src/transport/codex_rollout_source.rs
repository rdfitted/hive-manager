//! Normalizes one already-attributed Codex rollout. Discovery never occurs here.
use super::{feed, AgentEvent, AgentEventKind as Kind};
use crate::delivery::codex_rollout::read_rollout_window;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexBinding {
    pub key: String,
    pub spawned_at: DateTime<Utc>,
    pub thread_id: String,
    pub rollout_path: PathBuf,
}

struct Discovery {
    key: String,
    spawned_at: DateTime<Utc>,
    cwd: String,
    next_scan: Instant,
    backoff: u64,
    rejected: HashSet<PathBuf>,
    matches: HashMap<PathBuf, String>,
    binding: Option<CodexBinding>,
}

pub struct CodexBinder {
    root: Option<PathBuf>,
    agents: HashMap<(String, String), Discovery>,
    #[cfg(test)]
    scans: usize,
    #[cfg(test)]
    opened: HashMap<PathBuf, usize>,
}

enum Probe {
    Match(String),
    Rejected,
    Pending,
}

pub(crate) fn prompt_key(text: &str) -> String {
    format!(
        "codex-prompt-{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, text.as_bytes())
    )
}

fn normalized_cwd(path: &str) -> String {
    let mut path = path.replace('\\', "/");
    while path.ends_with('/') && path.len() > 1 {
        path.pop();
    }
    if path.as_bytes().get(1) == Some(&b':') && path.as_bytes()[0].is_ascii_alphabetic() {
        path.replace_range(..1, &path[..1].to_ascii_lowercase());
    }
    path
}

fn filename_thread(path: &Path) -> Option<&str> {
    let name = path
        .file_name()?
        .to_str()?
        .strip_prefix("rollout-")?
        .strip_suffix(".jsonl")?;
    let id = name.get(name.len().checked_sub(36)?..)?;
    uuid::Uuid::parse_str(id).ok()?;
    Some(id)
}

/// Read physical line zero, never the first parseable/later session_meta.
fn probe(root: &Path, path: &Path, key: &str, cwd: &str, spawned_at: DateTime<Utc>) -> Probe {
    if feed::contained_file(root, path).is_err() {
        return Probe::Rejected;
    }
    let Ok(file) = File::open(path) else {
        return Probe::Pending;
    };
    let mut bytes = Vec::new();
    if file
        .take(feed::MAX_READ_BYTES)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Probe::Pending;
    }
    probe_bytes(path, &bytes, key, cwd, spawned_at)
}

fn probe_bytes(
    path: &Path,
    bytes: &[u8],
    key: &str,
    cwd: &str,
    spawned_at: DateTime<Utc>,
) -> Probe {
    let Some(thread) = filename_thread(path) else {
        return Probe::Rejected;
    };
    let Some(first_end) = bytes.iter().position(|byte| *byte == b'\n') else {
        return Probe::Pending;
    };
    let Ok(meta) = serde_json::from_slice::<Value>(&bytes[..first_end]) else {
        return Probe::Rejected;
    };
    let payload = &meta["payload"];
    let recent = payload["timestamp"]
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .is_some_and(|time| time >= spawned_at);
    if meta["type"] != "session_meta"
        || payload["source"] != "cli"
        || payload["id"] != thread
        || payload["cwd"]
            .as_str()
            .is_none_or(|held| normalized_cwd(held) != cwd)
        || !recent
    {
        return Probe::Rejected;
    }
    for line in bytes[first_end + 1..].split_inclusive(|byte| *byte == b'\n') {
        if line.last() != Some(&b'\n') {
            break;
        }
        let Ok(record) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if record["type"] != "response_item"
            || record["payload"]["type"] != "message"
            || record["payload"]["role"] != "user"
        {
            continue;
        }
        let text = message_text(&record["payload"]);
        if injected_user(&text) {
            continue;
        }
        // First real user item only; an exact prompt buried later proves nothing.
        return if prompt_key(text.trim()) == key {
            Probe::Match(thread.into())
        } else {
            Probe::Rejected
        };
    }
    Probe::Pending
}

/// Date buckets only, never recursive traversal through arbitrary directories.
fn rollout_files(root: &Path, spawned_at: DateTime<Utc>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(years) = fs::read_dir(root) else {
        return files;
    };
    for year in years.flatten().filter(|entry| real_directory(entry, 4)) {
        let Ok(months) = fs::read_dir(year.path()) else {
            continue;
        };
        for month in months.flatten().filter(|entry| real_directory(entry, 2)) {
            let Ok(days) = fs::read_dir(month.path()) else {
                continue;
            };
            for day in days.flatten().filter(|entry| real_directory(entry, 2)) {
                let date = format!(
                    "{}-{}-{}",
                    year.file_name().to_string_lossy(),
                    month.file_name().to_string_lossy(),
                    day.file_name().to_string_lossy()
                );
                let Ok(date) = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d") else {
                    continue;
                };
                // Codex uses local date buckets. Include the preceding UTC day.
                if date
                    < spawned_at
                        .date_naive()
                        .pred_opt()
                        .unwrap_or(spawned_at.date_naive())
                {
                    continue;
                }
                let Ok(entries) = fs::read_dir(day.path()) else {
                    continue;
                };
                files.extend(
                    entries
                        .flatten()
                        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                        .map(|entry| entry.path())
                        .filter(|path| filename_thread(path).is_some()),
                );
            }
        }
    }
    files
}

fn real_directory(entry: &fs::DirEntry, digits: usize) -> bool {
    let name = entry.file_name();
    let text = name.to_string_lossy();
    text.len() == digits
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && entry.file_type().is_ok_and(|kind| kind.is_dir())
}

impl CodexBinder {
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }
    pub fn new(root: Option<PathBuf>) -> Self {
        Self {
            root,
            agents: HashMap::new(),
            #[cfg(test)]
            scans: 0,
            #[cfg(test)]
            opened: HashMap::new(),
        }
    }
    pub fn set_root(&mut self, root: Option<PathBuf>) {
        self.root = root;
        self.agents.clear();
    }

    /// Caller holds no journals lock. Backoff and rejected-file caches belong to
    /// the held agent identity, so each SSE/history request shares one schedule.
    pub fn resolve(
        &mut self,
        session: &str,
        agent: &str,
        key: Option<&str>,
        spawned_at: Option<DateTime<Utc>>,
        cwd: &Path,
        sidecar: &Path,
        now: Instant,
        before_scan: impl FnOnce(),
    ) -> Option<CodexBinding> {
        let identity = (session.to_owned(), agent.to_owned());
        let Some((key, spawned_at)) = key.zip(spawned_at).filter(|(key, _)| {
            key.starts_with("codex-prompt-") && feed::safe_component(key).is_ok()
        }) else {
            self.agents.remove(&identity);
            remove_sidecar(sidecar);
            return None;
        };
        let root = self.root.as_ref()?;
        let cwd = normalized_cwd(&cwd.to_string_lossy());
        if self
            .agents
            .get(&identity)
            .is_some_and(|held| held.key != key || held.spawned_at != spawned_at || held.cwd != cwd)
        {
            self.agents.remove(&identity);
            remove_sidecar(sidecar);
        }
        if !self.agents.contains_key(&identity) {
            let mut rejected = HashSet::new();
            let binding = load_binding(sidecar)
                .filter(|binding| binding.key == key && binding.spawned_at == spawned_at)
                .and_then(|binding| {
                    #[cfg(test)]
                    {
                        *self.opened.entry(binding.rollout_path.clone()).or_default() += 1;
                    }
                    match probe(root, &binding.rollout_path, key, &cwd, spawned_at) {
                        Probe::Match(ref thread) if thread == &binding.thread_id => Some(binding),
                        Probe::Pending => None,
                        _ => {
                            rejected.insert(binding.rollout_path);
                            None
                        }
                    }
                });
            if binding.is_none() {
                remove_sidecar(sidecar);
            }
            self.agents.insert(
                identity.clone(),
                Discovery {
                    key: key.into(),
                    spawned_at,
                    cwd: cwd.clone(),
                    next_scan: now,
                    backoff: 5,
                    rejected,
                    matches: HashMap::new(),
                    binding,
                },
            );
        }
        let state = self.agents.get_mut(&identity)?;
        if state.binding.is_some() {
            return state.binding.clone();
        }
        if now < state.next_scan {
            return None;
        }
        before_scan();
        #[cfg(test)]
        {
            self.scans += 1;
        }
        state.next_scan = now + Duration::from_secs(state.backoff);
        state.backoff = (state.backoff * 2).min(60);
        for path in rollout_files(root, spawned_at) {
            if state.rejected.contains(&path) || state.matches.contains_key(&path) {
                continue;
            }
            #[cfg(test)]
            {
                *self.opened.entry(path.clone()).or_default() += 1;
            }
            match probe(root, &path, key, &cwd, spawned_at) {
                Probe::Rejected => {
                    state.rejected.insert(path);
                }
                Probe::Match(thread) => {
                    state.matches.insert(path, thread);
                }
                Probe::Pending => {}
            }
        }
        if state.matches.len() != 1 {
            return None;
        }
        let (path, thread) = state.matches.iter().next()?;
        let binding = CodexBinding {
            key: key.into(),
            spawned_at,
            thread_id: thread.clone(),
            rollout_path: path.clone(),
        };
        save_binding(sidecar, &binding)?;
        state.binding = Some(binding.clone());
        Some(binding)
    }
}

fn load_binding(path: &Path) -> Option<CodexBinding> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(64 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save_binding(path: &Path, binding: &CodexBinding) -> Option<()> {
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink() || !meta.is_file())
    {
        return None;
    }
    let mut temp = tempfile::NamedTempFile::new_in(path.parent()?).ok()?;
    serde_json::to_writer(temp.as_file_mut(), binding).ok()?;
    temp.persist(path).ok()?;
    Some(())
}

fn remove_sidecar(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file() && !meta.file_type().is_symlink())
    {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;
    fn spawn() -> DateTime<Utc> {
        "2026-11-01T12:00:00Z".parse().unwrap()
    }
    fn id(index: u8) -> String {
        format!("00000000-0000-4000-8000-{index:012}")
    }
    fn prompt(seat: &str) -> String {
        format!("Read C:/fixture/{seat}-prompt.md and execute.")
    }
    fn meta(index: u8, source: Value, timestamp: &str, cwd: &str) -> Value {
        json!({"type":"session_meta","payload":{"id":id(index),"source":source,"timestamp":timestamp,"cwd":cwd}})
    }
    fn user(text: &str) -> Value {
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}})
    }
    fn write(root: &Path, index: u8, records: &[Value]) -> PathBuf {
        let dir = root.join("2026/11/01");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("rollout-2026-11-01T12-00-00-{}.jsonl", id(index)));
        fs::write(
            &path,
            records
                .iter()
                .map(|record| format!("{record}\n"))
                .collect::<String>(),
        )
        .unwrap();
        path
    }
    fn resolve(
        binder: &mut CodexBinder,
        temp: &Path,
        seat: &str,
        now: Instant,
    ) -> Option<CodexBinding> {
        binder.resolve(
            "session",
            seat,
            Some(&prompt_key(&prompt(seat))),
            Some(spawn()),
            Path::new("c:\\fixture\\"),
            &temp.join(format!("{seat}.binding")),
            now,
            || {},
        )
    }

    #[test]
    fn g16_same_second_same_cwd_worker_and_queen_bind_only_their_first_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("rollouts");
        for (index, seat) in [(1, "worker-a"), (2, "worker-b"), (3, "queen")] {
            let mut records = vec![meta(
                index,
                json!("cli"),
                "2026-11-01T12:00:00.000Z",
                "C:/fixture/",
            )];
            // A later meta must not change the identity of a valid physical line 0.
            if index == 1 {
                records.push(meta(99, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"));
            }
            records.extend([
                user("# AGENTS.md instructions\nfixture"),
                user("<environment_context>fixture</environment_context>"),
                user(&prompt(seat)),
            ]);
            write(&root, index, &records);
        }
        // Actual child layout: its own subagent meta, then its parent's cli meta and exact prompt.
        write(
            &root,
            4,
            &[
                meta(
                    4,
                    json!({"subagent":{"thread_spawn":{}}}),
                    "2026-11-01T12:00:00Z",
                    "C:/fixture",
                ),
                meta(1, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                user(&prompt("worker-a")),
            ],
        );
        write(
            &root,
            5,
            &[
                meta(5, json!("cli"), "2026-11-01T11:59:59.999Z", "C:/fixture"),
                user(&prompt("worker-a")),
            ],
        );
        write(
            &root,
            6,
            &[
                meta(6, json!("vscode"), "2026-11-01T12:00:00Z", "C:/fixture"),
                user(&prompt("worker-a")),
            ],
        );
        write(
            &root,
            7,
            &[
                meta(8, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                user(&prompt("worker-a")),
            ],
        );
        write(
            &root,
            9,
            &[
                meta(9, json!("cli"), "2026-11-01T12:00:00Z", "C:/other"),
                user(&prompt("worker-a")),
            ],
        );
        write(
            &root,
            10,
            &[
                meta(10, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                user("# an ordinary operator message"),
                user(&prompt("worker-a")),
            ],
        );
        let mut binder = CodexBinder::new(Some(root.clone()));
        let now = Instant::now();
        for (index, seat) in [(1, "worker-a"), (2, "worker-b"), (3, "queen")] {
            let binding = resolve(&mut binder, temp.path(), seat, now)
                .expect("G16 unique exact prompt must bind");
            assert_eq!(
                binding.thread_id,
                id(index),
                "G16 same-second same-cwd agents never cross-bind"
            );
            assert_eq!(
                load_binding(&temp.path().join(format!("{seat}.binding"))),
                Some(binding)
            );
        }
        // A child with the parent's repeated cli meta is never enough by itself.
        fs::remove_file(root.join(format!(
            "2026/11/01/rollout-2026-11-01T12-00-00-{}.jsonl",
            id(1)
        )))
        .unwrap();
        let mut child_only = CodexBinder::new(Some(root));
        assert!(
            resolve(&mut child_only, temp.path(), "worker-a", now).is_none(),
            "G16 later parent meta must not bind a subagent"
        );
    }

    #[test]
    fn g16_ambiguity_missing_identity_and_wrong_first_user_bind_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("rollouts");
        for index in [1, 2] {
            write(
                &root,
                index,
                &[
                    meta(index, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                    user(&prompt("worker-a")),
                ],
            );
        }
        let now = Instant::now();
        let mut binder = CodexBinder::new(Some(root.clone()));
        assert!(
            resolve(&mut binder, temp.path(), "worker-a", now).is_none(),
            "G16 ambiguity stays terminal-only"
        );
        let sidecar = temp.path().join("none.binding");
        assert!(binder
            .resolve(
                "session",
                "none",
                Some(&prompt_key(&prompt("worker-a"))),
                None,
                Path::new("C:/fixture"),
                &sidecar,
                now,
                || {}
            )
            .is_none());
        assert!(binder
            .resolve(
                "session",
                "none",
                None,
                Some(spawn()),
                Path::new("C:/fixture"),
                &sidecar,
                now,
                || {}
            )
            .is_none());
        for first in [
            "# operator text",
            "<operator>text</operator>",
            "prefix Read C:/fixture/worker-a-prompt.md and execute.",
        ] {
            let path = write(
                &root,
                3,
                &[
                    meta(3, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                    user(first),
                    user(&prompt("worker-a")),
                ],
            );
            assert!(matches!(
                probe(
                    &root,
                    &path,
                    &prompt_key(&prompt("worker-a")),
                    "c:/fixture",
                    spawn()
                ),
                Probe::Rejected
            ));
        }
        let path = write(
            &root,
            4,
            &[
                meta(4, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                user(&prompt("worker-a")),
            ],
        );
        let contents = fs::read(&path).unwrap();
        fs::write(&path, [b"\n".as_slice(), &contents].concat()).unwrap();
        assert!(
            matches!(
                probe(
                    &root,
                    &path,
                    &prompt_key(&prompt("worker-a")),
                    "c:/fixture",
                    spawn()
                ),
                Probe::Rejected
            ),
            "physical line zero is required"
        );
    }

    #[test]
    fn g16b_backoff_caps_at_sixty_seconds_and_rejected_files_never_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("rollouts");
        let rejected = write(
            &root,
            1,
            &[
                meta(1, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                user(&prompt("other")),
            ],
        );
        let mut binder = CodexBinder::new(Some(root.clone()));
        let now = Instant::now();
        assert!(resolve(&mut binder, temp.path(), "worker-a", now).is_none());
        for millis in [1, 250, 1000, 4999] {
            assert!(resolve(
                &mut binder,
                temp.path(),
                "worker-a",
                now + Duration::from_millis(millis)
            )
            .is_none());
        }
        assert_eq!(binder.scans, 1, "G16b 250ms configure must not rescan");
        // A rejected path remains rejected even if rewritten into a qualifying file.
        write(
            &root,
            1,
            &[
                meta(1, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                user(&prompt("worker-a")),
            ],
        );
        for seconds in [5, 15, 35, 75, 135, 195] {
            assert!(resolve(
                &mut binder,
                temp.path(),
                "worker-a",
                now + Duration::from_secs(seconds)
            )
            .is_none());
        }
        assert_eq!(binder.scans, 7);
        assert_eq!(
            binder.opened[&rejected], 1,
            "G16b rejected file never reopened"
        );
        assert_eq!(
            binder.agents[&("session".into(), "worker-a".into())].backoff,
            60
        );
        write(
            &root,
            2,
            &[
                meta(2, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                user(&prompt("worker-a")),
            ],
        );
        assert!(resolve(
            &mut binder,
            temp.path(),
            "worker-a",
            now + Duration::from_secs(254)
        )
        .is_none());
        assert_eq!(
            resolve(
                &mut binder,
                temp.path(),
                "worker-a",
                now + Duration::from_secs(255)
            )
            .unwrap()
            .thread_id,
            id(2)
        );
    }

    #[test]
    fn g16b_cold_sidecar_restores_exact_identity_and_respawn_discards_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("rollouts");
        write(
            &root,
            1,
            &[
                meta(1, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                user(&prompt("worker-a")),
            ],
        );
        let now = Instant::now();
        let mut first = CodexBinder::new(Some(root.clone()));
        let binding = resolve(&mut first, temp.path(), "worker-a", now).unwrap();
        let mut cold = CodexBinder::new(Some(root));
        assert_eq!(
            resolve(&mut cold, temp.path(), "worker-a", now),
            Some(binding)
        );
        assert_eq!(
            cold.scans, 0,
            "G16b cold binding reads its exact path without a directory scan"
        );
        let sidecar = temp.path().join("worker-a.binding");
        assert!(cold
            .resolve(
                "session",
                "worker-a",
                Some(&prompt_key(&prompt("worker-a"))),
                Some(spawn() + chrono::Duration::seconds(1)),
                Path::new("C:/fixture"),
                &sidecar,
                now,
                || {}
            )
            .is_none());
        assert!(!sidecar.exists(), "G16b respawn discards stale sidecar");
    }

    #[test]
    fn g16b_rejected_cold_sidecar_is_not_opened_again_by_discovery() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("rollouts");
        let path = write(
            &root,
            1,
            &[
                meta(1, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                user(&prompt("worker-b")),
            ],
        );
        save_binding(
            &temp.path().join("worker-a.binding"),
            &CodexBinding {
                key: prompt_key(&prompt("worker-a")),
                spawned_at: spawn(),
                thread_id: id(1),
                rollout_path: path.clone(),
            },
        )
        .unwrap();
        let mut binder = CodexBinder::new(Some(root));
        let now = Instant::now();
        assert!(resolve(&mut binder, temp.path(), "worker-a", now).is_none());
        assert!(resolve(
            &mut binder,
            temp.path(),
            "worker-a",
            now + Duration::from_secs(5)
        )
        .is_none());
        assert_eq!(
            binder.opened[&path], 1,
            "G16b rejected cold sidecar is cached before scans"
        );
    }

    #[test]
    fn g16_bound_source_rejects_replaced_foreign_rollout_before_reset_or_emission() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("rollouts");
        let path = write(
            &root,
            1,
            &[
                meta(1, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                user(&prompt("worker-a")),
            ],
        );
        let mut binder = CodexBinder::new(Some(root.clone()));
        let binding = resolve(&mut binder, temp.path(), "worker-a", Instant::now()).unwrap();
        let mut source =
            CodexRolloutSource::bound(root.clone(), &binding, Path::new("C:/fixture"), "worker-a")
                .unwrap();
        assert_eq!(source.poll().unwrap().0.len(), 1);
        let offset = source.offset();
        // Same path now contains a child's identity and the parent's copied meta/prompt.
        fs::write(
            &path,
            format!(
                "{}\n{}\n{}\n",
                meta(
                    2,
                    json!({"subagent":{}}),
                    "2026-11-01T12:00:00Z",
                    "C:/fixture"
                ),
                meta(1, json!("cli"), "2026-11-01T12:00:00Z", "C:/fixture"),
                user(&prompt("worker-a"))
            ),
        )
        .unwrap();
        assert!(
            source.poll().is_err(),
            "G16 replacement must be rejected before foreign records are emitted"
        );
        assert_eq!(
            source.offset(),
            offset,
            "G16 invalid replacement must not reset source progress"
        );
        assert!(
            CodexRolloutSource::bound(root, &binding, Path::new("C:/fixture"), "worker-a").is_err()
        );
    }
}

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
    held: Option<(CodexBinding, String)>,
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
    pub fn matches_binding(&self, binding: &CodexBinding) -> bool {
        self.held.as_ref().is_some_and(|(held, _)| held == binding)
    }
    fn new(root: PathBuf, path: PathBuf, agent_id: &str) -> Result<Self, String> {
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
            held: None,
        })
    }

    pub fn bound(
        root: PathBuf,
        binding: &CodexBinding,
        cwd: &Path,
        agent: &str,
    ) -> Result<Self, String> {
        let cwd = normalized_cwd(&cwd.to_string_lossy());
        if !matches!(probe(&root, &binding.rollout_path, &binding.key, &cwd, binding.spawned_at),
            Probe::Match(ref id) if id == &binding.thread_id)
        {
            return Err("Bound rollout identity is unavailable".into());
        }
        let mut source = Self::new(root, binding.rollout_path.clone(), agent)?;
        source.held = Some((binding.clone(), cwd));
        Ok(source)
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
        BufReader::new((&mut file).take(feed::MAX_READ_BYTES))
            .read_until(b'\n', &mut prefix)
            .map_err(|error| error.to_string())?;
        let reset = metadata.len() < self.window.offset
            || (self.created.is_some() && created != self.created)
            || (!self.prefix.is_empty() && prefix != self.prefix);
        if self.window.offset == 0 || reset {
            if let Some((binding, cwd)) = &self.held {
                file.seek(SeekFrom::Start(0))
                    .map_err(|error| error.to_string())?;
                let mut bytes = Vec::new();
                (&mut file)
                    .take(feed::MAX_READ_BYTES)
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                if !matches!(probe_bytes(&self.path, &bytes, &binding.key, cwd, binding.spawned_at),
                    Probe::Match(ref id) if id == &binding.thread_id)
                {
                    return Err("Bound rollout identity changed".into());
                }
            }
        }
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
