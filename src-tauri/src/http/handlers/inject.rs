use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime};
use crate::delivery::{self, Confirmation, Delivery};

use super::{validate_agent_id, validate_session_id};
use crate::adapters::CodexAdapter;
use crate::coordination::{InjectionError, InjectionReceipt};
use crate::http::error::ApiError;
use crate::http::state::AppState;

fn default_submit() -> bool {
    true
}

const INJECTION_EVIDENCE_SCOPE: &str = "submit_confirmed true means a nonce-matched user message was recorded in the agent transcript together with its turn start, or inside a turn already running (a Codex steer). null means unknown, pending, unavailable or not requested; transcript-pending-busy means the message was handed to Codex's queue key while a turn was running and is expected when that turn ends. false requires transcript timeout and visibly staged text. PTY screen activity is advisory and never confirms delivery. Transcript evidence does not identify who pressed Enter.";
const TRANSCRIPT_SYNC_WAIT: Duration = Duration::from_secs(8);
const TRANSCRIPT_WATCH_LIMIT: Duration = Duration::from_secs(60);
/// A queue-key retry into an idle Codex submits at once; wait this long for its transcript
/// turn so the receipt can say so instead of pending.
const CODEX_RETRY_CONFIRM_WAIT: Duration = Duration::from_secs(3);
/// Upper bound on watching a message queued behind a running Codex turn.
const CODEX_BUSY_WATCH_LIMIT: Duration = Duration::from_secs(30 * 60);
const TRANSCRIPT_POLL: Duration = Duration::from_millis(250);
const INJECTION_ACTIVITY_OBSERVATION_WINDOW: Duration = Duration::from_millis(250);
const INJECTION_ACTIVITY_POLL_INTERVAL: Duration = Duration::from_millis(10);
const SUBMIT_KEYSTROKE: &[u8] = b"\r";
/// How long the sender watches the ring for delivery evidence after the Enter write (#256).
const SUBMIT_CONFIRMATION_WINDOW: Duration = Duration::from_millis(1500);
/// Minimum spread between the first and last observed ring change for the activity to
/// count as sustained rather than a single composer repaint.
const SUBMIT_CONFIRMATION_MIN_SPAN: Duration = Duration::from_millis(250);

struct PreWriteActivityBaseline {
    pty_observation_available: bool,
    change_offsets: Vec<Duration>,
    observation_elapsed_ms: u64,
}

impl PreWriteActivityBaseline {
    fn receiver_busy(&self) -> Option<bool> {
        self.pty_observation_available
            .then_some(pre_write_activity_is_sustained(&self.change_offsets))
    }
}

struct InjectionObservation {
    pty_observation_available: bool,
    pty_output_bytes_after: Option<usize>,
    pty_activity_observed: Option<bool>,
    observation_window_ms: u64,
    observation_elapsed_ms: u64,
    submit_confirmed: Option<bool>,
    submit_confirmation_basis: &'static str,
    submit_confirmation_window_ms: u64,
    submit_confirmation_elapsed_ms: u64,
}

struct InjectionMeasurement {
    delivery: Delivery,
    delivery_elapsed_ms: u64,
    pre_write_baseline: PreWriteActivityBaseline,
    observation: InjectionObservation,
    submit_attempts: usize,
    submit_bytes_written: usize,
    submit_retry_failure: Option<String>,
    submit_retry_keystroke: Option<&'static str>,
    receiver_turn_running: Option<bool>,
}

type InjectionTransactionMutex = tokio::sync::Mutex<()>;
type InjectionTransactionLockMap = HashMap<(usize, String), Weak<InjectionTransactionMutex>>;

