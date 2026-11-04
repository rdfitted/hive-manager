//! A bounded JSONL client. No home inspection or token handling in Hive.
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

pub const PINNED_VERSION: &str = "0.166.0";
pub const PINNED_SCHEMA_SHA256: &str =
    "5ea4928c2186a75de0ca9f7cc66f682a348f24fdda35d901e24298f87c350620";
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_RPC_BYTES: u64 = 4 * 1024 * 1024;
type Reply = Result<Value, String>;

pub(crate) fn remove_parent_and_billing_env(
    command: &mut Command,
    inherited: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) {
    for (name, _) in inherited {
        let key = name.to_string_lossy();
        if crate::adapters::wsl::is_parent_session_env(&key)
            || key.eq_ignore_ascii_case("OPENAI_API_KEY")
            || key.eq_ignore_ascii_case("CODEX_API_KEY")
        {
            command.env_remove(name);
        }
    }
    // Also remove exact names when an inherited snapshot was supplied by a test.
    command
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY");
}

/// Resolve an npm shim without a shell when its native package is available.
/// Custom .cmd commands still work through std::process, and are tree-killed.
pub(crate) fn resolve_executable(command: &str) -> PathBuf {
    if !Path::new(command)
        .file_stem()
        .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case("codex"))
    {
        return command.into();
    }
    #[cfg(windows)]
    {
        let given = PathBuf::from(command);
        let roots: Vec<PathBuf> = if given.is_absolute() {
            given.parent().into_iter().map(Path::to_path_buf).collect()
        } else {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect()
        };
        for root in roots {
            let native = root.join("node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe");
            if native.is_file() {
                return native;
            }
            let native = root.join(command).with_extension("exe");
            if native.is_file() {
                return native;
            }
            let shim = root.join(command).with_extension("cmd");
            if shim.is_file() {
                return shim;
            }
        }
    }
    PathBuf::from(command)
}

pub(crate) struct RpcClient {
    child: Mutex<Option<Child>>,
    tree: Mutex<Option<crate::transport::process::ProcessTree>>,
    stdin: Mutex<Option<mpsc::SyncSender<Vec<u8>>>>,
    pending: Mutex<HashMap<u64, mpsc::SyncSender<Reply>>>,
    next: AtomicU64,
    pub alive: AtomicBool,
    cancelled: AtomicBool,
}

