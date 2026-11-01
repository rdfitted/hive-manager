//! Opt-in raw Claude CLI protocol. PTY remains the default.
pub mod client;
mod correlate;
mod events;
#[cfg(test)]
mod tests;

use crate::delivery::Delivery;
use crate::transport::{
    AgentEvent, AgentEventKind, AgentTransport, Capabilities, EventSource, TransportRegistry,
};
use async_trait::async_trait;
use parking_lot::{Condvar, Mutex};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub struct ClaudeOptions {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub transcript_id: String,
    pub initial_prompt: Option<String>,
    pub session_id: String,
    pub agent_id: String,
}
impl ClaudeOptions {
    pub fn from_pty(
        command: &str,
        args: &[&str],
        cwd: PathBuf,
        env: Vec<(String, String)>,
        session_id: String,
        agent_id: String,
    ) -> Result<Self, String> {
        let mut output = Vec::new();
        let mut transcript_id = None;
        let mut initial_prompt = None;
        let mut bypass = false;
        let mut i = 0;
        while i < args.len() {
            match args[i] {
                "--dangerously-skip-permissions" => {
                    bypass = true;
                    output.push(args[i].into());
                }
                "--session-id"
                | "--model"
                | "--effort"
                | "--append-system-prompt"
                | "--system-prompt"
                | "--append-system-prompt-file"
                | "--system-prompt-file"
                | "--plugin-dir"
                | "--agent" => {
                    let flag = args[i];
                    i += 1;
                    let value = *args
                        .get(i)
                        .ok_or("Missing Claude option value; select PTY")?;
                    if flag == "--session-id" {
                        if transcript_id.is_some() || uuid::Uuid::parse_str(value).is_err() {
                            return Err("Claude transport needs exactly one pinned session UUID; select PTY".into());
                        }
                        transcript_id = Some(value.to_owned());
                    }
                    output.extend([flag.to_string(), value.to_string()]);
                }
                "--verbose" => {}
                text if !text.starts_with('-') && i + 1 == args.len() => {
                    initial_prompt = Some(text.into())
                }
                other => {
                    let option = other.split('=').next().unwrap_or("unknown");
                    return Err(format!("Claude stream-json cannot preserve option {option:?}; select PTY for this launch. --bare and authentication overrides are not supported."));
                }
            }
            i += 1;
        }
        if !bypass {
            return Err("Claude stream-json requires the same bypass permission policy as the automated PTY launch; select PTY".into());
        }
        let transcript_id = transcript_id
            .ok_or("Claude stream-json needs the PTY launch's pinned --session-id; select PTY")?;
        output.extend(
            [
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--replay-user-messages",
                "--permission-prompts",
                "none",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        Ok(Self {
            command: command.into(),
            args: output,
            cwd,
            env,
            transcript_id,
            initial_prompt,
            session_id,
            agent_id,
        })
    }
}

struct Ticket {
    result: Mutex<Option<Result<Delivery, String>>>,
    changed: Condvar,
}
impl Ticket {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(None),
            changed: Condvar::new(),
        })
    }
    fn finish(&self, result: Result<Delivery, String>) {
        *self.result.lock() = Some(result);
        self.changed.notify_all();
    }
    fn wait(&self) -> Result<Delivery, String> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut result = self.result.lock();
        while result.is_none() && Instant::now() < deadline {
            self.changed.wait_for(
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
    staged: String,
    queue: VecDeque<Queued>,
    current: Option<correlate::TurnCorrelation>,
    buffered: VecDeque<AgentEvent>,
    message_id: Option<String>,
    initialized: bool,
    init_checked: bool,
    ready: bool,
    stopped: bool,
}
struct Core {
    options: ClaudeOptions,
    client: Arc<client::JsonClient>,
    registry: Weak<TransportRegistry>,
    state: Mutex<State>,
    changed: Condvar,
    startup: Mutex<()>,
    events: broadcast::Sender<AgentEvent>,
}
#[derive(Clone)]
pub struct ClaudeTransport {
    core: Arc<Core>,
}
impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.client.stop();
    }
}
impl ClaudeTransport {
    pub fn new(options: ClaudeOptions, registry: &Arc<TransportRegistry>) -> Arc<Self> {
        Arc::new(Self {
            core: Arc::new(Core {
                options,
                client: client::JsonClient::new(),
                registry: Arc::downgrade(registry),
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
                startup: Mutex::new(()),
                events: broadcast::channel(1024).0,
            }),
        })
    }
    pub fn initial_prompt(&self) -> Option<String> {
        self.core.options.initial_prompt.clone()
    }
    pub fn thread_id(&self) -> Option<String> {
        self.core
            .state
            .lock()
            .initialized
            .then(|| self.core.options.transcript_id.clone())
    }
    pub fn start_blocking(&self) -> Result<(), String> {
        let _startup = self.core.startup.lock();
        if self.core.state.lock().initialized {
            return if self.is_alive() {
                Ok(())
            } else {
                Err("Claude stopped; select PTY".into())
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
                &self.core.options.args,
                &self.core.options.cwd,
                &self.core.options.env,
                handler,
            )?;
            let response = self
                .core
                .client
                .request(json!({"subtype":"initialize"}), deadline)?;
            client::require_subscription(&response)?;
            if response["claude_code_version"].as_str() != Some(client::PINNED_VERSION) {
                self.core.emit(events::event(&self.core.options.agent_id, AgentEventKind::Status, None,
                    json!({"status":"version_mismatch","message":format!("Claude stream-json tested with {}; installed version differs. Select PTY if authentication/protocol behavior changes.",client::PINNED_VERSION)})));
            }
            self.core.state.lock().initialized = true;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.core.client.stop();
        }
        result
    }
    pub fn activate(&self) -> Result<(), String> {
        let buffered = {
            let mut state = self.core.state.lock();
            state.ready = true;
            std::mem::take(&mut state.buffered)
        };
        for event in buffered {
            self.core.emit(event);
        }
        let weak = Arc::downgrade(&self.core);
        std::thread::Builder::new()
            .name("claude-turns".into())
            .spawn(move || dispatch(weak))
            .map_err(|e| e.to_string())?;
        Ok(())
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
                    tracing::warn!(agent=%self.options.agent_id,%error,"Cannot persist Claude event")
                }
            }
        }
    }
    fn notification(&self, value: Value) {
        if value["type"] == "control_request" {
            self.emit(events::event(&self.options.agent_id, AgentEventKind::Status,
                value["request_id"].as_str().map(str::to_owned), json!({"status":"needs_approval",
                    "message":"Claude requested an interactive decision. It was declined; select PTY to approve.","request_id":value["request_id"]})));
            if self.client.write(events::server_reply(&value)).is_err() {
                let _ = self.client.stop();
            }
            return;
        }
        if value["session_id"].as_str() != Some(self.options.transcript_id.as_str())
            || !value["parent_tool_use_id"].is_null()
            || !value["agent_id"].is_null()
        {
            return;
        }
        if value["type"] == "system" && value["subtype"] == "init" {
            if value["apiKeySource"] != "none" || value["permissionMode"] != "bypassPermissions" {
                self.emit(events::event(&self.options.agent_id, AgentEventKind::Error, None,
                    json!({"text":"Claude runtime authentication or permission policy changed. Transport stopped to prevent accidental billing; select PTY."})));
                let _ = self.client.stop();
                return;
            }
            self.state.lock().init_checked = true;
            return;
        }
        let mut emitted = Vec::new();
        let mut state = self.state.lock();
        if !state.init_checked {
            return;
        }
        let mut start = None;
        let mut end = None;
        if let Some(current) = state.current.as_mut() {
            let previous = current.delivery().confirmed;
            let completed = current.completed;
            current.observe(&value);
            if previous != Some(true) && current.delivery().confirmed == Some(true) {
                start = Some(current.uuid().to_owned());
            }
            if !completed && current.completed {
                end = Some(current.uuid().to_owned());
            }
        }
        if value["type"] == "stream_event" && value["event"]["type"] == "message_start" {
            state.message_id = value["event"]["message"]["id"].as_str().map(str::to_owned);
        }
        if let Some(id) = start {
            emitted.push(events::event(
                &self.options.agent_id,
                AgentEventKind::TurnStart,
                Some(id.clone()),
                json!({"turn_id":id}),
            ));
        }
        emitted.extend(events::normalize(
            &self.options.agent_id,
            &value,
            state.message_id.as_deref(),
        ));
        if let Some(id) = end {
            emitted.push(events::event(
                &self.options.agent_id,
                AgentEventKind::TurnEnd,
                Some(id.clone()),
                json!({"turn_id":id,"is_error":value["is_error"].as_bool().unwrap_or(false)}),
            ));
            if let Some(error) = events::result_error(&self.options.agent_id, &value, &id) {
                emitted.push(error);
            }
        }
        drop(state);
        for event in emitted {
            self.emit(event);
        }
        self.changed.notify_all();
    }
}