/// Serialize the complete write -> observe -> optional retry transaction per runtime/agent.
///
/// The map holds only weak references, so completed transactions do not retain one mutex per
/// historical agent. Including the AppState identity keeps independent runtimes (and tests) from
/// blocking each other when they happen to use the same agent id.
fn injection_transaction_lock(state: &AppState, agent_id: &str) -> Arc<InjectionTransactionMutex> {
    static LOCKS: OnceLock<parking_lot::Mutex<InjectionTransactionLockMap>> = OnceLock::new();

    let key = (state as *const AppState as usize, agent_id.to_string());
    let mut locks = LOCKS
        .get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
        .lock();
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }

    let lock = Arc::new(InjectionTransactionMutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

#[cfg(all(test, windows))]
type BeforeSubmitRetryHook =
    std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>;

#[cfg(all(test, windows))]
fn before_submit_retry_hooks() -> &'static parking_lot::Mutex<HashMap<usize, BeforeSubmitRetryHook>>
{
    static HOOKS: OnceLock<parking_lot::Mutex<HashMap<usize, BeforeSubmitRetryHook>>> =
        OnceLock::new();
    HOOKS.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

#[cfg(all(test, windows))]
fn install_before_submit_retry_hook<F>(state: &AppState, hook: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    before_submit_retry_hooks()
        .lock()
        .insert(state as *const AppState as usize, Box::pin(hook));
}

#[cfg(all(test, windows))]
async fn run_before_submit_retry_hook(state: &AppState) {
    let hook = before_submit_retry_hooks()
        .lock()
        .remove(&(state as *const AppState as usize));
    if let Some(hook) = hook {
        hook.await;
    }
}

impl InjectionObservation {
    fn unobserved(
        pty_observation_available: bool,
        pty_output_bytes_after: Option<usize>,
        submit_confirmation_basis: &'static str,
    ) -> Self {
        Self {
            pty_observation_available,
            pty_output_bytes_after,
            pty_activity_observed: None,
            observation_window_ms: 0,
            observation_elapsed_ms: 0,
            submit_confirmed: None,
            submit_confirmation_basis,
            submit_confirmation_window_ms: 0,
            submit_confirmation_elapsed_ms: 0,
        }
    }
}

/// Classify post-Enter ring behaviour into the tri-state delivery signal (#256, #260).
///
/// `post_submit_change_offsets` holds the elapsed time of every poll at which the ring content
/// differed from the previous poll. Changes spread over at least
/// [`SUBMIT_CONFIRMATION_MIN_SPAN`] are the signature of a composer that accepted Enter
/// and started a turn. A short isolated burst is ambiguous — a swallowed Enter also
/// repaints the composer once. Zero changes mean the Enter provably produced no visible
/// reaction, which a live TUI never does for an accepted submit. Changes spanning at least
/// half the fixed pre-write baseline identify an already-streaming receiver; only a
/// would-be positive is downgraded because that output cannot be attributed to Enter.
fn classify_screen_activity(
    post_submit_change_offsets: &[Duration],
    pre_write_change_offsets: &[Duration],
) -> (Option<bool>, &'static str) {
    let classification = match post_submit_change_offsets {
        [] => (Some(false), "no-post-submit-activity"),
        [first, .., last] if last.saturating_sub(*first) >= SUBMIT_CONFIRMATION_MIN_SPAN => {
            (Some(true), "sustained-post-submit-activity")
        }
        _ => (None, "ambiguous-post-submit-activity"),
    };

    if classification.0 == Some(true) && pre_write_activity_is_sustained(pre_write_change_offsets) {
        (None, "busy-receiver-indeterminate")
    } else {
        classification
    }
}

fn pre_write_activity_is_sustained(change_offsets: &[Duration]) -> bool {
    // Half the bounded window separates a clustered repaint burst from activity that persists
    // across a meaningful portion of the baseline, while leaving ample polling-jitter headroom.
    let minimum_busy_span = INJECTION_ACTIVITY_OBSERVATION_WINDOW / 2;
    matches!(
        change_offsets,
        [first, .., last] if last.saturating_sub(*first) >= minimum_busy_span
    )
}

fn post_submit_activity_is_sustained(change_offsets: &[Duration]) -> bool {
    matches!(
        change_offsets,
        [first, .., last]
            if last.saturating_sub(*first) >= SUBMIT_CONFIRMATION_MIN_SPAN
    )
}

async fn observe_pre_write_activity(state: &AppState, agent_id: &str) -> PreWriteActivityBaseline {
    let observation_started = Instant::now();
    let observation_deadline = observation_started + INJECTION_ACTIVITY_OBSERVATION_WINDOW;
    let Some(mut previous) = state.pty_manager.read().recent_output(agent_id) else {
        return PreWriteActivityBaseline {
            pty_observation_available: false,
            change_offsets: Vec::new(),
            observation_elapsed_ms: 0,
        };
    };
    let mut change_offsets = Vec::new();
    let mut pty_observation_available = true;

    loop {
        let remaining = observation_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(INJECTION_ACTIVITY_POLL_INTERVAL.min(remaining)).await;
        let Some(current) = state.pty_manager.read().recent_output(agent_id) else {
            pty_observation_available = false;
            break;
        };
        if current != previous {
            change_offsets.push(observation_started.elapsed());
            previous = current;
        }
    }

    PreWriteActivityBaseline {
        pty_observation_available,
        change_offsets,
        observation_elapsed_ms: observation_started.elapsed().as_millis() as u64,
    }
}

async fn observe_submit(
    state: &AppState,
    agent_id: &str,
    submit_keystroke_issued: bool,
    pty_output_after_write: Option<&str>,
    pre_write_change_offsets: &[Duration],
) -> InjectionObservation {
    if !submit_keystroke_issued {
        return InjectionObservation::unobserved(
            pty_output_after_write.is_some(),
            pty_output_after_write.map(str::len),
            "not-submitted",
        );
    }

    let Some(baseline) = pty_output_after_write else {
        return InjectionObservation::unobserved(false, None, "pty-unobservable");
    };

    let observation_started = Instant::now();
    let confirmation_deadline = observation_started + SUBMIT_CONFIRMATION_WINDOW;
    let mut previous = baseline.to_owned();
    let mut output_after = Some(baseline.to_owned());
    let mut first_activity_elapsed: Option<Duration> = None;
    let mut change_offsets: Vec<Duration> = Vec::new();
    let mut ring_lost = false;

    loop {
        let remaining = confirmation_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(INJECTION_ACTIVITY_POLL_INTERVAL.min(remaining)).await;
        let current = state.pty_manager.read().recent_output(agent_id);
        let Some(current) = current else {
            output_after = None;
            ring_lost = true;
            break;
        };
        let elapsed = observation_started.elapsed();
        if current != previous {
            change_offsets.push(elapsed);
            if first_activity_elapsed.is_none() {
                first_activity_elapsed = Some(elapsed);
            }
            previous = current.clone();
        }
        output_after = Some(current);
        // Once the sustained-activity criterion is met, the verdict cannot change.
        if post_submit_activity_is_sustained(&change_offsets) {
            break;
        }
    }

    let (submit_confirmed, submit_confirmation_basis) = if ring_lost {
        (None, "pty-unobservable")
    } else {
        classify_screen_activity(&change_offsets, pre_write_change_offsets)
    };

    // The legacy activity fields keep their pre-#256 meaning: did the ring change within
    // the first 250 ms, and when was that change first seen.
    let activity_within_window = first_activity_elapsed
        .is_some_and(|elapsed| elapsed <= INJECTION_ACTIVITY_OBSERVATION_WINDOW);
    let observation_elapsed = match first_activity_elapsed {
        Some(elapsed) if activity_within_window => elapsed,
        _ => INJECTION_ACTIVITY_OBSERVATION_WINDOW.min(observation_started.elapsed()),
    };
    let observation_available = output_after.is_some();
    InjectionObservation {
        pty_observation_available: observation_available,
        pty_output_bytes_after: output_after.as_ref().map(String::len),
        pty_activity_observed: observation_available.then_some(activity_within_window),
        observation_window_ms: INJECTION_ACTIVITY_OBSERVATION_WINDOW.as_millis() as u64,
        observation_elapsed_ms: observation_elapsed.as_millis() as u64,
        submit_confirmed,
        submit_confirmation_basis,
        submit_confirmation_window_ms: SUBMIT_CONFIRMATION_WINDOW.as_millis() as u64,
        submit_confirmation_elapsed_ms: observation_started.elapsed().as_millis() as u64,
    }
}

/// An active composer prompt and exact trailing marker are necessary. Raw echo
/// or unrelated activity elsewhere in the terminal cannot justify another Enter.
fn staged_text_visible(output: Option<&str>, nonce: &str) -> bool {
    let Some(output) = output else { return false };
    if nonce.is_empty() { return false; }
    let prompt = output.lines().enumerate().filter_map(|(index, line)| {
        let line = line.trim_start();
        (line.starts_with('\u{203a}') || line.starts_with('\u{276f}') || line.starts_with("> "))
            .then_some(index)
    }).last();
    let Some(prompt) = prompt else { return false };
    output.lines().skip(prompt).collect::<Vec<_>>().join("\n").trim_end().ends_with(nonce)
}

fn confirmation_cwd(session: &crate::session::Session, agent: &crate::session::AgentInfo) -> std::path::PathBuf {
    use crate::pty::AgentRole;
    use crate::domain::WorkspaceStrategy;
    if session.no_git { return session.project_path.clone(); }
    if session.execution_policy.workspace_strategy == WorkspaceStrategy::SharedCell
        && matches!(agent.role, AgentRole::Queen | AgentRole::Worker { .. }) {
        return session.worktree_path.as_ref().map(std::path::PathBuf::from)
            .unwrap_or_else(|| session.project_path.clone());
    }
    let worktrees = session.project_path.join(".hive-manager/worktrees").join(&session.id);
    match &agent.role {
        AgentRole::Worker { index, .. } => worktrees.join(format!("worker-{index}")),
        AgentRole::Queen if matches!(session.session_type, crate::session::SessionType::Hive { .. }) => worktrees.join("queen"),
        _ => session.worktree_path.as_ref().map(std::path::PathBuf::from)
            .unwrap_or_else(|| session.project_path.clone()),
    }
}

fn confirmation_reader(state: &AppState, session_id: &str, agent_id: &str, _since: SystemTime) -> Confirmation {
    let Some(session) = state.session_controller.read().get_session(session_id) else {
        return Confirmation::Unavailable;
    };
    let Some(agent) = session.agents.iter().find(|agent| agent.id == agent_id) else {
        return Confirmation::Unavailable;
    };
    let cwd = confirmation_cwd(&session, agent);
    #[cfg(not(test))]
    let root = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from);
    #[cfg(test)]
    let root = Some(state.storage.base_dir().join("transcript-test-home"));
    let Some(root) = root else { return Confirmation::Unavailable };
    let spawned_at = SystemTime::from(agent.transcript_spawned_at.unwrap_or(session.created_at));
    Confirmation::new(&agent.config.cli, agent.transcript_session_id.as_deref(), &cwd, spawned_at,
        root.join(".codex/sessions"), root.join(".claude/projects"))
}

fn agent_cli(state: &AppState, session_id: &str, agent_id: &str) -> Option<String> {
    state.session_controller.read().get_session(session_id)?
        .agents.into_iter().find(|agent| agent.id == agent_id)
        .map(|agent| agent.config.cli)
}

#[derive(Clone)]
struct PendingNonce {
    nonce: String,
    since: chrono::DateTime<chrono::Utc>,
    submitted_at: chrono::DateTime<chrono::Utc>,
    expires: Instant,
    state: Weak<AppState>,
}

type PendingNonceKey = (usize, String, String);
fn pending_nonces() -> &'static parking_lot::Mutex<HashMap<PendingNonceKey, PendingNonce>> {
    static PENDING: OnceLock<parking_lot::Mutex<HashMap<PendingNonceKey, PendingNonce>>> = OnceLock::new();
    PENDING.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

fn pending_nonce_key(state: &AppState, session_id: &str, agent_id: &str) -> PendingNonceKey {
    (state as *const AppState as usize, session_id.to_owned(), agent_id.to_owned())
}

/// Empty flushes carry no marker. Reuse evidence only from an unexpired watch in
/// this runtime/session/agent, never another agent with the same workspace.
fn inherit_pending_nonce(state: &Arc<AppState>, session_id: &str, agent_id: &str, receipt: &mut InjectionReceipt) {
    if receipt.nonce.is_some() || !receipt.submit_keystroke_issued { return; }
    let mut pending = pending_nonces().lock();
    pending.retain(|_, value| value.expires > Instant::now() && value.state.strong_count() > 0);
    if let Some(previous) = pending.get(&pending_nonce_key(state, session_id, agent_id)) {
        let still_pending = state.session_controller.read().get_session(session_id)
            .and_then(|session| session.agents.into_iter().find(|agent| agent.id == agent_id))
            .is_some_and(|agent| matches!(agent.status, crate::pty::AgentStatus::WaitingForInput(reason)
                if reason == format!("awaiting_injected_turn:{}", previous.submitted_at.to_rfc3339())));
        if still_pending {
            receipt.nonce = Some(previous.nonce.clone());
            receipt.confirmation_since = Some(previous.since);
            receipt.confirmation_expires = Some(previous.expires);
        }
    }
}

type SharedConfirmation = Arc<parking_lot::Mutex<Confirmation>>;

async fn poll_transcript(reader: &SharedConfirmation, nonce: &str, since: SystemTime) -> (Delivery, bool) {
    let reader = Arc::clone(reader);
    let nonce = nonce.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut reader = reader.lock();
        let result = delivery::confirm(&mut reader, &nonce, since);
        let recorded = reader.message_recorded(&nonce);
        (result, recorded)
    }).await.unwrap_or_else(|_| (Delivery::unavailable(), false))
}

async fn wait_for_transcript(reader: &SharedConfirmation, nonce: &str, since: SystemTime,
    deadline: Instant) -> (Delivery, bool) {
    let mut last = (Delivery::pending(), false);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return last; }
        last = match tokio::time::timeout(remaining, poll_transcript(reader, nonce, since)).await {
            Ok(result) => result,
            // Unknown in-flight I/O suppresses retry; ordinary polling expiry below
            // preserves the actual last observed absent/recorded verdict instead.
            Err(_) => return (Delivery::pending(), true),
        };
        if last.0.confirmed == Some(true) || last.0.basis == delivery::TRANSCRIPT_UNAVAILABLE
            || Instant::now() >= deadline { return last; }
        tokio::time::sleep(TRANSCRIPT_POLL.min(deadline.saturating_duration_since(Instant::now()))).await;
    }
}

fn record_delivery(state: &AppState, session_id: &str, agent_id: &str,
    receipt: &InjectionReceipt, delivery: &Delivery) {
    if !delivery.is_pending() {
        let key = pending_nonce_key(state, session_id, agent_id);
        let mut pending = pending_nonces().lock();
        if pending.get(&key).is_some_and(|value| value.submitted_at == receipt.submitted_at) {
            pending.remove(&key);
        }
    }
    if let Err(error) = state.session_controller.read().record_injection_delivery(
        session_id, agent_id, receipt.submitted_at, delivery) {
        tracing::debug!("Delivery watch stopped for {session_id}/{agent_id}: {error}");
    }
}

