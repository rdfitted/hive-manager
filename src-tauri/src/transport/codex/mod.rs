//! Opt-in Codex app-server integration (experimental, behavioral confinement only).
pub mod client;
mod correlate;
mod events;
mod process;
#[cfg(test)]
mod tests;

use crate::delivery::Delivery;
use crate::transport::{
    AgentEvent, AgentEventKind, AgentTransport, Capabilities, EventSource, TransportRegistry,
};
use async_trait::async_trait;
use parking_lot::{Condvar, Mutex};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub struct CodexOptions {
    pub command: String,
    pub server_args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub initial_prompt: Option<String>,
    pub session_id: String,
    pub agent_id: String,
}

impl CodexOptions {
    pub fn from_pty(
        command: &str,
        args: &[&str],
        cwd: PathBuf,
        env: Vec<(String, String)>,
        session_id: String,
        agent_id: String,
    ) -> Result<Self, String> {
        let store = crate::cli::agent_store::store_dir_for(&agent_id);
        crate::cli::agent_store::ensure_store_dir(&store)
            .map_err(|e| format!("Cannot create isolated Codex store: {e}"))?;
        let mut server_args = crate::cli::agent_store::isolation_args(command, &store);
        let mut model = None;
        let mut effort = None;
        let mut initial_prompt = None;
        let mut index = 0;
        while index < args.len() {
            match args[index] {
                "--dangerously-bypass-approvals-and-sandbox" => {},
                "-m" | "--model" => { index+=1; model=Some(args.get(index).ok_or("Missing Codex model")?.to_string()); },
                "-c" | "--config" | "--enable" | "--disable" | "-p" | "--profile" => {
                    let flag=args[index]; index+=1;
                    let value=args.get(index).ok_or("Missing Codex option value")?;
                    if let Some(value)=value.strip_prefix("model_reasoning_effort=") { effort=Some(value.trim_matches('"').into()); }
                    server_args.extend([flag.to_string(),value.to_string()]);
                },
                text if !text.starts_with('-') && index+1==args.len() => initial_prompt=Some(text.into()),
                other => return Err(format!("Codex app-server cannot preserve option {other:?}; select PTY for this launch.")),
            }
            index += 1;
        }
        server_args.extend(["app-server".into(), "--listen".into(), "stdio://".into()]);
        Ok(Self {
            command: command.into(),
            server_args,
            cwd,
            env,
            model,
            effort,
            initial_prompt,
            session_id,
            agent_id,
        })
    }
}

struct Ticket {
    delivery: Mutex<Option<Result<Delivery, String>>>,
    ready: Condvar,
}
impl Ticket {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            delivery: Mutex::new(None),
            ready: Condvar::new(),
        })
    }
    fn finish(&self, delivery: Result<Delivery, String>) {
        *self.delivery.lock() = Some(delivery);
        self.ready.notify_all();
    }
    fn wait(&self) -> Result<Delivery, String> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut result = self.delivery.lock();
        while result.is_none() && Instant::now() < deadline {
            self.ready.wait_for(
                &mut result,
                deadline.saturating_duration_since(Instant::now()),
            );
        }
        result
            .clone()
            .unwrap_or_else(|| Ok(Delivery::protocol_pending()))
    }
}
struct Queued {
    text: String,
    ticket: Arc<Ticket>,
}
#[derive(Default)]
struct State {
    thread: Option<String>,
    children: HashSet<String>,
    staged: String,
    queue: VecDeque<Queued>,
    current: Option<correlate::TurnCorrelation>,
    active_turn: Option<String>,
    buffered: VecDeque<AgentEvent>,
    ready: bool,
    stopped: bool,
}
struct Core {
    options: CodexOptions,
    client: Arc<client::RpcClient>,
    registry: Weak<TransportRegistry>,
    state: Mutex<State>,
    changed: Condvar,
    startup: Mutex<()>,
    events: broadcast::Sender<AgentEvent>,
}
impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.client.stop();
    }
}

