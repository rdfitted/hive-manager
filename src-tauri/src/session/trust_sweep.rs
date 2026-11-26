//! Folder-trust prompt sweep (#466).
//!
//! Spawns into a Hive-managed worktree pre-trust it where the CLI allows
//! (`cli::folder_trust`). This sweep is the fallback. It renders each PTY agent's new
//! output and answers a trust prompt once, only when the prompt names the agent's own
//! managed worktree and its highlighted option is the trust choice. Anything else is
//! handed to the Queen and the operator. While a prompt is up the agent's status is
//! `WaitingForInput("trust_prompt")`; it returns to `Running` when the prompt goes,
//! whoever answered it.
//!
//! Every read, write and status change is bound to the PTY instance the guard was made
//! for, so a same-ID respawn never receives an Enter meant for its predecessor.

use std::time::Instant;

use super::*;
use crate::pty::trust_prompt::{GuardAction, TrustCli, TrustPrompt, TrustPromptGuard, ACCEPT_KEYS, TRUST_PROMPT_STATUS};

/// Grid used when a PTY's size was never recorded (it is recorded at every spawn).
const FALLBACK_SIZE: (u16, u16) = (120, 40);

/// What the sweep did about one agent's trust prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustPromptOutcome {
    pub agent_id: String,
    pub adapter: &'static str,
    pub path: String,
    /// `accepted`, `cleared` or `escalated`.
    pub action: &'static str,
    pub reason: Option<&'static str>,
}

impl SessionController {
    /// Check every live PTY agent of `session_id` for a folder-trust prompt.
    /// Runs on the desktop's five-second monitoring sweep beside dead-seat detection.
    pub fn sweep_trust_prompts(&self, session_id: &str) -> Vec<TrustPromptOutcome> {
        self.sweep_trust_prompts_at(session_id, Instant::now())
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
            if let Some((instance, action)) = self.observe_trust_prompt(session_id, &agent.id, cli, now) {
                outcomes.push(self.report_trust_prompt(&session, agent, instance, cli, action));
            }
        }
        outcomes
    }

    fn observe_trust_prompt(&self, session_id: &str, agent_id: &str, cli: TrustCli, now: Instant) -> Option<(Uuid, GuardAction)> {
        let manager = self.pty_manager.read();
        if manager.protocol_agent(agent_id).is_some() {
            return None;
        }
        let instance = manager.instance_id(agent_id)?;
        let mut guards = self.trust_guards.write();
        let key = (session_id.to_string(), agent_id.to_string());
        if guards.get(&key).is_none_or(|(seen, _)| *seen != instance) {
            // Measure the watch window from the spawn, not from the first sweep.
            let age = manager
                .spawned_at(agent_id)
                .and_then(|at| (Utc::now() - at).to_std().ok())
                .unwrap_or_default();
            let started = now.checked_sub(age).unwrap_or(now);
            let (cols, rows) = manager.pty_size(agent_id).unwrap_or(FALLBACK_SIZE);
            let guard = TrustPromptGuard::new(cli, manager.spawn_cwd(agent_id), cols, rows, started);
            guards.insert(key.clone(), (instance, guard));
        }
        let guard = &mut guards.get_mut(&key)?.1;
        if !guard.is_active(now) {
            return None;
        }
        let snapshot = manager.output_since(agent_id, instance, guard.fed_to())?;
        let (cols, rows) = manager.pty_size(agent_id).unwrap_or(FALLBACK_SIZE);
        let mut action = guard.observe(&snapshot, cols, rows, now)?;
        if matches!(action, GuardAction::Accept(_)) {
            let written = manager.write_instance(agent_id, instance, ACCEPT_KEYS);
            if let Err(error) = &written {
                tracing::warn!(agent = %agent_id, %error, "could not answer a folder-trust prompt");
            }
            if let Some(escalated) = guard.accept_written(written.is_ok()) {
                action = escalated;
            }
        }
        Some((instance, action))
    }

    fn report_trust_prompt(
        &self, session: &Session, agent: &AgentInfo, instance: Uuid, cli: TrustCli, action: GuardAction,
    ) -> TrustPromptOutcome {
        let (prompt, name, reason): (TrustPrompt, &'static str, Option<&'static str>) = match action {
            GuardAction::Accept(prompt) => (prompt, "accepted", None),
            GuardAction::Cleared(prompt) => (prompt, "cleared", None),
            GuardAction::Refused(prompt, refusal) => (prompt, "escalated", Some(refusal.reason())),
            GuardAction::Escalated(prompt, escalation) => (prompt, "escalated", Some(escalation.reason())),
        };
        let outcome = TrustPromptOutcome {
            agent_id: agent.id.clone(),
            adapter: cli.name(),
            path: prompt.folder.clone(),
            action: name,
            reason,
        };
        if name == "escalated" {
            tracing::warn!(agent = %agent.id, adapter = cli.name(), path = %prompt.folder,
                reason = reason.unwrap_or(""), "folder-trust prompt needs the operator");
        } else {
            tracing::info!(agent = %agent.id, adapter = cli.name(), path = %prompt.folder, action = name, "folder-trust prompt");
        }
        if self.set_trust_prompt_status(&session.id, &agent.id, instance, name == "cleared").is_some() {
            self.update_session_storage(&session.id);
            self.emit_session_update(&session.id);
        }
        self.publish_trust_prompt(session, agent, &outcome);
        outcome
    }

    /// Set the trust-prompt status for the agent's current instance, remembering what it
    /// replaced, or (when `cleared`) put that back. Only the status this sweep set is ever
    /// undone; anything newer wins. Returns the new status when the roster changed.
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
                // A seat still `Starting` when the prompt came up has started by now.
                before.filter(|status| *status != AgentStatus::Starting).unwrap_or(AgentStatus::Running)
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

    /// The event for the operator, plus a Queen inbox notice when it needs a person.
    fn publish_trust_prompt(&self, session: &Session, agent: &AgentInfo, outcome: &TrustPromptOutcome) {
        let Some(emitter) = self.event_emitter.clone() else {
            return;
        };
        let session_id = session.id.clone();
        let cell_id = agent_cell_id(session, agent);
        let event = outcome.clone();
        let notice = (outcome.action == "escalated").then(|| format!(
            "{} ({}) is stopped on a folder-trust prompt for {}. Hive Manager did not answer it ({}). \
             Answer it in the agent's terminal, or stop the agent.",
            agent.id, outcome.adapter, outcome.path, outcome.reason.unwrap_or("unknown"),
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
                adapter: event.adapter, path: &event.path, action: event.action, reason: event.reason,
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
