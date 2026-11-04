//! Retirement releases a proven finished runtime; it never supplies completion evidence.

use std::{fs, path::Path, process::Command};

use serde::{Deserialize, Serialize};

use super::work_state::{work_state, WorkEvidence, WorkState};
use crate::pty::AgentRole;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetirementSettings {
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub on_call_agent_id: Option<String>,
}

fn enabled_by_default() -> bool { true }

impl Default for RetirementSettings {
    fn default() -> Self {
        Self { enabled: true, on_call_agent_id: None }
    }
}

/// Positive proof only. In particular, neither a quiet screen nor a finalized queue
/// row substitutes for acceptance or current-instance completion.
pub fn eligible(
    settings: &RetirementSettings,
    evidence: &WorkEvidence<'_>,
    accepted: bool,
    idle: bool,
    locks_clear: bool,
    owned_work_committed: bool,
    final_qa_verdict: bool,
) -> bool {
    settings.enabled
        && settings.on_call_agent_id.as_deref() != Some(evidence.agent_id)
        && matches!(evidence.role, AgentRole::Worker { .. } | AgentRole::QaWorker { .. })
        && evidence.alive
        && work_state(evidence) == WorkState::Finished
        && accepted
        && idle
        && locks_clear
        && match evidence.role {
            AgentRole::QaWorker { .. } => final_qa_verdict,
            _ => owned_work_committed,
        }
}

/// The Hive lock convention is locks/<name>/owner. A malformed or unreadable
/// lock is unknown, including a directory whose owner file has not been written yet.
pub fn holds_no_locks(root: &Path, agent_id: &str) -> bool {
    let Ok(entries) = fs::read_dir(root.join("locks")) else { return false };
    for entry in entries {
        let Ok(entry) = entry else { return false };
        let Ok(kind) = entry.file_type() else { return false };
        if kind.is_symlink() { return false; }
        if !kind.is_dir() || entry.file_name() == "want" { continue; }
        let Ok(owner) = fs::read_to_string(entry.path().join("owner")) else { return false };
        if owner.trim().is_empty() || owner.trim() == agent_id { return false; }
    }
    true
}

/// Restrict git to declared relative files. Literal pathspecs prevent a plan output
/// from turning into a git pathspec expression; git errors are never clean evidence.
pub fn owned_work_committed(cwd: &Path, outputs: &[String]) -> bool {
    let mut paths = Vec::new();
    for output in outputs {
        let path = output.strip_prefix("file:").unwrap_or(output);
        if !output.starts_with("file:") && path.contains(':') { continue; }
        if path.is_empty() || path.contains(':') || Path::new(path).is_absolute()
            || path.split(['/', '\\']).any(|part| part == "..") { return false; }
        if output.starts_with("file:") || path.contains('/') || path.contains('.') {
            paths.push(path);
        }
    }
    if paths.is_empty() { return false; }
    owned_work_command(cwd, &paths).output()
        .is_ok_and(|result| result.status.success() && result.stdout.is_empty())
}

fn owned_work_command(cwd: &Path, paths: &[&str]) -> Command {
    let mut command = Command::new("git");
    command.arg("--literal-pathspecs").arg("-C").arg(cwd)
        .args(["status", "--porcelain", "--untracked-files=all", "--"])
        .args(paths);
    crate::process::hide_std_console_window(&mut command);
    command
}

