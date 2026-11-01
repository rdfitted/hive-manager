//! Bounded raw CLI JSONL; credentials stay inside the unmodified Claude binary.
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

pub const PINNED_VERSION: &str = "2.1.316";
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_FRAME_BYTES: u64 = 4 * 1024 * 1024;
type Reply = Result<Value, String>;

pub(crate) fn require_subscription_env(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<(), String> {
    for (name, _) in vars {
        let key = name.to_string_lossy().to_ascii_uppercase();
        if matches!(
            key.as_str(),
            "ANTHROPIC_API_KEY"
                | "ANTHROPIC_AUTH_TOKEN"
                | "ANTHROPIC_PROFILE"
                | "ANTHROPIC_BASE_URL"
                | "ANTHROPIC_FEDERATION_RULE_ID"
                | "ANTHROPIC_ORGANIZATION_ID"
        ) || key.starts_with("CLAUDE_CODE_USE_")
        {
            return Err(format!("Claude stream-json refuses {key} to prevent accidental API/provider billing. Remove it for this launch or select PTY."));
        }
    }
    Ok(())
}

pub(crate) fn remove_parent_env(
    command: &mut Command,
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) {
    for (name, _) in vars {
        if crate::adapters::wsl::is_parent_session_env(&name.to_string_lossy()) {
            command.env_remove(name);
        }
    }
}

/// Only the account shape measured in T29 is allowed. Unknown accounts fail closed.
pub(crate) fn require_subscription(response: &Value) -> Result<(), String> {
    let account = &response["account"];
    let no_api = account["apiKeySource"].is_null() || account["apiKeySource"] == "none";
    let no_override = account["tokenSource"].is_null() || account["tokenSource"] == "claude.ai";
    if account["subscriptionType"] == "Claude Ultra" && no_api && no_override {
        Ok(())
    } else {
        Err("Claude stream-json requires the existing subscription login verified before the first turn. This experimental version recognizes the measured Claude Ultra account only; unknown/API/provider authentication is refused. Run claude auth login or select PTY.".into())
    }
}

pub(crate) struct JsonClient {
    child: Mutex<Option<Child>>,
    tree: Mutex<Option<crate::transport::process::ProcessTree>>,
    stdin: Mutex<Option<mpsc::SyncSender<Vec<u8>>>>,
    pending: Mutex<HashMap<String, mpsc::SyncSender<Reply>>>,
    next: AtomicU64,
    pub alive: AtomicBool,
    cancelled: AtomicBool,
}

impl JsonClient {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            child: Mutex::new(None),
            tree: Mutex::new(None),
            stdin: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            alive: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
        })
    }
    pub fn spawn(
        self: &Arc<Self>,
        command: &str,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
        on_message: Arc<dyn Fn(Value) + Send + Sync>,
    ) -> Result<(), String> {
        let inherited: Vec<_> = std::env::vars_os().collect();
        require_subscription_env(inherited.clone())?;
        require_subscription_env(env.iter().map(|(k, v)| (k.into(), v.into())))?;
        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, _) in &inherited {
            if name
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("HIVE_")
            {
                cmd.env_remove(name);
            }
        }
        remove_parent_env(&mut cmd, inherited);
        cmd.envs(env.iter().map(|(k, v)| (k, v)));
        remove_parent_env(&mut cmd, env.iter().map(|(k, v)| (k.into(), v.into())));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000 | 0x0000_0004);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut slot = self.child.lock();
        if self.cancelled.load(Ordering::Acquire) {
            return Err("Claude startup cancelled".into());
        }
        let mut child = cmd.spawn().map_err(|e| {
            format!("Cannot start Claude stream-json: {e}. Select PTY in launch settings.")
        })?;
        match crate::transport::process::ProcessTree::attach(&child) {
            Ok(tree) => *self.tree.lock() = Some(tree),
            Err(error) => {
                if let Err(cleanup) = child.kill().and_then(|_| child.wait().map(|_| ())) {
                    *slot = Some(child);
                    return Err(format!("{error}; suspended-root cleanup failed: {cleanup}"));
                }
                return Err(error);
            }
        }
        let stdout = child.stdout.take().ok_or("Claude stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("Claude stderr unavailable")?;
        let mut pipe = child.stdin.take().ok_or("Claude stdin unavailable")?;
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(16);
        *self.stdin.lock() = Some(tx);
        *slot = Some(child);
        self.alive.store(true, Ordering::Release);
        drop(slot);
        let monitor = self.clone();
        std::thread::Builder::new()
            .name("claude-liveness".into())
            .spawn(move || {
                while monitor.alive.load(Ordering::Acquire) {
                    let exited = monitor
                        .child
                        .lock()
                        .as_mut()
                        .is_none_or(|c| c.try_wait().ok().flatten().is_some());
                    if exited {
                        monitor.alive.store(false, Ordering::Release);
                        monitor.fail_pending("Claude stream-json exited; select PTY to restart");
                        if let Some(tree) = monitor.tree.lock().as_ref() {
                            let _ = tree.terminate();
                        }
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .map_err(|e| e.to_string())?;
        let writer = self.clone();
        std::thread::Builder::new()
            .name("claude-writer".into())
            .spawn(move || {
                for bytes in rx {
                    if pipe.write_all(&bytes).and_then(|_| pipe.flush()).is_err() {
                        writer.alive.store(false, Ordering::Release);
                        writer.fail_pending("Claude input closed; select PTY");
                        let _ = writer.stop();
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        std::thread::Builder::new()
            .name("claude-stderr".into())
            .spawn(move || {
                let _ = std::io::copy(&mut BufReader::new(stderr), &mut std::io::sink());
            })
            .map_err(|e| e.to_string())?;
        let client = self.clone();
        std::thread::Builder::new().name("claude-jsonl".into()).spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                match reader.by_ref().take(MAX_FRAME_BYTES + 1).read_until(b'\n', &mut bytes) {
                    Ok(0) | Err(_) => break,
                    _ if bytes.len() as u64 > MAX_FRAME_BYTES => break,
                    _ => {}
                }
                let Ok(message) = serde_json::from_slice::<Value>(&bytes) else { break; };
                if message["type"] == "control_response" {
                    let response = &message["response"];
                    if let Some(id) = response["request_id"].as_str() {
                        if let Some(tx) = client.pending.lock().remove(id) {
                            let reply = if response["subtype"] == "success" {
                                Ok(response["response"].clone())
                            } else { Err("Claude rejected a control request; select PTY for this configuration".into()) };
                            let _ = tx.send(reply);
                        }
                    }
                } else { on_message(message); }
            }
            client.alive.store(false, Ordering::Release);
            client.fail_pending("Claude exited or sent invalid/oversized JSON. Select PTY to retry.");
            let _ = client.stop();
        }).map_err(|e| e.to_string())?;
        Ok(())
    }
    fn fail_pending(&self, reason: &str) {
        for (_, tx) in self.pending.lock().drain() {
            let _ = tx.send(Err(reason.into()));
        }
    }
    pub fn write(&self, value: Value) -> Result<(), String> {
        if self.cancelled.load(Ordering::Acquire) || !self.alive.load(Ordering::Acquire) {
            return Err("Claude stream-json is stopped; select PTY".into());
        }
        let mut bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_FRAME_BYTES {
            return Err("Claude input exceeds protocol limit".into());
        }
        bytes.push(b'\n');
        self.stdin
            .lock()
            .as_ref()
            .ok_or("Claude has no stdin")?
            .try_send(bytes)
            .map_err(|_| "Claude input is blocked or closed; retry with PTY".into())
    }
    pub fn request(&self, request: Value, deadline: Instant) -> Reply {
        let id = format!("hive-control-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = mpsc::sync_channel(1);
        self.pending.lock().insert(id.clone(), tx);
        if let Err(error) =
            self.write(json!({"type":"control_request","request_id":id,"request":request}))
        {
            self.pending.lock().remove(&id);
            return Err(error);
        }
        let result = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|_| Err("Claude startup/control deadline exceeded (startup allows 180 seconds). Check claude auth login or select PTY in launch settings.".into()));
        self.pending.lock().remove(&id);
        result
    }
    pub fn stop(&self) -> Result<(), String> {
        self.cancelled.store(true, Ordering::Release);
        self.alive.store(false, Ordering::Release);
        self.fail_pending("Claude transport cancelled");
        let mut slot = self.child.lock();
        if let Some(child) = slot.as_mut() {
            if let Some(tree) = self.tree.lock().as_ref() {
                tree.terminate()?;
            }
            if self.tree.lock().is_none() && child.try_wait().map_err(|e| e.to_string())?.is_none()
            {
                child.kill().map_err(|e| e.to_string())?;
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while child.try_wait().map_err(|e| e.to_string())?.is_none() {
                if Instant::now() >= deadline {
                    return Err("Claude cleanup timed out; retry stop".into());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            *slot = None;
        }
        self.tree.lock().take();
        self.stdin.lock().take();
        Ok(())
    }
}
