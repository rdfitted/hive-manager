//! Folder-trust prompt sweep (#466): notify only.
//!
//! Spawns into a seat's own Hive-managed worktree pre-trust it where the CLI allows
//! (`cli::folder_trust`). This sweep covers the rest: it renders each PTY agent's new
//! output and, when a trust prompt is up, sets `WaitingForInput("trust_prompt")`, emits
//! an event and tells the Queen and the operator. It never sends a keystroke: a screen
//! match cannot prove which dialog has the terminal's input (Ryan, 2026-11-27). When the
//! prompt goes, whoever answered it, the status the prompt replaced is put back.
//!
//! Reads and status changes are bound to the PTY instance a guard was made for. A
//! same-id respawn gets a fresh guard, and a status its predecessor's prompt left behind
//! is put back.

use std::time::Instant;

use super::*;
use crate::pty::trust_prompt::{GuardAction, TrustCli, TrustPromptGuard, TRUST_PROMPT_STATUS};

/// Grid used when a PTY's size was never recorded (it is recorded at every spawn).
const FALLBACK_SIZE: (u16, u16) = (120, 40);

/// What the sweep did about one agent's trust prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustPromptOutcome {
    pub agent_id: String,
    pub adapter: &'static str,
    /// The agent's known spawn folder, never text read off its screen.
    pub path: String,
    /// `detected` or `cleared`.
    pub action: &'static str,
}

/// What one agent's observation found.
enum Observation {
    Action(Uuid, GuardAction),
    /// The PTY was replaced since the last sweep (same-id respawn).
    Replaced,
}

impl SessionController {
    /// Check every live PTY agent of `session_id` for a folder-trust prompt.
    pub fn sweep_trust_prompts(&self, session_id: &str) -> Vec<TrustPromptOutcome> {
        self.sweep_trust_prompts_at(session_id, Instant::now())
    }

    /// Sweep every session that is not finished, including ones waiting on fusion
    /// variants or debate rounds, whose agents may be the ones stopped on a prompt; drop
    /// the guards of the rest. Runs on the desktop's five-second monitoring loop.
    pub fn sweep_live_trust_prompts(&self) -> Vec<TrustPromptOutcome> {
        let live: Vec<String> = self
            .list_sessions()
            .iter()
            .filter(|session| !session.state.is_terminal())
            .map(|session| session.id.clone())
            .collect();
        self.prune_trust_guards(&live);
        live.iter().flat_map(|session_id| self.sweep_trust_prompts(session_id)).collect()
    }

    /// Drop the guards of sessions that are no longer swept, with their screens.
    pub fn prune_trust_guards(&self, swept_sessions: &[String]) {
        self.trust_guards.write().retain(|(session_id, _), _| swept_sessions.contains(session_id));
        self.trust_prior_status.write().retain(|(session_id, _), _| swept_sessions.contains(session_id));
    }

    pub(crate) fn sweep_trust_prompts_at(&self, session_id: &str, now: Instant) -> Vec<TrustPromptOutcome> {
        let Some(session) = self.sessions.read().get(session_id).cloned() else {
            return Vec::new();
        };
        self.trust_guards.write().retain(|(sid, aid), _| {
            sid != session_id || session.agents.iter().any(|agent| &agent.id == aid)
        });
        let mut outcomes = Vec::new();
        for agent in &session.agents {
            if matches!(agent.status, AgentStatus::Completed | AgentStatus::Error(_)) {
                continue;
            }
            let Some(cli) = TrustCli::from_cli(&agent.config.cli) else {
                continue;
            };
            match self.observe_trust_prompt(&session.id, &agent.id, cli, now) {
                Some(Observation::Action(instance, action)) => {
                    outcomes.extend(self.report_trust_prompt(&session, agent, instance, cli, action));
                }
                Some(Observation::Replaced) => self.drop_inherited_trust_status(&session.id, &agent.id),
                None => {}
            }
        }
        outcomes
    }

    fn observe_trust_prompt(&self, session_id: &str, agent_id: &str, cli: TrustCli, now: Instant) -> Option<Observation> {
        let manager = self.pty_manager.read();
        if manager.protocol_agent(agent_id).is_some() {
            return None;
        }
        let instance = manager.instance_id(agent_id)?;
        let mut guards = self.trust_guards.write();
        let key = (session_id.to_string(), agent_id.to_string());
        let previous = guards.get(&key).map(|(seen, _)| *seen);
        if previous != Some(instance) {
            // Measure the watch window from the spawn, not from the first sweep.
            let age = manager
                .spawned_at(agent_id)
                .and_then(|at| (Utc::now() - at).to_std().ok())
                .unwrap_or_default();
            let started = now.checked_sub(age).unwrap_or(now);
            let (cols, rows) = manager.pty_size(agent_id).unwrap_or(FALLBACK_SIZE);
            guards.insert(key.clone(), (instance, TrustPromptGuard::new(cli, cols, rows, started)));
            if previous.is_some() {
                return Some(Observation::Replaced);
            }
        }
        let guard = &mut guards.get_mut(&key)?.1;
        if !guard.is_active(now) || !manager.is_alive(agent_id) {
            // Expired, or a dead seat that exit handling owns: release the screen. A
            // prompt surfaced on a seat that died is cleared by the dead-seat path.
            guard.finish();
            return None;
        }
        let snapshot = manager.output_since(agent_id, instance, guard.fed_to())?;
        let (cols, rows) = manager.pty_size(agent_id).unwrap_or(FALLBACK_SIZE);
        let action = guard.observe(&snapshot, cols, rows, now)?;
        Some(Observation::Action(instance, action))
    }