#[cfg(test)]
#[path = "git_console_tests.rs"]
mod git_console_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::pty::AgentStatus;

    #[test]
    fn g33_idle_alone_unaccepted_dirty_locked_and_protected_seats_survive() {
        let now = Utc::now();
        let worker = AgentRole::Worker { index: 1, parent: None };
        let mut evidence = WorkEvidence {
            agent_id: "worker", role: &worker, status: &AgentStatus::Running,
            alive: true, spawned_at: Some(now), heartbeat: Some(("completed", now)),
            heartbeat_is_self: false, queue: Some(&[]), verdict_at: None,
        };
        let settings = RetirementSettings::default();
        assert!(eligible(&settings, &evidence, true, true, true, true, false));
        for proofs in [(false,true,true,true), (true,false,true,true),
            (true,true,false,true), (true,true,true,false)] {
            assert!(!eligible(&settings, &evidence, proofs.0, proofs.1, proofs.2, proofs.3, false));
        }
        evidence.heartbeat = Some(("idle", now));
        assert!(!eligible(&settings, &evidence, true, true, true, true, false), "idle is not earned completion");
        evidence.heartbeat = Some(("completed", now));
        evidence.spawned_at = Some(now + chrono::Duration::seconds(1));
        assert!(!eligible(&settings, &evidence, true, true, true, true, false), "predecessor proof");
        evidence.spawned_at = Some(now);
        for role in [AgentRole::Queen, AgentRole::MasterPlanner, AgentRole::Evaluator,
            AgentRole::Prince, AgentRole::Planner { index: 1 }, AgentRole::ScratchShell,
            AgentRole::Fusion { variant: "a".into() }, AgentRole::Judge { session_id: "s".into() }] {
            let protected = WorkEvidence { role: &role, ..evidence };
            assert!(!eligible(&settings, &protected, true, true, true, true, true));
        }
        evidence.role = &worker;
        assert!(!eligible(&RetirementSettings { enabled: false, ..settings.clone() }, &evidence, true,true,true,true,true));
        assert!(!eligible(&RetirementSettings { on_call_agent_id: Some("worker".into()), ..settings }, &evidence, true,true,true,true,true));
        let dead = AgentStatus::Error("exited_during_startup:1".into());
        evidence.status = &dead;
        assert!(!eligible(&RetirementSettings::default(), &evidence, true,true,true,true,true));
    }

    #[test]
    fn g33_qa_requires_final_verdict_and_does_not_own_files() {
        let now = Utc::now();
        let evidence = WorkEvidence {
            agent_id: "qa", role: &AgentRole::QaWorker { index: 1, parent: None },
            status: &AgentStatus::Running, alive: true, spawned_at: Some(now),
            heartbeat: Some(("completed", now)), heartbeat_is_self: true, queue: Some(&[]), verdict_at: None,
        };
        assert!(!eligible(&RetirementSettings::default(), &evidence, true,true,true,false,false));
        assert!(eligible(&RetirementSettings::default(), &evidence, true,true,true,false,true));
    }

    #[test]
    fn g33_unreadable_and_owned_locks_are_not_clear() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!holds_no_locks(temp.path(), "worker"));
        fs::create_dir(temp.path().join("locks")).unwrap();
        assert!(holds_no_locks(temp.path(), "worker"));
        let gate = temp.path().join("locks/gate");
        fs::create_dir(&gate).unwrap();
        assert!(!holds_no_locks(temp.path(), "worker"));
        fs::write(gate.join("owner"), "worker\n").unwrap();
        assert!(!holds_no_locks(temp.path(), "worker"));
        fs::write(gate.join("owner"), "other").unwrap();
        assert!(holds_no_locks(temp.path(), "worker"));
    }

    #[test]
    fn g33_git_checks_only_owned_paths_and_never_changes_commits() {
        let temp = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| Command::new("git").arg("-C").arg(temp.path()).args(args).output().unwrap();
        assert!(git(&["init", "--quiet"]).status.success());
        fs::write(temp.path().join("owned.txt"), "one").unwrap();
        assert!(!owned_work_committed(temp.path(), &["file:owned.txt".into()]));
        assert!(git(&["add", "owned.txt"]).status.success());
        assert!(git(&["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-qm", "fixture"]).status.success());
        let head = git(&["rev-parse", "HEAD"]).stdout;
        fs::write(temp.path().join("other.txt"), "other lane").unwrap();
        assert!(owned_work_committed(temp.path(), &["file:owned.txt".into()]));
        fs::write(temp.path().join("owned.txt"), "two").unwrap();
        assert!(!owned_work_committed(temp.path(), &["file:owned.txt".into()]));
        assert!(!owned_work_committed(temp.path(), &[]));
        assert!(!owned_work_committed(temp.path(), &["file:../outside".into()]));
        assert!(!owned_work_committed(temp.path(), &["file:owned.txt".into(), "file:../outside".into()]));
        assert!(!owned_work_committed(&temp.path().join("missing"), &["owned.txt".into()]));
        assert_eq!(git(&["rev-parse", "HEAD"]).stdout, head);
    }
}