/// With `extend_while_busy`, an expiring watch is renewed while Codex's title still shows its
/// running-turn spinner (up to [`CODEX_BUSY_WATCH_LIMIT`]): a message queued behind that turn
/// can only reach the transcript once the turn ends.
fn watch_pending_delivery(state: &Arc<AppState>, session_id: &str, agent_id: &str,
    receipt: &InjectionReceipt, reader: SharedConfirmation, watch_limit: Duration,
    extend_while_busy: bool) {
    let weak = Arc::downgrade(state);
    let session_id = session_id.to_owned();
    let agent_id = agent_id.to_owned();
    let receipt = receipt.clone();
    let nonce = receipt.nonce.clone().expect("submitted nonce");
    let since = SystemTime::from(receipt.confirmation_since.unwrap_or(receipt.write_started_at));
    tokio::spawn(async move {
        let started = Instant::now();
        let busy_deadline = started + CODEX_BUSY_WATCH_LIMIT.max(watch_limit);
        let mut deadline = started + watch_limit;
        let mut message_recorded = false;
        loop {
            let Some(state) = weak.upgrade() else { return };
            if !state.pty_manager.read().is_alive(&agent_id)
                || !state.session_controller.read().get_session(&session_id)
                    .is_some_and(|session| session.agents.iter().any(|agent| agent.id == agent_id
                        && matches!(&agent.status, crate::pty::AgentStatus::WaitingForInput(reason)
                            if reason == &format!("awaiting_injected_turn:{}", receipt.submitted_at.to_rfc3339())))) {
                return;
            }
            if Instant::now() >= deadline {
                let output = state.pty_manager.read().recent_output(&agent_id);
                if extend_while_busy && Instant::now() < busy_deadline
                    && output.as_deref().and_then(CodexAdapter::turn_running) == Some(true) {
                    deadline = (Instant::now() + TRANSCRIPT_WATCH_LIMIT).min(busy_deadline);
                } else {
                    let result = Delivery::timeout(!message_recorded && staged_text_visible(output.as_deref(), &nonce));
                    record_delivery(&state, &session_id, &agent_id, &receipt, &result);
                    return;
                }
            }
            // At least one poll interval, so a watch renewed at its deadline still reads once.
            let remaining = deadline.saturating_duration_since(Instant::now()).max(TRANSCRIPT_POLL);
            let polled = tokio::time::timeout(remaining, poll_transcript(&reader, &nonce, since)).await;
            let (result, recorded) = match polled {
                Ok(result) => result,
                Err(_) => {
                    // A blocking filesystem read cannot be cancelled. End this watch
                    // instead of piling more tasks onto the same reader mutex.
                    record_delivery(&state, &session_id, &agent_id, &receipt, &Delivery::unavailable());
                    return;
                }
            };
            message_recorded |= recorded;
            if result.basis == delivery::TRANSCRIPT_UNAVAILABLE {
                record_delivery(&state, &session_id, &agent_id, &receipt, &result);
                return;
            }
            if result.confirmed == Some(true) {
                record_delivery(&state, &session_id, &agent_id, &receipt, &result);
                return;
            }
            drop(state);
            tokio::time::sleep(TRANSCRIPT_POLL).await;
        }
    });
}

async fn observe_injection_with_retry(
    state: &Arc<AppState>, session_id: &str, agent_id: &str,
    receipt: &InjectionReceipt, pre_write_baseline: PreWriteActivityBaseline,
) -> InjectionMeasurement {
    let started = Instant::now();
    let since = SystemTime::from(receipt.confirmation_since.unwrap_or(receipt.write_started_at));
    let reader = Arc::new(parking_lot::Mutex::new(if receipt.nonce.is_some() {
        confirmation_reader(state, session_id, agent_id, since)
    } else { Confirmation::Unavailable }));
    let nonce = receipt.nonce.as_deref().unwrap_or("");
    // A busy Codex drains input late, so its paste-burst check can turn the sender's Enter into
    // a newline and strand the message in the composer. That composer is drawn as cell diffs
    // under a spinner-driven title stream, so the raw PTY tail cannot show staged text. Codex
    // therefore retries on transcript evidence alone, with its queue key, which does nothing
    // on an empty composer (see `CodexAdapter::QUEUE_KEY`).
    let codex = agent_cli(state, session_id, agent_id).as_deref() == Some("codex");
    let retry_gate = |output: Option<&str>| codex || staged_text_visible(output, nonce);
    let retry_keystroke = if codex { CodexAdapter::QUEUE_KEY } else { SUBMIT_KEYSTROKE };
    let screen = observe_submit(state, agent_id, receipt.submit_keystroke_issued,
        receipt.pty_output_after_write.as_deref(), &pre_write_baseline.change_offsets);
    let transcript = async {
        if receipt.submit_keystroke_issued {
            let deadline = receipt.confirmation_expires.map(|expires| expires.min(started + TRANSCRIPT_SYNC_WAIT))
                .unwrap_or(started + TRANSCRIPT_SYNC_WAIT);
            wait_for_transcript(&reader, nonce, since, deadline).await
        } else { (Delivery::unavailable(), false) }
    };
    let (observation, (mut verdict, recorded)) = tokio::join!(screen, transcript);
    let mut submit_attempts = usize::from(receipt.submit_keystroke_issued);
    let mut submit_bytes_written = receipt.submit_bytes_written;
    let mut submit_retry_failure = None;
    let mut submit_retry_keystroke = None;
    let mut receiver_turn_running = None;
    let output = state.pty_manager.read().recent_output(agent_id);
    if receipt.submit_keystroke_issued && verdict.is_pending()
        && !recorded && retry_gate(output.as_deref()) {
        let latest = poll_transcript(&reader, nonce, since).await;
        verdict = latest.0;
        let current_output = state.pty_manager.read().recent_output(agent_id);
        if verdict.is_pending() && !latest.1 && retry_gate(current_output.as_deref()) {
            #[cfg(all(test, windows))]
            run_before_submit_retry_hook(state).await;
            if codex {
                receiver_turn_running = state.pty_manager.read().recent_output(agent_id)
                    .as_deref().and_then(CodexAdapter::turn_running);
            }
            let manager = Arc::clone(&state.pty_manager);
            let target = agent_id.to_owned();
            let result = tokio::task::spawn_blocking(move || manager.read().write(&target, retry_keystroke)).await;
            match result {
                Ok(Ok(())) => {
                    submit_attempts += 1;
                    submit_bytes_written += retry_keystroke.len();
                    submit_retry_keystroke = Some(if codex { "tab" } else { "enter" });
                    if codex && receiver_turn_running == Some(true) {
                        // Queued behind the running turn: nothing can confirm before it ends.
                        verdict = Delivery::pending_busy();
                    } else if codex {
                        let deadline = Instant::now() + CODEX_RETRY_CONFIRM_WAIT;
                        let deadline = receipt.confirmation_expires.map_or(deadline, |expires| expires.min(deadline));
                        verdict = wait_for_transcript(&reader, nonce, since, deadline).await.0;
                    }
                }
                other => submit_retry_failure = Some(format!("Submit retry failed: {other:?}")),
            }
        }
    }
    if verdict.basis == delivery::TRANSCRIPT_PENDING
        && receipt.confirmation_expires.is_some_and(|expires| expires <= Instant::now()) {
        verdict = Delivery::timeout(false);
    }
    let busy = verdict.basis == delivery::TRANSCRIPT_PENDING_BUSY;
    if receipt.submit_keystroke_issued {
        let key = pending_nonce_key(state, session_id, agent_id);
        let mut pending = pending_nonces().lock();
        pending.retain(|_, value| value.expires > Instant::now() && value.state.strong_count() > 0);
        pending.remove(&key);
        if verdict.is_pending() {
            let watch_limit = if busy { CODEX_BUSY_WATCH_LIMIT } else { TRANSCRIPT_WATCH_LIMIT };
            pending.insert(key, PendingNonce {
                nonce: nonce.to_owned(), since: receipt.confirmation_since.unwrap_or(receipt.write_started_at),
                submitted_at: receipt.submitted_at,
                expires: receipt.confirmation_expires.unwrap_or_else(|| Instant::now() + watch_limit),
                state: Arc::downgrade(state),
            });
        }
    }
    if receipt.submit_keystroke_issued {
        record_delivery(state, session_id, agent_id, receipt, &verdict);
        if verdict.is_pending() {
            let limit = receipt.confirmation_expires.map(|expires| expires.saturating_duration_since(Instant::now()))
                .unwrap_or(TRANSCRIPT_WATCH_LIMIT);
            watch_pending_delivery(state, session_id, agent_id, receipt, reader, limit, busy);
        }
    }
    InjectionMeasurement {
        delivery: verdict, delivery_elapsed_ms: started.elapsed().as_millis() as u64,
        pre_write_baseline, observation, submit_attempts, submit_bytes_written, submit_retry_failure,
        submit_retry_keystroke, receiver_turn_running,
    }
}