    fn report_trust_prompt(
        &self, session: &Session, agent: &AgentInfo, instance: Uuid, cli: TrustCli, action: GuardAction,
    ) -> Option<TrustPromptOutcome> {
        let name = match action {
            GuardAction::Detected(_) => "detected",
            GuardAction::Cleared(_) => "cleared",
        };
        // The known spawn folder, so nothing read off the screen reaches anyone, and only
        // while the observed instance is still the one running: a respawn in between
        // gets no notice about its predecessor's prompt.
        let path = {
            let manager = self.pty_manager.read();
            manager.with_current_instance(&agent.id, instance, |_| manager.spawn_cwd(&agent.id))?
        }
        .unwrap_or_else(|| "(unknown folder)".to_string());
        let outcome = TrustPromptOutcome { agent_id: agent.id.clone(), adapter: cli.name(), path, action: name };
        if name == "detected" {
            tracing::warn!(agent = %agent.id, adapter = cli.name(), path = %outcome.path, "folder-trust prompt needs the operator");
        } else {
            tracing::info!(agent = %agent.id, adapter = cli.name(), path = %outcome.path, "folder-trust prompt gone");
        }
        if self.set_trust_prompt_status(&session.id, &agent.id, instance, name == "cleared").is_some() {
            self.update_session_storage(&session.id);
            self.emit_session_update(&session.id);
        }
        self.publish_trust_prompt(session, agent, instance, &outcome);
        Some(outcome)
    }

    /// Set the trust-prompt status for the agent's current instance, remembering what it
    /// replaced, or (when `cleared`) put exactly that back. Only the status this sweep set
    /// is ever undone; anything newer wins. Returns the new status when the roster changed.
    fn set_trust_prompt_status(&self, session_id: &str, agent_id: &str, instance: Uuid, cleared: bool) -> Option<AgentStatus> {
        let waiting = AgentStatus::WaitingForInput(TRUST_PROMPT_STATUS.to_string());
        let key = (session_id.to_string(), agent_id.to_string());
        // Same lock order as `flag_dead_startup_seats`: PTY manager, then the roster.
        let manager = self.pty_manager.read();
        let mut sessions = self.sessions.write();
        let live = sessions.get_mut(session_id)?.agents.iter_mut().find(|live| live.id == agent_id)?;
        let mut prior = self.trust_prior_status.write();
        manager.with_current_instance(agent_id, instance, |_| {
            if matches!(live.status, AgentStatus::Completed | AgentStatus::Error(_)) {
                return None;
            }
            let next = if cleared {
                let before = prior.remove(&key).filter(|(seen, _)| *seen == instance).map(|(_, status)| status);
                if live.status != waiting {
                    return None;
                }
                // Exactly what it was: a gone dialog does not prove the CLI started (it may
                // have exited on "No"), so `Starting` stays `Starting`.
                before.unwrap_or(AgentStatus::Running)
            } else {
                if live.status == waiting {
                    return None;
                }
                prior.insert(key.clone(), (instance, live.status.clone()));
                waiting.clone()
            };
            live.status = next.clone();
            Some(next)
        })?
    }

    /// A same-id respawn must not keep the `trust_prompt` status its predecessor's prompt
    /// set: put back what that prompt replaced and forget the predecessor's record.
    fn drop_inherited_trust_status(&self, session_id: &str, agent_id: &str) {
        let key = (session_id.to_string(), agent_id.to_string());
        let Some((_, before)) = self.trust_prior_status.write().remove(&key) else {
            return;
        };
        let changed = {
            let mut sessions = self.sessions.write();
            sessions
                .get_mut(session_id)
                .and_then(|live| live.agents.iter_mut().find(|live| live.id == agent_id))
                .filter(|live| live.status == AgentStatus::WaitingForInput(TRUST_PROMPT_STATUS.to_string()))
                .map(|live| live.status = before)
                .is_some()
        };
        if changed {
            self.update_session_storage(session_id);
            self.emit_session_update(session_id);
        }
    }

    /// The `agent_waiting_input` event and a Queen inbox notice when a prompt comes up.
    /// A prompt going away is carried by the roster's status change alone: an
    /// `agent_waiting_input` event for it would read as a second "blocked" signal.
    fn publish_trust_prompt(&self, session: &Session, agent: &AgentInfo, instance: Uuid, outcome: &TrustPromptOutcome) {
        if outcome.action != "detected" {
            return;
        }
        let Some(emitter) = self.event_emitter.clone() else {
            return;
        };
        let session_id = session.id.clone();
        let cell_id = agent_cell_id(session, agent);
        let event = outcome.clone();
        let instance_id = instance.to_string();
        let notice = Some(format!(
            "{} ({}) is stopped on a folder-trust prompt in {}. Hive Manager does not answer these. \
             Answer it in the agent's terminal, or stop the agent.",
            agent.id, outcome.adapter, outcome.path,
        ));
        let inboxes: Vec<String> = session
            .agents
            .iter()
            .filter(|peer| matches!(peer.role, AgentRole::Queen) && peer.id != agent.id)
            .map(|peer| peer.id.clone())
            .collect();
        let storage = self.storage.clone();
        tokio::spawn(async move {
            let payload = crate::events::TrustPromptEvent {
                adapter: event.adapter, path: &event.path, action: event.action, instance_id: &instance_id,
            };
            if let Err(error) = emitter.emit_agent_trust_prompt(&session_id, &cell_id, &event.agent_id, &payload).await {
                tracing::warn!("Failed to emit trust prompt event: {}", error);
            }
            let (Some(notice), Some(storage)) = (notice, storage) else {
                return;
            };
            for inbox in inboxes {
                if let Err(error) = storage.append_conversation_message(&session_id, &inbox, "hive-manager", &notice).await {
                    tracing::warn!("Failed to tell {} about a trust prompt: {}", inbox, error);
                }
            }
        });
    }
}
