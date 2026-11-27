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

/// The status an agent gets when its process exits while its trust prompt is up and
/// nothing else has recorded the exit.
const TRUST_PROMPT_EXIT: &str = "exited_at_trust_prompt";

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
    /// The PTY instance named was replaced since the last sweep (same-id respawn).
    Replaced(Uuid),
    /// The PTY instance named exited or was removed.
    Exited(Uuid),
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
            self.drop_stale_trust_status(&session.id, &agent.id);
            match self.observe_trust_prompt(&session.id, &agent.id, cli, now) {
                Some(Observation::Action(instance, action)) => {
                    outcomes.extend(self.report_trust_prompt(&session, agent, instance, cli, action));
                }
                Some(Observation::Replaced(ended)) => self.end_trust_prompt(&session, agent, cli, ended, false),
                Some(Observation::Exited(ended)) => self.end_trust_prompt(&session, agent, cli, ended, true),
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
        let key = (session_id.to_string(), agent_id.to_string());
        let Some(instance) = manager.instance_id(agent_id) else {
            // The PTY is gone: nobody can answer a prompt it showed.
            return self.trust_guards.write().remove(&key).map(|(seen, _)| Observation::Exited(seen));
        };
        let mut guards = self.trust_guards.write();
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
            if let Some(previous) = previous {
                return Some(Observation::Replaced(previous));
            }
        }
        let guard = &mut guards.get_mut(&key)?.1;
        if !manager.is_alive(agent_id) {
            // The process exited: release the screen and drop any status its prompt set.
            let surfaced = guard.is_active(now);
            guard.finish();
            return surfaced.then_some(Observation::Exited(instance));
        }
        if !guard.is_active(now) {
            guard.finish(); // release the screen; the instance marker stays
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
        let committed = self.set_trust_prompt_status(&session.id, &agent.id, instance, name == "cleared");
        if committed.is_some() {
            self.update_session_storage(&session.id);
            self.emit_session_update(&session.id);
        }
        // A detection is reported only once its status landed on the observed instance;
        // a clear is reported even when a newer status already replaced ours.
        if name == "detected" && committed.is_none() {
            return None;
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

    /// A `trust_prompt` status must not outlive the PTY whose prompt set it. `ended` is
    /// that PTY instance. Only its own record is consumed. After a same-id respawn (a
    /// different instance now runs), the status the prompt replaced is put back. After
    /// an exit with no replacement, the agent shows `exited_at_trust_prompt`, since
    /// nobody is waiting any more. Either way an `agent_input_resolved` event follows.
    fn end_trust_prompt(&self, session: &Session, agent: &AgentInfo, cli: TrustCli, ended: Uuid, exited: bool) {
        let key = (session.id.clone(), agent.id.clone());
        let prior = {
            let mut records = self.trust_prior_status.write();
            match records.get(&key) {
                Some((owner, _)) if *owner == ended => records.remove(&key).map(|(_, status)| status),
                _ => None,
            }
        };
        let Some(prior) = prior else {
            return;
        };
        let changed = {
            // Same lock order as `report_trust_prompt`: PTY instances, then the roster.
            // The instance stays held until the status is written, so a same-id respawn
            // can't land between the check and the write and inherit the exit.
            let manager = self.pty_manager.read();
            manager.with_instance_held(&agent.id, |current| {
                let replaced = current.is_some_and(|current| current != ended);
                let next = if exited && !replaced { AgentStatus::Error(TRUST_PROMPT_EXIT.to_string()) } else { prior };
                let mut sessions = self.sessions.write();
                sessions
                    .get_mut(&session.id)
                    .and_then(|live| live.agents.iter_mut().find(|live| live.id == agent.id))
                    .filter(|live| live.status == AgentStatus::WaitingForInput(TRUST_PROMPT_STATUS.to_string()))
                    .map(|live| live.status = next)
                    .is_some()
            })
        };
        if changed {
            self.update_session_storage(&session.id);
            self.emit_session_update(&session.id);
        }
        let path = self.pty_manager.read().spawn_cwd(&agent.id).unwrap_or_else(|| "(unknown folder)".to_string());
        let outcome = TrustPromptOutcome { agent_id: agent.id.clone(), adapter: cli.name(), path, action: "cleared" };
        self.publish_trust_prompt(session, agent, ended, &outcome);
    }

    /// A `trust_prompt` status with no prompt record behind it was persisted by an
    /// earlier run of the app (the records live in memory). Put the agent back to
    /// `Running`; if its prompt is still up, the new guard surfaces it again.
    fn drop_stale_trust_status(&self, session_id: &str, agent_id: &str) {
        let key = (session_id.to_string(), agent_id.to_string());
        if self.trust_prior_status.read().contains_key(&key) || self.trust_guards.read().contains_key(&key) {
            return;
        }
        let waiting = AgentStatus::WaitingForInput(TRUST_PROMPT_STATUS.to_string());
        let changed = {
            let mut sessions = self.sessions.write();
            sessions
                .get_mut(session_id)
                .and_then(|live| live.agents.iter_mut().find(|live| live.id == agent_id))
                .filter(|live| live.status == waiting)
                .map(|live| live.status = AgentStatus::Running)
                .is_some()
        };
        if changed {
            self.update_session_storage(session_id);
            self.emit_session_update(session_id);
        }
    }

    /// `agent_waiting_input` plus a Queen inbox notice when a prompt comes up, and
    /// `agent_input_resolved` when it goes (never a second "waiting" signal).
    fn publish_trust_prompt(&self, session: &Session, agent: &AgentInfo, instance: Uuid, outcome: &TrustPromptOutcome) {
        let Some(emitter) = self.event_emitter.clone() else {
            return;
        };
        let session_id = session.id.clone();
        let cell_id = agent_cell_id(session, agent);
        let event = outcome.clone();
        let instance_id = instance.to_string();
        let notice = (outcome.action == "detected").then(|| format!(
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