#[derive(Clone)]
pub struct CodexTransport {
    core: Arc<Core>,
}
impl CodexTransport {
    pub fn new(options: CodexOptions, registry: &Arc<TransportRegistry>) -> Arc<Self> {
        let (events, _) = broadcast::channel(256);
        Arc::new(Self {
            core: Arc::new(Core {
                options,
                client: client::RpcClient::new(),
                registry: Arc::downgrade(registry),
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
                startup: Mutex::new(()),
                events,
            }),
        })
    }
    pub fn thread_id(&self) -> Option<String> {
        self.core.state.lock().thread.clone()
    }
    pub fn initial_prompt(&self) -> Option<String> {
        self.core.options.initial_prompt.clone()
    }
    pub fn start_blocking(&self) -> Result<(), String> {
        tracing::debug!(
            version = client::PINNED_VERSION,
            schema = client::PINNED_SCHEMA_SHA256,
            "Pinned Codex protocol contract"
        );
        let _startup = self.core.startup.lock();
        if self.thread_id().is_some() {
            return if self.is_alive() {
                Ok(())
            } else {
                Err("Codex transport stopped; select PTY to restart".into())
            };
        }
        let deadline = Instant::now() + client::STARTUP_TIMEOUT;
        let weak = Arc::downgrade(&self.core);
        let handler = Arc::new(move |value| {
            if let Some(core) = weak.upgrade() {
                core.notification(value);
            }
        });
        let result = (|| {
            self.core.client.spawn(
                &self.core.options.command,
                &self.core.options.server_args,
                &self.core.options.cwd,
                &self.core.options.env,
                handler,
            )?;
            let init=self.core.client.request("initialize",json!({"clientInfo":{"name":"hive_manager","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),deadline)?;
            self.core
                .client
                .write(json!({"method":"initialized","params":{}}))?;
            if !init["userAgent"]
                .as_str()
                .unwrap_or("")
                .contains(client::PINNED_VERSION)
            {
                self.core.emit(events::event(&self.core.options.agent_id,AgentEventKind::Status,None,json!({"status":"version_mismatch","message":format!("Codex app-server tested with {}; installed version differs. Select PTY if protocol behavior changes.",client::PINNED_VERSION)})));
            }
            let account = self.core.client.request(
                "account/read",
                json!({"refreshToken":false}),
                deadline,
            )?;
            require_chatgpt(&account)?;
            let thread=self.core.client.request("thread/start",json!({"cwd":self.core.options.cwd,"model":self.core.options.model,"approvalPolicy":"never","sandbox":"danger-full-access"}),deadline)?;
            let held = thread["thread"]["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or("Codex thread/start omitted thread.id; select PTY")?;
            if thread["modelProvider"].as_str() != Some("openai") {
                return Err("Codex app-server requires the OpenAI ChatGPT provider; custom providers are refused to prevent accidental API billing. Select PTY for this configuration.".into());
            }
            self.core.state.lock().thread = Some(held.into());
            Ok(())
        })();
        if result.is_err() {
            let _ = self.core.client.stop();
        }
        result
    }
    /// Register first, then activate persistence and permit the first turn.
    pub fn activate(&self) -> Result<(), String> {
        let mut state = self.core.state.lock();
        state.ready = true;
        let buffered = std::mem::take(&mut state.buffered);
        drop(state);
        for event in buffered {
            self.core.emit(event);
        }
        let weak = Arc::downgrade(&self.core);
        std::thread::Builder::new()
            .name("codex-turns".into())
            .spawn(move || dispatch(weak))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

pub(crate) fn require_chatgpt(response: &Value) -> Result<(), String> {
    if response["account"]["type"] == "chatgpt" && response["requiresOpenaiAuth"] == true {
        Ok(())
    } else {
        Err("Codex app-server requires the existing ChatGPT login. API-key/provider auth is refused to prevent accidental billing. Run codex login, or select PTY.".into())
    }
}

impl Core {
    fn emit(&self, event: AgentEvent) {
        let mut state = self.state.lock();
        if !state.ready {
            if state.buffered.len() == 64 {
                state.buffered.pop_front();
            }
            state.buffered.push_back(event);
            return;
        }
        drop(state);
        if let Some(registry) = self.registry.upgrade() {
            match registry.append_event(&self.options.session_id, &self.options.agent_id, event) {
                Ok(event) => {
                    let _ = self.events.send(event);
                }
                Err(error) => {
                    tracing::warn!(agent=%self.options.agent_id,%error,"Cannot persist Codex event")
                }
            }
        }
    }
    fn notification(&self, message: Value) {
        let method = message["method"].as_str().unwrap_or("");
        let params = &message["params"];
        let thread = params["threadId"].as_str();
        let mut state = self.state.lock();
        let held = state.thread.clone();
        let parent = held.as_deref().is_some_and(|held| thread == Some(held));
        // Discover children solely through an already-held parent's live links.
        if parent {
            let item = &params["item"];
            if item["type"] == "subAgentActivity" {
                if let Some(id) = item["agentThreadId"].as_str() {
                    if state.children.len() < 128 {
                        state.children.insert(id.into());
                    }
                }
            } else if item["type"] == "collabAgentToolCall"
                && item["senderThreadId"].as_str() == held.as_deref()
            {
                if let Some(ids) = item["receiverThreadIds"].as_array() {
                    for id in ids.iter().filter_map(Value::as_str).take(128) {
                        if state.children.len() < 128 {
                            state.children.insert(id.into());
                        }
                    }
                }
            }
        }
        if method == "thread/started"
            && held.is_some()
            && params["thread"]["parentThreadId"].as_str() == held.as_deref()
        {
            if let Some(id) = params["thread"]["id"].as_str() {
                if state.children.len() < 128 {
                    state.children.insert(id.into());
                }
            }
        }
        let child = thread.is_some_and(|id| state.children.contains(id));
        if let (Some(current), Some(thread)) = (&mut state.current, thread) {
            if matches!(method, "turn/started" | "turn/completed") {
                current.observe(
                    thread,
                    params["turn"]["id"].as_str().unwrap_or(""),
                    method == "turn/completed",
                );
                self.changed.notify_all();
            }
        }
        drop(state);
        if message.get("id").is_some() {
            if parent || child {
                self.emit(events::event(&self.options.agent_id,AgentEventKind::Status,message.get("id").map(Value::to_string),json!({"status":"needs_approval","message":"Codex requested approval or input. Hive declined it; use PTY for interactive approval.","request_id":message["id"]})));
            }
            if self.client.write(events::server_reply(&message)).is_err() {
                let _ = self.client.stop();
            }
        }
        if let Some(held) = held {
            if parent || child {
                if let Some(event) =
                    events::normalize(&self.options.agent_id, &held, method, params, child)
                {
                    self.emit(event);
                }
            }
        }
    }
}

fn dispatch(weak: Weak<Core>) {
    loop {
        let Some(core) = weak.upgrade() else {
            return;
        };
        let mut state = core.state.lock();
        if state.stopped {
            return;
        }
        let Some(job) = state.queue.pop_front() else {
            core.changed
                .wait_for(&mut state, Duration::from_millis(200));
            continue;
        };
        let Some(thread) = state.thread.clone() else {
            job.ticket.finish(Err("Codex thread unavailable".into()));
            continue;
        };
        state.current = Some(correlate::TurnCorrelation::new(thread.clone()));
        drop(state);
        let response=core.client.request("turn/start",json!({"threadId":thread,"input":[{"type":"text","text":job.text}],"effort":core.options.effort}),Instant::now()+Duration::from_secs(30));
        let result = response.and_then(|response| {
            response["turn"]["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(String::from)
                .ok_or_else(|| "Codex turn/start omitted turn.id".into())
        });
        let turn = match result {
            Ok(turn) => turn,
            Err(error) => {
                job.ticket.finish(Err(error));
                core.emit(events::event(
                    &core.options.agent_id,
                    AgentEventKind::Error,
                    None,
                    json!({"text":"Codex turn could not start; select PTY or retry the session."}),
                ));
                // A lost response can still have started a turn. Stop rather than
                // overlap it with the next queued message or confirm the wrong turn.
                let mut state = core.state.lock();
                state.stopped = true;
                state.current = None;
                for queued in state.queue.drain(..) {
                    queued
                        .ticket
                        .finish(Err("Codex turn outcome unknown; restart with PTY".into()));
                }
                core.changed.notify_all();
                drop(state);
                let _ = core.client.stop();
                return;
            }
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut state = core.state.lock();
        state.active_turn = Some(turn.clone());
        if let Some(current) = state.current.as_mut() {
            current.response(&turn);
        }
        while state
            .current
            .as_ref()
            .is_some_and(|c| c.delivery().confirmed != Some(true))
            && core.client.alive.load(Ordering::Acquire)
            && !state.stopped
            && Instant::now() < deadline
        {
            core.changed.wait_for(&mut state, Duration::from_millis(50));
        }
        job.ticket.finish(Ok(state
            .current
            .as_ref()
            .map(|c| c.delivery())
            .unwrap_or_else(Delivery::protocol_pending)));
        while !state.stopped
            && core.client.alive.load(Ordering::Acquire)
            && state.current.as_ref().is_some_and(|c| !c.completed())
        {
            core.changed
                .wait_for(&mut state, Duration::from_millis(200));
        }
        state.current = None;
        state.active_turn = None;
        if !core.client.alive.load(Ordering::Acquire) {
            for queued in state.queue.drain(..) {
                queued.ticket.finish(Err("Codex app-server exited".into()));
            }
            drop(state);
            core.emit(events::event(
                &core.options.agent_id,
                AgentEventKind::Error,
                None,
                json!({"text":"Codex app-server exited; select PTY to restart."}),
            ));
            return;
        }
    }
}

#[async_trait]
impl AgentTransport for CodexTransport {
    async fn start(&self) -> Result<(), String> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.start_blocking())
            .await
            .map_err(|e| e.to_string())?
    }
    async fn send(&self, text: &str) -> Result<Delivery, String> {
        let this = self.clone();
        let text = text.to_owned();
        tokio::task::spawn_blocking(move || this.send_input(&text, true))
            .await
            .map_err(|e| e.to_string())?
    }
    async fn stop(&self) -> Result<(), String> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.stop_sync())
            .await
            .map_err(|e| e.to_string())?
    }
    async fn interrupt(&self) -> Result<(), String> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.interrupt_sync())
            .await
            .map_err(|e| e.to_string())?
    }
    fn send_input(&self, text: &str, submit: bool) -> Result<Delivery, String> {
        if !self.is_alive() {
            return Err("Codex app-server is not alive; select PTY to restart".into());
        }
        let mut state = self.core.state.lock();
        if state.stopped || !state.ready {
            return Err("Codex transport is not ready".into());
        }
        if state.staged.len() + text.len() > 512 * 1024 {
            return Err("Codex staged input exceeds 512 KiB".into());
        }
        if !text.is_empty() {
            if !state.staged.is_empty() {
                state.staged.push('\n');
            }
            state.staged.push_str(text);
        }
        if !submit || state.staged.is_empty() {
            return Ok(Delivery::protocol_pending());
        }
        if state.queue.len() >= 32
            || state.queue.iter().map(|j| j.text.len()).sum::<usize>() + state.staged.len()
                > 4 * 1024 * 1024
        {
            return Err("Codex input queue is full; retry after the active turn".into());
        }
        let busy = state.current.is_some() || !state.queue.is_empty();
        let text = std::mem::take(&mut state.staged);
        let ticket = Ticket::new();
        state.queue.push_back(Queued {
            text,
            ticket: ticket.clone(),
        });
        drop(state);
        self.core.changed.notify_all();
        if busy {
            Ok(Delivery::protocol_pending())
        } else {
            ticket.wait()
        }
    }
    fn is_alive(&self) -> bool {
        self.core.client.alive.load(Ordering::Acquire) && !self.core.state.lock().stopped
    }
    fn stop_sync(&self) -> Result<(), String> {
        {
            let mut state = self.core.state.lock();
            state.stopped = true;
            state.staged.clear();
            for queued in state.queue.drain(..) {
                queued
                    .ticket
                    .finish(Err("Codex transport cancelled".into()));
            }
        }
        self.core.changed.notify_all();
        self.core.client.stop()
    }
    fn interrupt_sync(&self) -> Result<(), String> {
        let state = self.core.state.lock();
        let thread = state.thread.clone().ok_or("Codex thread not ready")?;
        let turn = state.active_turn.clone().ok_or("No active Codex turn")?;
        drop(state);
        self.core
            .client
            .request(
                "turn/interrupt",
                json!({"threadId":thread,"turnId":turn}),
                Instant::now() + Duration::from_secs(5),
            )
            .map(|_| ())
    }
    fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.core.events.subscribe()
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            has_terminal: false,
            can_interrupt: true,
            supports_approvals: false,
            event_source: EventSource::Protocol,
        }
    }
}
