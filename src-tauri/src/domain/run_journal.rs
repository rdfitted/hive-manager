//! Domain types for the resumable-run journal + side-effect ledger (#125).
//!
//! A *run* is identified by a session id (one active run per session). Each *step*
//! is an orchestrator-level action — spawning a worker/evaluator or performing a git
//! commit/branch op. Steps that mutate the repo/worktree are *write-steps*: those are
//! journaled `Started` BEFORE execution and `Completed`/`Failed` after, so a resume can
//! tell whether a destructive op already happened and must NOT be re-run.
//!
//! The *ledger* records confirmable side-effects (a commit SHA, a created branch). The
//! ledger row is written BEFORE the destructive op and `confirmed` AFTER. An unconfirmed
//! ledger row paired with a non-`Completed` step is the recovery signal: on resume we
//! verify the effect actually landed (`git cat-file -e <sha>` / `git rev-parse --verify
//! <branch>`) and set a [`Confidence`] accordingly.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The kind of orchestrator step being journaled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// Spawning a hive worker (creates a worktree/branch — destructive).
    WorkerSpawn,
    /// Spawning an evaluator.
    EvaluatorSpawn,
    /// A git commit captured at worker completion.
    GitCommit,
    /// A git branch/worktree creation.
    GitBranch,
    /// A direct file write side-effect.
    FileWrite,
    /// A task injection into a running agent.
    TaskInjection,
    /// Anything else / not yet classified.
    Other,
}

impl StepKind {
    /// Write-steps mutate the repo/worktree and must not be re-executed on resume
    /// once they are journaled `Completed`.
    pub fn is_write_step(&self) -> bool {
        matches!(
            self,
            StepKind::WorkerSpawn
                | StepKind::EvaluatorSpawn
                | StepKind::GitCommit
                | StepKind::GitBranch
                | StepKind::FileWrite
        )
    }

    /// Stable string tag used in the deterministic step-id and SQL `kind` column.
    pub fn as_tag(&self) -> &'static str {
        match self {
            StepKind::WorkerSpawn => "worker_spawn",
            StepKind::EvaluatorSpawn => "evaluator_spawn",
            StepKind::GitCommit => "git_commit",
            StepKind::GitBranch => "git_branch",
            StepKind::FileWrite => "file_write",
            StepKind::TaskInjection => "task_injection",
            StepKind::Other => "other",
        }
    }

    /// Parse a `kind` tag from the SQL column. Unknown tags map to [`StepKind::Other`].
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "worker_spawn" => StepKind::WorkerSpawn,
            "evaluator_spawn" => StepKind::EvaluatorSpawn,
            "git_commit" => StepKind::GitCommit,
            "git_branch" => StepKind::GitBranch,
            "file_write" => StepKind::FileWrite,
            "task_injection" => StepKind::TaskInjection,
            _ => StepKind::Other,
        }
    }
}

/// The lifecycle status of a journaled step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// Recorded immediately before the step executes.
    Started,
    /// The step finished successfully.
    Completed,
    /// The step ran and failed.
    Failed,
    /// Derived on resume: the step was `Started` but never finished (app was killed mid-step).
    Interrupted,
    /// Derived on resume: `Started` with an unconfirmed ledger effect (recovery candidate).
    Unknown,
    /// Derived on resume: a completed write-step intentionally not re-executed.
    Skipped,
}

impl StepStatus {
    /// Stable string tag for the SQL `status` column.
    pub fn as_tag(&self) -> &'static str {
        match self {
            StepStatus::Started => "started",
            StepStatus::Completed => "completed",
            StepStatus::Failed => "failed",
            StepStatus::Interrupted => "interrupted",
            StepStatus::Unknown => "unknown",
            StepStatus::Skipped => "skipped",
        }
    }

    /// Parse a `status` tag from the SQL column. Unknown tags map to [`StepStatus::Unknown`].
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "started" => StepStatus::Started,
            "completed" => StepStatus::Completed,
            "failed" => StepStatus::Failed,
            "interrupted" => StepStatus::Interrupted,
            "skipped" => StepStatus::Skipped,
            _ => StepStatus::Unknown,
        }
    }
}

