//! Folder-trust prompt sweep (#466).
//!
//! Spawns pre-trust their worktree where the CLI allows it (`cli::folder_trust`). This
//! sweep is the fallback: it renders each PTY agent's new output, answers a trust prompt
//! once when it names the agent's own worktree and its highlighted option is the trust
//! choice, and otherwise hands it to the Queen and the operator. While a prompt is up
//! the agent's status is `WaitingForInput("trust_prompt")`.

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
            if let Some(action) = self.observe_trust_prompt(session_id, &agent.id, cli, now) {
                outcomes.push(self.report_trust_prompt(&session, agent, cli, action));
            }
        }
        outcomes
    }

    fn observe_trust_prompt(&self, session_id: &str, agent_id: &str, cli: TrustCli, now: Instant) -> Option<GuardAction> {
        let manager = self.pty_manager.read();
        if manager.protocol_agent(agent_id).is_some() {
            return None;
        }
        let instance = manager.instance_id(agent_id)?;
        let mut guards = self.trust_guards.write();
        let key = (session_id.to_string(), agent_id.to_string());
        let fresh = guards.get(&key).is_none_or(|(seen, _)| *seen != instance);
        if fresh {
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
        let snapshot = manager.output_since(agent_id, guard.fed_to())?;
        let (cols, rows) = manager.pty_size(agent_id).unwrap_or(FALLBACK_SIZE);
        let action = guard.observe(&snapshot, cols, rows, now);
        if matches!(action, Some(GuardAction::Accept(_))) {
            let written = manager.write(agent_id, ACCEPT_KEYS);
            if let Err(error) = &written {
                tracing::warn!(agent = %agent_id, %error, "could not answer a folder-trust prompt");
            }
            if let Some(escalated) = guard.accept_written(written.is_ok()) {
                return Some(escalated);
            }
        }
        action
    }

    fn report_trust_prompt(&self, session: &Session, agent: &AgentInfo, cli: TrustCli, action: GuardAction) -> TrustPromptOutcome {
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
        match name {
            "escalated" => tracing::warn!(agent = %agent.id, adapter = cli.name(), path = %prompt.folder,
                reason = reason.unwrap_or(""), "folder-trust prompt needs the operator"),
            _ => tracing::info!(agent = %agent.id, adapter = cli.name(), path = %prompt.folder,
                action = name, "folder-trust prompt"),
        }

        let waiting = AgentStatus::WaitingForInput(TRUST_PROMPT_STATUS.to_string());
        let changed = {
            let mut sessions = self.sessions.write();
            sessions
                .get_mut(&session.id)
                .and_then(|live| live.agents.iter_mut().find(|live| live.id == agent.id))
                .is_some_and(|live| {
                    let next = match name {
                        // Only undo the status this sweep set; anything newer wins.
                        "cleared" if live.status == waiting => AgentStatus::Running,
                        "cleared" => return false,
                        _ => waiting.clone(),
                    };
                    if matches!(live.status, AgentStatus::Completed | AgentStatus::Error(_)) || live.status == next {
                        return false;
                    }
                    live.status = next;
                    true
                })
        };
        if changed {
            self.update_session_storage(&session.id);
            self.emit_session_update(&session.id);
        }

        let Some(emitter) = self.event_emitter.clone() else {
            return outcome;
        };
        let session_id = session.id.clone();
        let cell_id = agent_cell_id(session, agent);
        let event = outcome.clone();
        // The Queen reads her conversation inbox; the operator sees the status and event.
        let notice = (name == "escalated").then(|| format!(
            "{} ({}) is stopped on a folder-trust prompt for {}. Hive Manager did not answer it ({}). \
             Answer it in the agent's terminal, or stop the agent.",
            agent.id, cli.name(), prompt.folder, reason.unwrap_or("unknown"),
        ));
        let inboxes: Vec<String> = session
            .agents
            .iter()
            .filter(|peer| matches!(peer.role, AgentRole::Queen) && peer.id != agent.id)
            .map(|peer| peer.id.clone())
            .collect();
        let storage = self.storage.clone();
        tokio::spawn(async move {
            if let Err(error) = emitter
                .emit_agent_trust_prompt(&session_id, &cell_id, &event.agent_id, &crate::events::TrustPromptEvent {
                    adapter: event.adapter, path: &event.path, action: event.action, reason: event.reason,
                })
                .await
            {
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
        outcome
    }
}