fn dispatch(weak: Weak<Core>) {
    loop {
        let Some(core) = weak.upgrade() else {
            return;
        };
        let mut state = core.state.lock();
        if state.stopped || !core.client.alive.load(Ordering::Acquire) {
            return;
        }
        let Some(job) = state.queue.pop_front() else {
            core.changed
                .wait_for(&mut state, Duration::from_millis(100));
            continue;
        };
        let uuid = uuid::Uuid::new_v4().to_string();
        state.current = Some(correlate::TurnCorrelation::new(
            core.options.transcript_id.clone(),
            uuid.clone(),
        ));
        state.message_id = None;
        drop(state);
        if let Err(error) = core.client.write(json!({"type":"user","uuid":uuid,
            "session_id":core.options.transcript_id,"parent_tool_use_id":null,"priority":"later","client_composed":true,
            "message":{"role":"user","content":job.text}})) {
            job.ticket.finish(Err(error));
            let _ = core.client.stop();
        } else {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut state = core.state.lock();
            while !state.stopped && core.client.alive.load(Ordering::Acquire) && Instant::now() < deadline
                && state.current.as_ref().is_some_and(|c| c.delivery().confirmed != Some(true)) {
                core.changed.wait_for(&mut state, Duration::from_millis(25));
            }
            if !core.client.alive.load(Ordering::Acquire) || state.stopped {
                job.ticket.finish(Err("Claude exited or was cancelled; select PTY".into()));
            } else {
                job.ticket.finish(Ok(state.current.as_ref().map(|c|c.delivery()).unwrap_or_else(Delivery::protocol_pending)));
            }
            while !state.stopped && core.client.alive.load(Ordering::Acquire)
                && state.current.as_ref().is_some_and(|c| !c.completed) {
                core.changed.wait_for(&mut state, Duration::from_millis(100));
            }
        }
        let mut state = core.state.lock();
        state.current = None;
        if !core.client.alive.load(Ordering::Acquire) {
            state.stopped = true;
            for queued in state.queue.drain(..) {
                queued
                    .ticket
                    .finish(Err("Claude transport exited; select PTY".into()));
            }
            drop(state);
            core.emit(events::event(
                &core.options.agent_id,
                AgentEventKind::Error,
                None,
                json!({"text":"Claude stream-json exited; select PTY to restart."}),
            ));
            return;
        }
    }
}

#[async_trait]
impl AgentTransport for ClaudeTransport {
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
        let mut state = self.core.state.lock();
        if state.stopped || !state.ready || !self.core.client.alive.load(Ordering::Acquire) {
            return Err("Claude transport is not ready; select PTY".into());
        }
        if state.staged.len() + text.len() > 512 * 1024 {
            return Err("Claude staged input exceeds 512 KiB".into());
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
            return Err("Claude input queue is full; retry after the active turn".into());
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
                    .finish(Err("Claude transport cancelled".into()));
            }
        }
        self.core.changed.notify_all();
        self.core.client.stop()
    }
    fn interrupt_sync(&self) -> Result<(), String> {
        if self.core.state.lock().current.is_none() {
            return Err("No active Claude turn".into());
        }
        self.core
            .client
            .request(
                json!({"subtype":"interrupt"}),
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