/// Confidence that a recovered side-effect actually landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// The effect was verified to exist (e.g. the commit SHA is reachable).
    High,
    /// Probable but not verified.
    Likely,
    /// Could not be verified — the human/agent should decide.
    Uncertain,
}

/// A single journal row: one logical step and its current status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunJournalEntry {
    pub run_id: String,
    pub step_id: String,
    pub kind: StepKind,
    pub status: StepStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub detail: Option<String>,
}

impl RunJournalEntry {
    /// Deterministic step id: same `(run_id, kind, ordinal)` always maps to the same
    /// row across resumes. Mirrors the `stable_learning_id` UUID v5 pattern in storage.
    pub fn deterministic_step_id(run_id: &str, kind: StepKind, ordinal: u64) -> String {
        let content = format!("{}:{}:{}", run_id, kind.as_tag(), ordinal);
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, content.as_bytes()).to_string()
    }
}

/// A confirmable side-effect (commit SHA, branch name) recorded around a write-step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub run_id: String,
    pub step_id: String,
    pub effect_kind: String,
    /// e.g. a commit SHA or a branch name. Never an absolute path (relocation-safe).
    pub effect_ref: Option<String>,
    pub confirmed: bool,
    pub confidence: Confidence,
    pub recorded_at: DateTime<Utc>,
}

/// Summary attached to a resumed [`crate::session::Session`] describing what the
/// resume classifier found, so the frontend can render a confirmation modal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeReport {
    /// Completed write-steps that were marked Skipped (will not be re-run).
    pub skipped: Vec<RunJournalEntry>,
    /// Steps that were Started but never finished (app killed mid-step).
    pub interrupted: Vec<RunJournalEntry>,
    /// Ledger effects that could not be confirmed and need human attention.
    pub uncertain: Vec<LedgerEntry>,
}

impl ResumeReport {
    /// True when there is nothing to surface (a clean resume).
    pub fn is_empty(&self) -> bool {
        self.skipped.is_empty() && self.interrupted.is_empty() && self.uncertain.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_write_step() {
        assert!(StepKind::WorkerSpawn.is_write_step());
        assert!(StepKind::EvaluatorSpawn.is_write_step());
        assert!(StepKind::GitCommit.is_write_step());
        assert!(StepKind::GitBranch.is_write_step());
        assert!(StepKind::FileWrite.is_write_step());
        assert!(!StepKind::TaskInjection.is_write_step());
        assert!(!StepKind::Other.is_write_step());
    }

    #[test]
    fn test_kind_tag_roundtrip() {
        for kind in [
            StepKind::WorkerSpawn,
            StepKind::EvaluatorSpawn,
            StepKind::GitCommit,
            StepKind::GitBranch,
            StepKind::FileWrite,
            StepKind::TaskInjection,
            StepKind::Other,
        ] {
            assert_eq!(StepKind::from_tag(kind.as_tag()), kind);
        }
    }

    #[test]
    fn test_status_tag_roundtrip() {
        for status in [
            StepStatus::Started,
            StepStatus::Completed,
            StepStatus::Failed,
            StepStatus::Interrupted,
            StepStatus::Unknown,
            StepStatus::Skipped,
        ] {
            assert_eq!(StepStatus::from_tag(status.as_tag()), status);
        }
    }

    #[test]
    fn test_deterministic_step_id_is_stable() {
        let a = RunJournalEntry::deterministic_step_id("run-1", StepKind::GitCommit, 2);
        let b = RunJournalEntry::deterministic_step_id("run-1", StepKind::GitCommit, 2);
        assert_eq!(a, b, "same inputs must produce the same id");

        let c = RunJournalEntry::deterministic_step_id("run-1", StepKind::GitCommit, 3);
        assert_ne!(a, c, "different ordinal must produce a different id");
        let d = RunJournalEntry::deterministic_step_id("run-2", StepKind::GitCommit, 2);
        assert_ne!(a, d, "different run must produce a different id");
        assert_eq!(a.len(), 36, "UUID format");
    }
}