impl RpcClient {
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
        if self.cancelled.load(Ordering::Acquire) {
            return Err("Startup cancelled".into());
        }
        let mut cmd = Command::new(resolve_executable(command));
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let inherited: Vec<_> = std::env::vars_os().collect();
        for (name, _) in &inherited {
            if name
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("HIVE_")
            {
                cmd.env_remove(name);
            }
        }
        remove_parent_and_billing_env(&mut cmd, inherited);
        cmd.envs(env.iter().map(|(k, v)| (k, v)));
        remove_parent_and_billing_env(&mut cmd, env.iter().map(|(k, v)| (k.into(), v.into())));
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
        // Hold the process slot across spawn so stop cannot miss a newly created child.
        let mut slot = self.child.lock();
        if self.cancelled.load(Ordering::Acquire) {
            return Err("Startup cancelled".into());
        }
        let mut child = cmd.spawn().map_err(|e| {
            format!("Cannot start Codex app-server: {e}. Select PTY in launch settings.")
        })?;
        match crate::transport::process::ProcessTree::attach(&child) {
            Ok(tree) => *self.tree.lock() = Some(tree),
            Err(error) => {
                // Root is still suspended, so no descendant exists. Reap it
                // directly instead of depending on a second taskkill process.
                if let Err(cleanup) = child.kill().and_then(|_| child.wait().map(|_| ())) {
                    *slot = Some(child);
                    return Err(format!("{error}; suspended-root cleanup failed: {cleanup}"));
                }
                return Err(error);
            }
        }
        let stdout = child.stdout.take().ok_or("Codex stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("Codex stderr unavailable")?;
        let mut pipe = child.stdin.take().ok_or("Codex stdin unavailable")?;
        let (writer_tx, writer_rx) = mpsc::sync_channel::<Vec<u8>>(16);
        *self.stdin.lock() = Some(writer_tx);
        *slot = Some(child);
        self.alive.store(true, Ordering::Release);
        drop(slot);
        let monitor = self.clone();
        std::thread::Builder::new()
            .name("codex-liveness".into())
            .spawn(move || {
                while monitor.alive.load(Ordering::Acquire) {
                    let exited = monitor
                        .child
                        .lock()
                        .as_mut()
                        .is_none_or(|child| crate::transport::process::child_has_exited(child).unwrap_or(false));
                    if exited {
                        monitor.alive.store(false, Ordering::Release);
                        monitor.fail_pending("Codex app-server exited");
                        if let Some(tree) = monitor.tree.lock().as_ref() {
                            let _ = tree.terminate();
                        }
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .map_err(|e| e.to_string())?;
        let writer_client = self.clone();
        std::thread::Builder::new()
            .name("codex-writer".into())
            .spawn(move || {
                for bytes in writer_rx {
                    if pipe.write_all(&bytes).and_then(|_| pipe.flush()).is_err() {
                        writer_client.alive.store(false, Ordering::Release);
                        writer_client.fail_pending("Codex input closed");
                        let _ = writer_client.stop();
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        std::thread::Builder::new()
            .name("codex-stderr".into())
            .spawn(move || {
                // Drain without retaining possibly sensitive stderr or unbounded lines.
                let _ = std::io::copy(&mut BufReader::new(stderr), &mut std::io::sink());
            })
            .map_err(|e| e.to_string())?;
        let client = self.clone();
        std::thread::Builder::new().name("codex-jsonl".into()).spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                let read = reader.by_ref().take(MAX_RPC_BYTES + 1).read_until(b'\n', &mut bytes);
                match read {
                    Ok(0) | Err(_) => break,
                    _ if bytes.len() as u64 > MAX_RPC_BYTES => break,
                    _ => {}
                }
                let Ok(message) = serde_json::from_slice::<Value>(&bytes) else { break; };
                if message.get("method").is_some() { on_message(message); }
                else if let Some(id) = message.get("id").and_then(Value::as_u64) {
                    if let Some(tx) = client.pending.lock().remove(&id) {
                        let reply = if message.get("error").is_some() { Err("Codex rejected the protocol request; inspect the raw event log or select PTY.".into()) }
                            else { message.get("result").cloned().ok_or_else(|| "Missing Codex response result".into()) };
                        let _ = tx.send(reply);
                    }
                }
            }
            client.alive.store(false, Ordering::Release);
            client.fail_pending("Codex app-server exited or sent invalid/oversized JSON. Select PTY to retry.");
            let _=client.stop();
        }).map_err(|e| e.to_string())?;
        Ok(())
    }
    fn fail_pending(&self, reason: &str) {
        for (_, tx) in self.pending.lock().drain() {
            let _ = tx.send(Err(reason.into()));
        }
    }
    pub fn write(&self, value: Value) -> Result<(), String> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err("Codex transport stopped".into());
        }
        let stdin = self.stdin.lock();
        let pipe = stdin.as_ref().ok_or("Codex transport has no stdin")?;
        let mut bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        pipe.try_send(bytes)
            .map_err(|_| "Codex input is blocked or closed; retry with PTY".into())
    }
    pub fn request(&self, method: &str, params: Value, deadline: Instant) -> Reply {
        if !self.alive.load(Ordering::Acquire) {
            return Err("Codex app-server is not alive".into());
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(1);
        self.pending.lock().insert(id, tx);
        if let Err(e) = self.write(json!({"id":id,"method":method,"params":params})) {
            self.pending.lock().remove(&id);
            return Err(e);
        }
        let result = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|_| Err(format!("Codex {method} exceeded its deadline. Select PTY in launch settings and retry.")));
        self.pending.lock().remove(&id);
        result
    }
    pub fn stop(&self) -> Result<(), String> {
        self.stop_with_cleanup(crate::transport::process::stop_child)
    }

    pub(super) fn stop_with_cleanup(&self, cleanup: impl FnOnce(&mut Child, Option<&crate::transport::process::ProcessTree>) -> Result<(), String>) -> Result<(), String> {
        self.cancelled.store(true, Ordering::Release);
        self.alive.store(false, Ordering::Release);
        self.fail_pending("Codex transport cancelled");
        let mut slot = self.child.lock();
        if let Some(child) = slot.as_mut() {
            let tree = self.tree.lock();
            cleanup(child, tree.as_ref())?;
            *slot = None;
        }
        self.tree.lock().take();
        self.stdin.lock().take();
        Ok(())
    }
}