fn record_injection_submission(
    state: &AppState,
    session_id: &str,
    agent_id: &str,
    receipt: &InjectionReceipt,
) {
    if let Err(error) = state.session_controller.read().record_injection_submission(
        session_id,
        agent_id,
        receipt.submitted_at,
        receipt.submit_keystroke_issued,
    ) {
        tracing::warn!(
            "Injection succeeded but its awaiting-turn observation was not recorded for {session_id}/{agent_id}: {error}"
        );
    }
}

fn measured_response(
    message: String,
    receipt: &InjectionReceipt,
    measurement: &InjectionMeasurement,
) -> Value {
    let pre_write_baseline = &measurement.pre_write_baseline;
    let observation = &measurement.observation;
    json!({
        "status": "success",
        "message": message,
        "write_started_at": receipt.write_started_at,
        "submitted_at": receipt.submitted_at,
        "payload_bytes_written": receipt.payload_bytes_written,
        "submit_attempted": receipt.submit_keystroke_issued,
        "submit_keystroke_issued": receipt.submit_keystroke_issued,
        "submit_bytes_written": measurement.submit_bytes_written,
        "submit_attempts": measurement.submit_attempts,
        "submit_retry_failed": measurement.submit_retry_failure.is_some(),
        "submit_retry_failure": measurement.submit_retry_failure,
        "submit_retry_keystroke": measurement.submit_retry_keystroke,
        "receiver_turn_running": measurement.receiver_turn_running,
        "pre_write_pty_observation_available": pre_write_baseline.pty_observation_available,
        "pre_write_activity_change_count": pre_write_baseline.change_offsets.len(),
        "receiver_busy_before_write": pre_write_baseline.receiver_busy(),
        "pre_write_observation_window_ms": INJECTION_ACTIVITY_OBSERVATION_WINDOW.as_millis() as u64,
        "pre_write_observation_elapsed_ms": pre_write_baseline.observation_elapsed_ms,
        "pty_observation_available": observation.pty_observation_available,
        "pty_output_bytes_before": receipt.pty_output_before.as_ref().map(String::len),
        "pty_output_bytes_after_write": receipt.pty_output_after_write.as_ref().map(String::len),
        "pty_output_bytes_after": observation.pty_output_bytes_after,
        "pty_activity_observed": observation.pty_activity_observed,
        "observation_window_ms": observation.observation_window_ms,
        "observation_elapsed_ms": observation.observation_elapsed_ms,
        "submit_confirmed": measurement.delivery.confirmed,
        "submit_confirmation_source": measurement.delivery.source,
        "turn_id": measurement.delivery.turn_id,
        "submit_screen_activity": observation.submit_confirmed,
        "submit_screen_activity_basis": observation.submit_confirmation_basis,
        "submit_screen_activity_window_ms": observation.submit_confirmation_window_ms,
        "submit_screen_activity_elapsed_ms": observation.submit_confirmation_elapsed_ms,
        "submit_confirmation_basis": measurement.delivery.basis,
        "submit_confirmation_window_ms": TRANSCRIPT_SYNC_WAIT.as_millis() as u64,
        "submit_confirmation_elapsed_ms": measurement.delivery_elapsed_ms,
        "evidence_scope": INJECTION_EVIDENCE_SCOPE,
    })
}

#[derive(Deserialize)]
pub struct OperatorInjectRequest {
    pub target_agent_id: String,
    pub message: String,
    #[serde(default = "default_submit")]
    pub submit: bool,
}

#[derive(Deserialize)]
pub struct QueenInjectRequest {
    pub queen_id: String,
    pub target_worker_id: String,
    pub message: String,
    #[serde(default = "default_submit")]
    pub submit: bool,
}

#[derive(Deserialize)]
pub struct EvaluatorInjectRequest {
    pub evaluator_id: String,
    pub target_agent_id: String,
    pub message: String,
    #[serde(default = "default_submit")]
    pub submit: bool,
}

pub async fn operator_inject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<OperatorInjectRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&id)?;
    validate_agent_id(&payload.target_agent_id)?;

    let target_agent_id = payload.target_agent_id.clone();
    let transaction_lock = injection_transaction_lock(&state, &target_agent_id);
    let _transaction_guard = transaction_lock.lock().await;
    let pre_write_baseline = observe_pre_write_activity(&state, &target_agent_id).await;
    let injection_target_agent_id = target_agent_id.clone();
    let manager = Arc::clone(&state.injection_manager);
    let injection_session_id = id.clone();
    let injection_result = tokio::task::spawn_blocking(move || {
        manager.read().operator_inject(
            &injection_session_id,
            &injection_target_agent_id,
            &payload.message,
            payload.submit,
        )
    })
    .await
    .map_err(|error| ApiError::internal(format!("Injection task failed: {error}")))?;
    let mut receipt = injection_result.map_err(|error| ApiError::internal(error.to_string()))?;
    inherit_pending_nonce(&state, &id, &target_agent_id, &mut receipt);
    record_injection_submission(&state, &id, &target_agent_id, &receipt);
    let measurement =
        observe_injection_with_retry(&state, &id, &target_agent_id, &receipt, pre_write_baseline).await;

    Ok(Json(measured_response(
        format!("Operator injection sent to session {}", id),
        &receipt,
        &measurement,
    )))
}

pub async fn queen_inject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<QueenInjectRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&id)?;
    validate_agent_id(&payload.queen_id)?;
    validate_agent_id(&payload.target_worker_id)?;

    let target_worker_id = payload.target_worker_id.clone();
    let transaction_lock = injection_transaction_lock(&state, &target_worker_id);
    let _transaction_guard = transaction_lock.lock().await;
    let pre_write_baseline = observe_pre_write_activity(&state, &target_worker_id).await;
    let injection_target_worker_id = target_worker_id.clone();
    let manager = Arc::clone(&state.injection_manager);
    let injection_session_id = id.clone();
    let injection_result = tokio::task::spawn_blocking(move || {
        manager.read().queen_inject(
            &injection_session_id,
            &payload.queen_id,
            &injection_target_worker_id,
            &payload.message,
            payload.submit,
        )
    })
    .await
    .map_err(|error| ApiError::internal(format!("Injection task failed: {error}")))?;
    let mut receipt = injection_result.map_err(map_injection_error)?;
    inherit_pending_nonce(&state, &id, &target_worker_id, &mut receipt);
    record_injection_submission(&state, &id, &target_worker_id, &receipt);
    let measurement =
        observe_injection_with_retry(&state, &id, &target_worker_id, &receipt, pre_write_baseline).await;

    Ok(Json(measured_response(
        format!("Queen injection sent to session {}", id),
        &receipt,
        &measurement,
    )))
}

pub async fn evaluator_inject(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<EvaluatorInjectRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_session_id(&id)?;
    validate_agent_id(&payload.evaluator_id)?;
    validate_agent_id(&payload.target_agent_id)?;

    let target_agent_id = payload.target_agent_id.clone();
    let transaction_lock = injection_transaction_lock(&state, &target_agent_id);
    let _transaction_guard = transaction_lock.lock().await;
    let pre_write_baseline = observe_pre_write_activity(&state, &target_agent_id).await;
    let injection_target_agent_id = target_agent_id.clone();
    let manager = Arc::clone(&state.injection_manager);
    let injection_session_id = id.clone();
    let injection_result = tokio::task::spawn_blocking(move || {
        manager.read().evaluator_inject(
            &injection_session_id,
            &payload.evaluator_id,
            &injection_target_agent_id,
            &payload.message,
            payload.submit,
        )
    })
    .await
    .map_err(|error| ApiError::internal(format!("Injection task failed: {error}")))?;
    let mut receipt = injection_result.map_err(map_injection_error)?;
    inherit_pending_nonce(&state, &id, &target_agent_id, &mut receipt);
    record_injection_submission(&state, &id, &target_agent_id, &receipt);
    let measurement =
        observe_injection_with_retry(&state, &id, &target_agent_id, &receipt, pre_write_baseline).await;

    Ok(Json(measured_response(
        format!("Evaluator injection sent to session {}", id),
        &receipt,
        &measurement,
    )))
}

fn map_injection_error(error: InjectionError) -> ApiError {
    match error {
        InjectionError::NotAuthorized(message) => {
            ApiError::new(axum::http::StatusCode::FORBIDDEN, message)
        }
        InjectionError::AgentNotFound(message) | InjectionError::SessionNotFound(message) => {
            ApiError::not_found(message)
        }
        other => ApiError::internal(other.to_string()),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::coordination::{InjectionManager, QueueManager};
    use crate::domain::HiveExecutionPolicy;
    use crate::events::EventBus;
    use crate::pty::{AgentConfig, AgentRole, AgentStatus, PtyManager};
    use crate::session::{
        AgentInfo, AuthStrategy, Session, SessionController, SessionState, SessionType,
    };
    use crate::storage::{ApplicationStateDb, QueueRepo, SessionStorage};
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        routing::post,
        Router,
    };
    use chrono::{Duration as ChronoDuration, Utc};
    use parking_lot::RwLock;
    use serde_json::{json, Value};
    use tempfile::TempDir;
    use tower::ServiceExt;

    const SESSION_ID: &str = "inject-test";
    const AGENT_ID: &str = "inject-test-worker-1";
    const QUEEN_ID: &str = "inject-test-queen";
    const QA_WORKER_ID: &str = "inject-test-qa-worker-1";

    fn agent(id: &str, role: AgentRole) -> AgentInfo {
        AgentInfo {
            id: id.to_string(),
            role,
            status: AgentStatus::Running,
            config: AgentConfig::default(),
            parent_id: None,
            role_definition_id: None,
            role_definition_version: None,
            commit_sha: None,
            base_commit_sha: None,
            transcript_spawned_at: None,
            transcript_session_id: None,
        }
    }

    fn running_test_session(project_path: std::path::PathBuf) -> Session {
        let now = Utc::now();
        Session {
            id: SESSION_ID.to_string(),
            name: None,
            color: None,
            session_type: SessionType::Hive { worker_count: 1 },
            project_path,
            state: SessionState::Running,
            created_at: now,
            last_activity_at: now,
            agents: vec![
                agent(
                    AGENT_ID,
                    AgentRole::Worker {
                        index: 1,
                        parent: None,
                    },
                ),
                agent(QUEEN_ID, AgentRole::Queen),
                agent(
                    QA_WORKER_ID,
                    AgentRole::QaWorker {
                        index: 1,
                        parent: Some("inject-test-evaluator".to_string()),
                    },
                ),
            ],
            default_cli: "claude".to_string(),
            default_model: None,
            default_principal_cli: None,
            default_principal_model: None,
            default_principal_flags: Vec::new(),
            execution_policy: HiveExecutionPolicy::default(),
            qa_workers: Vec::new(),
            max_qa_iterations: 3,
            qa_timeout_secs: 300,
            qa_inconclusive_at: None,
            auth_strategy: AuthStrategy::default(),
            worktree_path: None,
            worktree_branch: None,
            no_git: true,
            resume_report: None,
        }
    }

    fn setup_test_app() -> (TempDir, Router, Arc<AppState>) {
        let temp_dir = TempDir::new().unwrap();
        let storage =
            Arc::new(SessionStorage::new_with_base(temp_dir.path().to_path_buf()).unwrap());
        storage.create_session_dir(SESSION_ID).unwrap();
        let config = Arc::new(tokio::sync::RwLock::new(storage.load_config().unwrap()));
        let pty_manager = Arc::new(RwLock::new(PtyManager::new()));
        pty_manager
            .write()
            .create_session(
                AGENT_ID.to_string(),
                AgentRole::Worker {
                    index: 1,
                    parent: None,
                },
                "claude",
                &[],
                None,
                80,
                24,
            )
            .unwrap();
        pty_manager
            .write()
            .create_session(
                QUEEN_ID.to_string(),
                AgentRole::Queen,
                "claude",
                &[],
                None,
                80,
                24,
            )
            .unwrap();
        pty_manager
            .write()
            .create_session(
                QA_WORKER_ID.to_string(),
                AgentRole::QaWorker {
                    index: 1,
                    parent: Some("inject-test-evaluator".to_string()),
                },
                "claude",
                &[],
                None,
                80,
                24,
            )
            .unwrap();
        let session_controller = Arc::new(RwLock::new(SessionController::new(pty_manager.clone())));
        session_controller.write().set_storage(storage.clone());
        session_controller
            .read()
            .insert_test_session(running_test_session(temp_dir.path().to_path_buf()));
        session_controller
            .read()
            .update_session_metadata(SESSION_ID, None, None)
            .unwrap();
        let injection_manager = Arc::new(RwLock::new(InjectionManager::new(
            pty_manager.clone(),
            SessionStorage::new_with_base(temp_dir.path().to_path_buf()).unwrap(),
        )));
        let event_bus = EventBus::new(storage.base_dir().clone());
        let app_state_db = Arc::new(ApplicationStateDb::open_in_memory().unwrap());
        let queue_repo = Arc::new(QueueRepo::new(app_state_db.clone()));
        queue_repo.ensure_schema().unwrap();
        let queue_manager = Arc::new(QueueManager::new(queue_repo, event_bus.clone()));
        let state = Arc::new(AppState::new(
            config,
            pty_manager,
            session_controller,
            injection_manager,
            storage,
            event_bus,
            app_state_db,
            queue_manager,
            None,
        ));
        let app = Router::new()
            .route("/api/sessions/{id}/inject", post(operator_inject))
            .route("/api/sessions/{id}/inject/queen", post(queen_inject))
            .route(
                "/api/sessions/{id}/inject/evaluator",
                post(evaluator_inject),
            )
            .with_state(state.clone());

        (temp_dir, app, state)
    }

    async fn post_inject(app: Router, path: &str, body: Value) -> (StatusCode, Value) {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, json)
    }

    fn support_codex(state: &AppState) {
        let mut session = state.session_controller.read().get_session(SESSION_ID).unwrap();
        session.agents.iter_mut().find(|agent| agent.id == AGENT_ID).unwrap().config.cli = "codex".into();
        state.session_controller.read().insert_test_session(session);
    }

    fn writes(state: &AppState) -> Vec<Vec<u8>> {
        state.pty_manager.read().write_records_for_test(AGENT_ID).unwrap()
    }

    async fn written_nonce(state: &AppState) -> String {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                for write in writes(state) {
                    let text = String::from_utf8_lossy(&write);
                    if let Some(start) = text.find("[hive-msg:") {
                        let rest = &text[start..];
                        if let Some(end) = rest.find(']') { return rest[..=end].to_owned(); }
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("submitted dynamic nonce")
    }

    fn transcript_path(state: &AppState) -> std::path::PathBuf {
        let dir = state.storage.base_dir().join("transcript-test-home/.codex/sessions")
            .join(chrono::Local::now().format("%Y/%m/%d").to_string());
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("rollout-http-fixture.jsonl")
    }

    fn write_transcript(state: &AppState, nonce: &str, started: bool) {
        let timestamp = Utc::now().to_rfc3339();
        let mut text = format!("{}\n", json!({"timestamp": timestamp, "type":"response_item", "payload":{
            "role":"user", "content":[{"type":"input_text","text":format!("Fixture request\n{nonce}")}]}}));
        if started {
            text.push_str(&format!("{}\n", json!({"timestamp":timestamp,"type":"event_msg","payload":{
                "type":"task_started","turn_attribution":{"turn_id":"http-fixture-turn"}}})));
        }
        std::fs::write(transcript_path(state), text).unwrap();
    }

    fn stage_after_write(state: Arc<AppState>) -> tokio::task::JoinHandle<String> {
        tokio::spawn(async move {
            let nonce = written_nonce(&state).await;
            // Explicit composer prompt is required: raw stdin echo is insufficient.
            tokio::time::sleep(Duration::from_millis(100)).await;
            state.pty_manager.read().record_output_for_test(AGENT_ID, format!("\n> {nonce}").as_bytes());
            nonce
        })
    }

    #[tokio::test]
    async fn operator_inject_omitted_submit_defaults_to_true() {
        let (_temp, app, state) = setup_test_app();
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"hello\r\n"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_attempted"], true);
        assert_eq!(response["submit_keystroke_issued"], true);
        assert_eq!(response["submit_attempts"], 1);
        assert_eq!(response["submit_bytes_written"], 1);
        assert_eq!(response["submit_confirmed"], Value::Null);
        assert_eq!(response["submit_confirmation_basis"], "transcript-unavailable");
        assert_eq!(response["submit_confirmation_source"], "none");
        assert_eq!(response["submit_retry_failed"], false);
        assert_eq!(response["submit_retry_keystroke"], Value::Null, "Claude path unchanged");
        assert_eq!(response["receiver_turn_running"], Value::Null);
        let records = writes(&state);
        assert_eq!(records[0], b"\x1b[200~");
        assert_eq!(records[2], b"\x1b[201~");
        assert_eq!(records[3], b"\r");
        let payload = String::from_utf8(records[1].clone()).unwrap();
        assert!(payload.starts_with("hello\n[hive-msg:"));
        assert_eq!(response["payload_bytes_written"], payload.len());
        assert!(response["write_started_at"].is_string());
        assert!(response["submitted_at"].is_string());
    }

    #[tokio::test]
    async fn operator_inject_submit_false_writes_payload_without_enter_or_nonce() {
        let (_temp, app, state) = setup_test_app();
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"draft\r\n","submit":false})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["payload_bytes_written"], 5);
        assert_eq!(response["submit_attempts"], 0);
        assert_eq!(response["submit_keystroke_issued"], false);
        assert_eq!(response["submit_bytes_written"], 0);
        assert_eq!(response["submit_confirmed"], Value::Null);
        assert_eq!(response["submit_confirmation_basis"], "transcript-unavailable");
        assert_eq!(writes(&state), vec![b"draft".to_vec()]);
    }

    #[tokio::test]
    async fn operator_inject_large_multiline_payload_brackets_once_and_reports_sanitized_bytes() {
        let (_temp, app, state) = setup_test_app();
        let payload = format!("{}\nline two\u{1b}[201~safe\r\n", "p".repeat(1287));
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":payload})).await;
        assert_eq!(status, StatusCode::OK);
        let records = writes(&state);
        assert_eq!(records.len(), 4);
        assert_eq!(records[0], b"\x1b[200~");
        assert_eq!(records[2], b"\x1b[201~");
        assert_eq!(records[3], b"\r");
        let actual = String::from_utf8(records[1].clone()).unwrap();
        assert!(actual.starts_with(&format!("{}\nline twosafe\n[hive-msg:", "p".repeat(1287))));
        assert_eq!(response["payload_bytes_written"], actual.len());
    }

    #[tokio::test]
    async fn operator_inject_unassociated_empty_message_is_bare_enter_and_unknown() {
        let (_temp, app, state) = setup_test_app();
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":""})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_attempts"], 1);
        assert_eq!(response["payload_bytes_written"], 0);
        assert_eq!(response["submit_bytes_written"], 1);
        assert_eq!(response["submit_confirmed"], Value::Null);
        assert_eq!(response["submit_confirmation_basis"], "transcript-unavailable");
        assert_eq!(writes(&state), vec![b"\r".to_vec()]);
    }

    #[tokio::test]
    async fn pending_message_empty_flush_reuses_prior_nonce_without_writing_new_marker() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let (_, pending) = post_inject(app.clone(), "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"pending fixture"})).await;
        assert_eq!(pending["submit_confirmation_basis"], "transcript-pending");
        let nonce = written_nonce(&state).await;
        // This user timestamp predates the flush. Confirmation must reuse the
        // original message's evidence window, rather than the flush timestamp.
        write_transcript(&state, &nonce, false);
        let before = writes(&state);
        let fixture_state = state.clone();
        let before_len = before.len();
        let turn = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(3), async {
                while writes(&fixture_state).len() == before_len {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.expect("bare flush write");
            let line = format!("{}\n", json!({"timestamp":Utc::now().to_rfc3339(),"type":"event_msg","payload":{
                "type":"task_started","turn_attribution":{"turn_id":"http-fixture-turn"}}}));
            let mut file = std::fs::OpenOptions::new().append(true).open(transcript_path(&fixture_state)).unwrap();
            std::io::Write::write_all(&mut file, line.as_bytes()).unwrap();
        });
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":""})).await;
        turn.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["payload_bytes_written"], 0);
        assert_eq!(response["submit_bytes_written"], 1);
        assert_eq!(response["submit_confirmed"], true);
        assert_eq!(response["turn_id"], "http-fixture-turn");
        let after = writes(&state);
        assert_eq!(&after[..before.len()], before.as_slice());
        assert_eq!(&after[before.len()..], &[b"\r".to_vec()]);
    }

    #[tokio::test]
    async fn operator_inject_blocking_path_preserves_pty_error_status() {
        let (_temp, app, _state) = setup_test_app();
        let (status, _) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":"missing-agent","message":"hello"})).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn all_inject_routes_return_transcript_receipt_contract() {
        let (_temp, app, _state) = setup_test_app();
        for (path, body) in [
            ("/api/sessions/inject-test/inject", json!({"target_agent_id":AGENT_ID,"message":"operator"})),
            ("/api/sessions/inject-test/inject/queen", json!({"queen_id":QUEEN_ID,"target_worker_id":AGENT_ID,"message":"queen"})),
            ("/api/sessions/inject-test/inject/evaluator", json!({"evaluator_id":"inject-test-evaluator","target_agent_id":QUEEN_ID,"message":"evaluator"})),
        ] {
            let (status, response) = post_inject(app.clone(), path, body).await;
            assert_eq!(status, StatusCode::OK, "{path}: {response}");
            assert_eq!(response["submit_attempts"], 1);
            assert_eq!(response["submit_bytes_written"], 1);
            assert_eq!(response["submit_confirmed"], Value::Null);
            assert_eq!(response["submit_confirmation_source"], "none");
            assert_eq!(response["submit_confirmation_basis"], "transcript-unavailable");
            assert!(response["submit_screen_activity_basis"].is_string());
        }
    }

    #[tokio::test]
    async fn codex_nonce_user_then_turn_confirms_real_fixture() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let fixture_state = state.clone();
        let fixture = tokio::spawn(async move {
            let nonce = written_nonce(&fixture_state).await;
            write_transcript(&fixture_state, &nonce, true);
        });
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"fixture request"})).await;
        fixture.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_confirmed"], true);
        assert_eq!(response["submit_confirmation_basis"], "transcript-confirmed");
        assert_eq!(response["submit_confirmation_source"], "transcript");
        assert_eq!(response["turn_id"], "http-fixture-turn");
        assert_eq!(response["submit_attempts"], 1);
    }

    #[tokio::test]
    async fn g_false_positive_one_hz_ticker_with_swallowed_enter_never_confirms() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let ticker_state = state.clone();
        let ticker = tokio::spawn(async move {
            written_nonce(&ticker_state).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            for tick in 0..3 {
                ticker_state.pty_manager.read().record_output_for_test(AGENT_ID, format!("\nticker-{tick}").as_bytes());
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"swallowed Enter"})).await;
        ticker.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["receiver_busy_before_write"], false);
        assert_eq!(response["submit_screen_activity"], true, "ticker must exercise old false-positive path: {response}");
        assert_ne!(response["submit_confirmed"], true);
        assert_eq!(response["submit_confirmation_basis"], "transcript-pending");
        assert_eq!(response["submit_attempts"], 2, "a silent Codex transcript earns one queue-key retry");
        assert_eq!(response["submit_retry_keystroke"], "tab");
        assert_eq!(response["receiver_turn_running"], Value::Null, "no title in the tail");
    }

    #[tokio::test]
    async fn supported_missing_transcript_and_visible_staging_retries_once() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let staged = stage_after_write(state.clone());
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"retry fixture"})).await;
        staged.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_confirmed"], Value::Null);
        assert_eq!(response["submit_confirmation_basis"], "transcript-pending");
        assert_eq!(response["submit_attempts"], 2);
        assert_eq!(response["submit_bytes_written"], 2);
        assert_eq!(writes(&state).iter().filter(|w| w.as_slice() == b"\r").count(), 1);
        assert_eq!(writes(&state).iter().filter(|w| w.as_slice() == b"\t").count(), 1);
    }

    #[tokio::test]
    async fn recorded_user_without_turn_suppresses_retry_despite_staged_text() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let fixture_state = state.clone();
        let fixture = tokio::spawn(async move {
            let nonce = stage_after_write(fixture_state.clone()).await.unwrap();
            write_transcript(&fixture_state, &nonce, false);
        });
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"queued fixture"})).await;
        fixture.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_confirmation_basis"], "transcript-pending");
        assert_eq!(response["submit_attempts"], 1);
    }

    #[tokio::test]
    async fn concurrent_injects_to_same_agent_do_not_cross_submit_staged_text() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let staged_screen = stage_after_write(state.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();
        install_before_submit_retry_hook(&state, async move {
            let _ = tx.send(());
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        let submit_app = app.clone();
        let submitting = tokio::spawn(async move {
            post_inject(submit_app, "/api/sessions/inject-test/inject", json!({"target_agent_id":AGENT_ID,"message":"first"})).await
        });
        tokio::time::timeout(TRANSCRIPT_SYNC_WAIT + Duration::from_secs(3), rx).await.unwrap().unwrap();
        let staging = tokio::spawn(async move {
            post_inject(app, "/api/sessions/inject-test/inject", json!({"target_agent_id":AGENT_ID,"message":"draft","submit":false})).await
        });
        let ((status, response), (stage_status, stage_response)) = (submitting.await.unwrap(), staging.await.unwrap());
        staged_screen.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(stage_status, StatusCode::OK);
        assert_eq!(response["submit_attempts"], 2);
        assert_eq!(stage_response["submit_attempts"], 0);
        let records = writes(&state);
        assert_eq!(&records[records.len()-2..], &[b"\t".to_vec(), b"draft".to_vec()]);
    }

    #[tokio::test]
    async fn failed_submit_retry_returns_success_and_records_initial_submission() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let staged = stage_after_write(state.clone());
        let pty = state.pty_manager.clone();
        install_before_submit_retry_hook(&state, async move { pty.read().kill(AGENT_ID).unwrap(); });
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"go"})).await;
        staged.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["status"], "success");
        assert_eq!(response["submit_attempts"], 1);
        assert_eq!(response["submit_retry_failed"], true);
        assert!(response["submit_retry_failure"].as_str().unwrap().contains("Submit retry failed"));
        assert_eq!(response["submit_confirmed"], Value::Null);
        assert!(matches!(state.session_controller.read().get_session(SESSION_ID).unwrap().agents
            .iter().find(|agent| agent.id == AGENT_ID).unwrap().status,
            AgentStatus::WaitingForInput(_)));
    }

    #[tokio::test]
    async fn pending_background_watch_updates_status_after_late_turn() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"late fixture"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_confirmation_basis"], "transcript-pending");
        assert_eq!(response["submit_attempts"], 2, "a silent Codex transcript earns one queue-key retry");
        let nonce = written_nonce(&state).await;
        write_transcript(&state, &nonce, true);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let status = state.session_controller.read().get_session(SESSION_ID).unwrap().agents
                    .iter().find(|agent| agent.id == AGENT_ID).unwrap().status.clone();
                if matches!(status, AgentStatus::Running) { break; }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.expect("background transcript confirmation updates agent status");
    }

    fn short_watch_fixture(state: &Arc<AppState>, user_recorded: bool) -> (InjectionReceipt, SharedConfirmation) {
        support_codex(state);
        let receipt = state.injection_manager.read().write_to_agent(AGENT_ID, "watch fixture", true).unwrap();
        record_injection_submission(state, SESSION_ID, AGENT_ID, &receipt);
        let nonce = receipt.nonce.as_deref().unwrap();
        state.pty_manager.read().record_output_for_test(AGENT_ID, format!("\n> {nonce}").as_bytes());
        if user_recorded { write_transcript(state, nonce, false); }
        let since = SystemTime::from(receipt.confirmation_since.unwrap_or(receipt.write_started_at));
        let reader = Arc::new(parking_lot::Mutex::new(confirmation_reader(state, SESSION_ID, AGENT_ID, since)));
        (receipt, reader)
    }

    async fn wait_status_reason(state: &AppState, expected: &str) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let status = state.session_controller.read().get_session(SESSION_ID).unwrap().agents
                    .iter().find(|agent| agent.id == AGENT_ID).unwrap().status.clone();
                if matches!(status, AgentStatus::WaitingForInput(ref reason) if reason == expected) { break; }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("bounded watch status transition");
    }

    #[tokio::test]
    async fn short_watch_absent_message_and_visible_staging_reports_failure() {
        let (_temp, _app, state) = setup_test_app();
        let (receipt, reader) = short_watch_fixture(&state, false);
        watch_pending_delivery(&state, SESSION_ID, AGENT_ID, &receipt, reader, Duration::from_millis(300), false);
        wait_status_reason(&state, &format!("injected_delivery_failed:transcript-timeout:{}", receipt.submitted_at.to_rfc3339())).await;
        state.session_controller.read().update_heartbeat(SESSION_ID, AGENT_ID, "working", None).unwrap();
        state.session_controller.read().get_stalled_agent_reports(SESSION_ID, Duration::from_secs(30));
        let status = state.session_controller.read().get_session(SESSION_ID).unwrap().agents
            .into_iter().find(|agent| agent.id == AGENT_ID).unwrap().status;
        assert!(matches!(status, AgentStatus::Running), "newer heartbeat clears transcript timeout failure");
    }

    #[tokio::test]
    async fn short_watch_recorded_user_without_turn_reports_unknown_even_when_staged() {
        let (_temp, _app, state) = setup_test_app();
        let (receipt, reader) = short_watch_fixture(&state, true);
        watch_pending_delivery(&state, SESSION_ID, AGENT_ID, &receipt, reader.clone(), Duration::from_millis(300), false);
        tokio::time::timeout(Duration::from_secs(2), async {
            while Arc::strong_count(&reader) != 1 { tokio::time::sleep(Duration::from_millis(20)).await; }
        }).await.expect("unknown watch completed without replacing recovery marker");
        wait_status_reason(&state, &format!("awaiting_injected_turn:{}", receipt.submitted_at.to_rfc3339())).await;
    }

    #[tokio::test]
    async fn deleting_pty_cancels_pending_watch_and_releases_reader() {
        let (_temp, _app, state) = setup_test_app();
        let (receipt, reader) = short_watch_fixture(&state, false);
        watch_pending_delivery(&state, SESSION_ID, AGENT_ID, &receipt, reader.clone(), Duration::from_millis(300), false);
        state.pty_manager.read().kill(AGENT_ID).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while Arc::strong_count(&reader) != 1 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("deleted agent must release its watch reader");
        let status = state.session_controller.read().get_session(SESSION_ID).unwrap().agents
            .iter().find(|agent| agent.id == AGENT_ID).unwrap().status.clone();
        assert!(matches!(status, AgentStatus::WaitingForInput(reason) if reason.starts_with("awaiting_injected_turn:")));
    }

    #[test]
    fn submit_confirmation_classifier_distinguishes_all_three_verdicts() {
        use std::time::Duration;

        assert_eq!(
            classify_screen_activity(&[], &[]),
            (Some(false), "no-post-submit-activity")
        );
        assert_eq!(
            classify_screen_activity(&[Duration::from_millis(40)], &[]),
            (None, "ambiguous-post-submit-activity"),
            "a single repaint also matches a swallowed Enter"
        );
        assert_eq!(
            classify_screen_activity(
                &[Duration::from_millis(40), Duration::from_millis(120)],
                &[]
            ),
            (None, "ambiguous-post-submit-activity"),
            "a short burst is not sustained activity"
        );
        assert_eq!(
            classify_screen_activity(
                &[
                    Duration::from_millis(40),
                    Duration::from_millis(120),
                    Duration::from_millis(290)
                ],
                &[]
            ),
            (Some(true), "sustained-post-submit-activity")
        );

        let busy_baseline = [Duration::from_millis(40), Duration::from_millis(180)];
        assert_eq!(
            classify_screen_activity(&[], &busy_baseline),
            (Some(false), "no-post-submit-activity"),
            "a busy baseline does not change a confident negative"
        );
        assert_eq!(
            classify_screen_activity(&[Duration::from_millis(40)], &busy_baseline),
            (None, "ambiguous-post-submit-activity"),
            "a busy baseline does not change an ambiguous verdict"
        );
        assert_eq!(
            classify_screen_activity(
                &[
                    Duration::from_millis(40),
                    Duration::from_millis(120),
                    Duration::from_millis(290)
                ],
                &busy_baseline
            ),
            (None, "busy-receiver-indeterminate"),
            "pre-write streaming invalidates only the would-be confident positive"
        );
    }

    #[test]
    fn short_pre_write_burst_does_not_suppress_confident_positive() {
        let sustained_submit = [
            Duration::from_millis(40),
            Duration::from_millis(120),
            Duration::from_millis(290),
        ];
        let short_pre_write_burst = [Duration::from_millis(40), Duration::from_millis(120)];

        assert_eq!(
            classify_screen_activity(&sustained_submit, &short_pre_write_burst),
            (Some(true), "sustained-post-submit-activity"),
            "an 80 ms pre-write burst is shorter than half the 250 ms baseline window"
        );
    }

    #[test]
    fn stall_report_distinguishes_awaiting_inject_from_crashed_and_idle() {
        let (_temp_dir, _app, state) = setup_test_app();
        let controller = state.session_controller.read();
        controller
            .update_heartbeat(SESSION_ID, AGENT_ID, "working", None)
            .unwrap();
        controller
            .update_heartbeat(SESSION_ID, QUEEN_ID, "working", None)
            .unwrap();
        controller
            .update_heartbeat(SESSION_ID, QA_WORKER_ID, "idle", None)
            .unwrap();

        let stale_at = Utc::now() - ChronoDuration::minutes(5);
        for agent_id in [AGENT_ID, QUEEN_ID, QA_WORKER_ID] {
            controller.set_heartbeat_last_activity_for_test(SESSION_ID, agent_id, stale_at);
        }
        controller
            .record_injection_submission(
                SESSION_ID,
                AGENT_ID,
                stale_at + ChronoDuration::seconds(1),
                true,
            )
            .unwrap();
        state.pty_manager.read().kill(QUEEN_ID).unwrap();

        let reports =
            controller.get_stalled_agent_reports(SESSION_ID, std::time::Duration::from_secs(30));
        let status_for = |agent_id: &str| {
            reports
                .iter()
                .find(|(id, _, _)| id == agent_id)
                .map(|(_, _, status)| status)
                .unwrap()
        };

        assert_eq!(
            status_for(AGENT_ID),
            &AgentStatus::WaitingForInput("awaiting_injected_turn".to_string())
        );
        assert_eq!(
            status_for(QUEEN_ID),
            &AgentStatus::Error("process_not_alive".to_string())
        );
        assert_eq!(status_for(QA_WORKER_ID), &AgentStatus::Idle);
    }
    #[tokio::test]
    async fn cursor_inject_unknown_status_recovers_after_newer_heartbeat() {
        let (_temp, app, state) = setup_test_app();
        let mut session = state.session_controller.read().get_session(SESSION_ID).unwrap();
        session.agents.iter_mut().find(|agent| agent.id == AGENT_ID).unwrap().config.cli = "cursor".into();
        state.session_controller.read().insert_test_session(session);
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"unsupported runtime fixture"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_confirmation_basis"], "transcript-unavailable");
        let before = state.session_controller.read().get_session(SESSION_ID).unwrap().agents
            .into_iter().find(|agent| agent.id == AGENT_ID).unwrap().status;
        assert!(matches!(before, AgentStatus::WaitingForInput(reason) if reason.starts_with("awaiting_injected_turn:")));
        state.session_controller.read().update_heartbeat(SESSION_ID, AGENT_ID, "working", None).unwrap();
        state.session_controller.read().get_stalled_agent_reports(SESSION_ID, Duration::from_secs(30));
        let after = state.session_controller.read().get_session(SESSION_ID).unwrap().agents
            .into_iter().find(|agent| agent.id == AGENT_ID).unwrap().status;
        assert!(matches!(after, AgentStatus::Running));
    }

    #[tokio::test]
    async fn long_lived_dynamic_worker_uses_its_spawn_bucket_not_injection_or_session_date() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let spawned_at = Utc::now() - ChronoDuration::days(3);
        let mut session = state.session_controller.read().get_session(SESSION_ID).unwrap();
        session.created_at = Utc::now() - ChronoDuration::days(5);
        session.agents.iter_mut().find(|agent| agent.id == AGENT_ID).unwrap()
            .transcript_spawned_at = Some(spawned_at);
        state.session_controller.read().insert_test_session(session);
        let fixture_state = state.clone();
        let fixture = tokio::spawn(async move {
            let nonce = written_nonce(&fixture_state).await;
            let dir = fixture_state.storage.base_dir().join("transcript-test-home/.codex/sessions")
                .join(spawned_at.with_timezone(&chrono::Local).format("%Y/%m/%d").to_string());
            std::fs::create_dir_all(&dir).unwrap();
            let timestamp = Utc::now().to_rfc3339();
            let user = json!({"timestamp":timestamp,"type":"response_item","payload":{
                "role":"user","content":[{"type":"input_text","text":format!("Fixture request\n{nonce}")}]}});
            let turn = json!({"timestamp":timestamp,"type":"event_msg","payload":{
                "type":"task_started","turn_attribution":{"turn_id":"long-lived-http-turn"}}});
            std::fs::write(dir.join("rollout-long-lived-fixture.jsonl"), format!("{user}\n{turn}\n")).unwrap();
        });
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"old dynamic worker fixture"})).await;
        fixture.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_confirmed"], true);
        assert_eq!(response["turn_id"], "long-lived-http-turn");
    }

    /// Codex 0.165 receiver driven by the sender's recorded PTY writes. Rules follow codex-rs
    /// `bottom_pane/chat_composer.rs` and `chatwidget/input_flow.rs` at rust-v0.165.0:
    /// - Enter submits, as a steer when a turn runs, unless the paste-burst check (evaluated
    ///   when Codex dequeues the key) still counts it as part of the paste; then it inserts a
    ///   newline. `swallow_enter` models a busy TUI that drained the paste and Enter together.
    /// - Tab queues the draft while a turn runs, submits when idle, ignores an empty composer.
    /// - Rollouts record `task_started` before the turn's user item; a queued draft starts the
    ///   next turn when the running one ends.
    #[derive(Default)]
    struct FakeCodex {
        seen: usize,
        composer: String,
        queued: Vec<String>,
        turn: Option<String>,
        turns: usize,
        swallow_enter: bool,
    }

    impl FakeCodex {
        fn pump(&mut self, state: &AppState) {
            let records = writes(state);
            for record in records.iter().skip(self.seen) {
                match record.as_slice() {
                    b"\x1b[200~" | b"\x1b[201~" => {}
                    b"\r" if self.swallow_enter => self.composer.push('\n'),
                    b"\r" => self.submit(state, false),
                    b"\t" => self.submit(state, true),
                    payload => self.composer.push_str(&String::from_utf8_lossy(payload)),
                }
            }
            self.seen = records.len();
        }

        fn submit(&mut self, state: &AppState, queue_key: bool) {
            let text = std::mem::take(&mut self.composer).trim().to_owned();
            if text.is_empty() {
                return;
            }
            match (self.turn.is_some(), queue_key) {
                (true, true) => self.queued.push(text),
                (true, false) => append_rollout(state, &user_record(&text)),
                (false, _) => self.start_turn(state, Some(text)),
            }
        }

        fn start_turn(&mut self, state: &AppState, text: Option<String>) {
            self.turns += 1;
            let turn = format!("fake-turn-{}", self.turns);
            append_rollout(state, &json!({"timestamp":Utc::now().to_rfc3339(),"type":"event_msg",
                "payload":{"type":"task_started","turn_attribution":{"turn_id":turn.as_str()}}}));
            if let Some(text) = text {
                append_rollout(state, &user_record(&text));
            }
            self.turn = Some(turn);
            state.pty_manager.read().record_output_for_test(AGENT_ID, "\x1b]0;\u{280b} Working | fake\x07".as_bytes());
        }

        fn end_turn(&mut self, state: &AppState) {
            if let Some(turn) = self.turn.take() {
                append_rollout(state, &json!({"timestamp":Utc::now().to_rfc3339(),"type":"event_msg",
                    "payload":{"type":"task_complete","turn_id":turn}}));
            }
            state.pty_manager.read().record_output_for_test(AGENT_ID, b"\x1b]0;Working | fake\x07");
            if !self.queued.is_empty() {
                let next = self.queued.remove(0);
                self.start_turn(state, Some(next));
            }
        }
    }

    fn user_record(text: &str) -> Value {
        json!({"timestamp":Utc::now().to_rfc3339(),"type":"response_item","payload":{
            "type":"message","role":"user","content":[{"type":"input_text","text":text}]}})
    }

    fn append_rollout(state: &AppState, record: &Value) {
        let mut file = std::fs::OpenOptions::new().create(true).append(true)
            .open(transcript_path(state)).unwrap();
        std::io::Write::write_all(&mut file, format!("{record}\n").as_bytes()).unwrap();
    }

    fn user_records_with(state: &AppState, nonce: &str) -> usize {
        std::fs::read_to_string(transcript_path(state)).unwrap_or_default().lines()
            .filter(|line| line.contains("\"role\":\"user\"") && line.contains(nonce))
            .count()
    }

    fn spawn_fake_codex(state: &Arc<AppState>, fake: FakeCodex) -> Arc<parking_lot::Mutex<FakeCodex>> {
        let fake = Arc::new(parking_lot::Mutex::new(fake));
        let (weak, shared) = (Arc::downgrade(state), fake.clone());
        tokio::spawn(async move {
            while let Some(state) = weak.upgrade() {
                shared.lock().pump(&state);
                drop(state);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        fake
    }

    fn keystrokes(state: &AppState, key: &[u8]) -> usize {
        writes(state).iter().filter(|write| write.as_slice() == key).count()
    }

    #[tokio::test]
    async fn busy_codex_swallowed_enter_is_queued_and_delivered_once_after_the_turn() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let fake = spawn_fake_codex(&state, FakeCodex { swallow_enter: true, ..Default::default() });
        fake.lock().start_turn(&state, None);
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"relay while busy"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_attempts"], 2);
        assert_eq!(response["submit_retry_keystroke"], "tab");
        assert_eq!(response["receiver_turn_running"], true);
        assert_eq!(response["submit_confirmed"], Value::Null);
        assert_eq!(response["submit_confirmation_basis"], "transcript-pending-busy");
        let nonce = written_nonce(&state).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while fake.lock().queued.is_empty() { tokio::time::sleep(Duration::from_millis(10)).await; }
        }).await.expect("queue key moves the stranded draft into Codex's queue");
        assert!(fake.lock().composer.is_empty());
        assert_eq!(user_records_with(&state, &nonce), 0, "nothing is delivered mid-turn");

        fake.lock().end_turn(&state);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let status = state.session_controller.read().get_session(SESSION_ID).unwrap().agents
                    .iter().find(|agent| agent.id == AGENT_ID).unwrap().status.clone();
                if matches!(status, AgentStatus::Running) { break; }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.expect("background watch confirms the queued turn");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(user_records_with(&state, &nonce), 1, "delivered exactly once");
        assert_eq!((keystrokes(&state, b"\r"), keystrokes(&state, b"\t")), (1, 1));
    }

    #[tokio::test]
    async fn idle_codex_enter_submits_once_without_retry() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let _fake = spawn_fake_codex(&state, FakeCodex::default());
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"idle relay"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_confirmed"], true, "task_started before the user item: {response}");
        assert_eq!(response["turn_id"], "fake-turn-1");
        assert_eq!(response["submit_attempts"], 1);
        assert_eq!(response["submit_retry_keystroke"], Value::Null);
        let records = writes(&state);
        assert_eq!(records.len(), 4);
        assert_eq!((records[0].as_slice(), records[2].as_slice(), records[3].as_slice()),
            (&b"\x1b[200~"[..], &b"\x1b[201~"[..], &b"\r"[..]));
        assert_eq!(user_records_with(&state, &written_nonce(&state).await), 1);
    }

    #[tokio::test]
    async fn busy_codex_accepted_enter_is_a_steer_confirmed_in_the_running_turn() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let fake = spawn_fake_codex(&state, FakeCodex::default());
        fake.lock().start_turn(&state, None);
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"steer relay"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_confirmed"], true, "{response}");
        assert_eq!(response["turn_id"], "fake-turn-1");
        assert_eq!(response["submit_attempts"], 1);
        assert_eq!(keystrokes(&state, b"\t"), 0);
    }

    #[tokio::test]
    async fn codex_turn_ending_before_the_retry_submits_exactly_once() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let fake = spawn_fake_codex(&state, FakeCodex { swallow_enter: true, ..Default::default() });
        fake.lock().start_turn(&state, None);
        let (hook_state, hook_fake) = (state.clone(), fake.clone());
        install_before_submit_retry_hook(&state, async move {
            hook_fake.lock().end_turn(&hook_state);
        });
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"relay racing turn end"})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["receiver_turn_running"], false);
        assert_eq!(response["submit_retry_keystroke"], "tab");
        assert_eq!(response["submit_confirmed"], true, "idle Tab submits at once: {response}");
        assert_eq!(response["turn_id"], "fake-turn-2");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let nonce = written_nonce(&state).await;
        assert_eq!(user_records_with(&state, &nonce), 1, "no duplicate submission");
        assert!(fake.lock().queued.is_empty() && fake.lock().composer.is_empty());
        assert_eq!((keystrokes(&state, b"\r"), keystrokes(&state, b"\t")), (1, 1));
    }

    #[tokio::test]
    async fn codex_empty_flush_writes_exactly_one_bare_enter() {
        let (_temp, app, state) = setup_test_app();
        support_codex(&state);
        let (status, response) = post_inject(app, "/api/sessions/inject-test/inject",
            json!({"target_agent_id":AGENT_ID,"message":"","submit":true})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["submit_attempts"], 1);
        assert_eq!(response["submit_retry_keystroke"], Value::Null);
        assert_eq!(response["submit_confirmation_basis"], "transcript-unavailable");
        assert_eq!(writes(&state), vec![b"\r".to_vec()]);
    }

}
