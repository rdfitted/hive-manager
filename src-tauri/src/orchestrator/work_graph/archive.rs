//! Immutable work-graph instance archives for issue #214, owned by WS-5.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::coordination::StateManager;
use crate::domain::event::Event;
use crate::domain::run_journal::{LedgerEntry, RunJournalEntry};
use crate::storage::{LearningSubmission, RunJournalStore, SessionStorage};

use super::completion_ledger::read_node_completion_facts;
use super::divergence::{compute_divergence, DivergenceSummary};
use super::retro::{
    evaluate_archive_paths, evaluate_completed_session, IndependentEvaluator, RetroArchivePath,
    RetroOmission, RetroOmissionReason, RetroReport, UNREVIEWED_OUTCOME,
};
use super::runtime::{
    derive_runtime_graph_with_completion_facts, mutation_log_snapshot,
    reconstruct_structural_history, record_omission, GraphMutationDelta, RuntimeOutcome,
};
use super::schema::{TaskGraph, WorkGraph, WorkGraphOmission, WorkGraphOmissionReason};

pub const WORK_GRAPH_ARCHIVE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveSourceKind {
    PlanGraph,
    EventLog,
    RunJournal,
    RunLedger,
    MutationLog,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveSourceReport {
    pub kind: ArchiveSourceKind,
    pub location: String,
    pub available: bool,
    pub record_count: usize,
    #[serde(default)]
    pub omissions: Vec<WorkGraphOmission>,
}

/// Published archive interface consumed by the post-run retro (#217).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkGraphArchive {
    pub schema_version: u32,
    pub archive_id: String,
    pub session_id: String,
    pub archived_at: DateTime<Utc>,
    pub plan_graph: Option<TaskGraph>,
    pub runtime_graph: WorkGraph,
    pub deltas: Vec<GraphMutationDelta>,
    pub outcomes: Vec<RuntimeOutcome>,
    pub divergence: DivergenceSummary,
    pub sources: Vec<ArchiveSourceReport>,
}

impl WorkGraphArchive {
    pub fn reconstruct_structural_history(&self) -> Result<Vec<WorkGraph>, String> {
        reconstruct_structural_history(&self.runtime_graph, &self.deltas)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveCompletion {
    pub path: PathBuf,
    pub archive: WorkGraphArchive,
    pub created: bool,
}

/// Independence provenance captured while planner/supervisor identities are
/// still live. Completion reads this record rather than guessing departed roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetroEvaluatorProvenance {
    pub repo_id: String,
    pub evaluator_id: String,
    #[serde(default)]
    pub planner_agent_ids: Vec<String>,
    #[serde(default)]
    pub supervisor_agent_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveRetroCompletion {
    pub archive: Option<ArchiveCompletion>,
    pub report_path: PathBuf,
    pub report: RetroReport,
    pub submitted_learning_ids: Vec<String>,
}

#[derive(Debug)]
pub enum ArchiveError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Storage(String),
    InvalidArchive(String),
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "archive I/O error: {error}"),
            Self::Json(error) => write!(formatter, "archive JSON error: {error}"),
            Self::Storage(error) => write!(formatter, "runtime source error: {error}"),
            Self::InvalidArchive(error) => write!(formatter, "invalid work-graph archive: {error}"),
        }
    }
}

impl Error for ArchiveError {}

impl From<std::io::Error> for ArchiveError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ArchiveError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

fn validate_session_id(session_id: &str) -> Result<(), ArchiveError> {
    if session_id.is_empty()
        || session_id.contains("..")
        || session_id.contains('/')
        || session_id.contains('\\')
    {
        return Err(ArchiveError::InvalidArchive(
            "session id cannot be empty or contain path separators".to_string(),
        ));
    }
    Ok(())
}

fn archive_id_for_session(session_id: &str) -> String {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("hive-manager:work-graph:{session_id}").as_bytes(),
    )
    .to_string()
}

fn retro_provenance_path(session_dir: &Path) -> PathBuf {
    session_dir
        .join("state")
        .join("retro-evaluator-provenance.json")
}

fn retro_report_path(session_dir: &Path, archive_id: &str) -> PathBuf {
    session_dir
        .join("archive")
        .join("work-graph-retros")
        .join(format!("{archive_id}.json"))
}

fn write_atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), ArchiveError> {
    let parent = path.parent().ok_or_else(|| {
        ArchiveError::InvalidArchive(format!("no parent directory for {}", path.display()))
    })?;
    fs::create_dir_all(parent)?;
    let mut temp = NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, value)?;
    temp.persist(path)
        .map_err(|error| ArchiveError::Io(error.error))?;
    Ok(())
}

fn evaluate_archive_corpus(
    evaluator: &IndependentEvaluator,
    storage: &SessionStorage,
    completed_session_dir: &Path,
    completed_provenance: &RetroEvaluatorProvenance,
) -> Result<RetroReport, super::retro::RetroError> {
    let mut paths = Vec::new();
    let mut provenance_omissions = Vec::new();
    if let Ok(entries) = fs::read_dir(storage.sessions_dir()) {
        let mut session_dirs = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        session_dirs.sort();
        for session_dir in session_dirs {
            let provenance_path = retro_provenance_path(&session_dir);
            let provenance = fs::read_to_string(&provenance_path)
                .ok()
                .and_then(|json| serde_json::from_str::<RetroEvaluatorProvenance>(&json).ok());
            let Some(provenance) = provenance else {
                continue;
            };
            if provenance.evaluator_id != completed_provenance.evaluator_id {
                provenance_omissions.push(RetroOmission::new(
                    RetroOmissionReason::EvaluatorProvenanceUnavailable,
                    "retro_corpus",
                    "archive used a different evaluator identity and was excluded from this corpus",
                    vec![provenance_path.display().to_string()],
                ));
                continue;
            }
            if IndependentEvaluator::new(
                provenance.evaluator_id.clone(),
                provenance.planner_agent_ids.clone(),
                provenance.supervisor_agent_ids.clone(),
            )
            .is_err()
            {
                provenance_omissions.push(RetroOmission::new(
                    RetroOmissionReason::EvaluatorProvenanceUnavailable,
                    "retro_corpus",
                    "archive provenance does not prove evaluator independence and was excluded",
                    vec![provenance_path.display().to_string()],
                ));
                continue;
            }
            if let Ok(archive_paths) = list_archives(&session_dir) {
                paths.extend(archive_paths.into_iter().map(|path| RetroArchivePath {
                    repo_id: provenance.repo_id.clone(),
                    path,
                }));
            }
        }
    }
    if paths.len() <= 1 {
        return evaluate_completed_session(
            evaluator,
            &completed_provenance.repo_id,
            completed_session_dir,
        );
    }
    let mut report = evaluate_archive_paths(evaluator, &paths)?;
    report.omissions.extend(provenance_omissions);
    Ok(report)
}

/// Persist the identities needed to prove that the post-run evaluator neither
/// planned nor supervised this session. The lifecycle owner calls this before
/// the planner launches, while the complete identity set is still available.
pub fn persist_retro_evaluator_provenance(
    storage_base_dir: &Path,
    session_id: &str,
    provenance: &RetroEvaluatorProvenance,
) -> Result<PathBuf, ArchiveError> {
    validate_session_id(session_id)?;
    if provenance.repo_id.trim().is_empty() {
        return Err(ArchiveError::InvalidArchive(
            "retro provenance repo id cannot be empty".to_string(),
        ));
    }
    IndependentEvaluator::new(
        provenance.evaluator_id.clone(),
        provenance.planner_agent_ids.clone(),
        provenance.supervisor_agent_ids.clone(),
    )
    .map_err(|error| ArchiveError::InvalidArchive(error.to_string()))?;
    let storage = SessionStorage::new_with_base(storage_base_dir.to_path_buf())
        .map_err(|error| ArchiveError::Storage(error.to_string()))?;
    let path = retro_provenance_path(&storage.session_dir(session_id));
    write_atomic_json(&path, provenance)?;
    Ok(path)
}

/// Complete the terminal archive synchronously. Callers that run on the session
/// completion path should use `schedule_completed_session_archive_and_retro`,
/// which moves this work off that path and logs failures without changing the
/// session outcome.
pub fn archive_completed_session(
    storage_base_dir: &Path,
    run_journal: Option<&RunJournalStore>,
    session_id: &str,
) -> Result<ArchiveCompletion, ArchiveError> {
    validate_session_id(session_id)?;
    let storage = SessionStorage::new_with_base(storage_base_dir.to_path_buf())
        .map_err(|error| ArchiveError::Storage(error.to_string()))?;
    let session_dir = storage.session_dir(session_id);
    let archive_dir = session_dir.join("archive").join("work-graphs");
    fs::create_dir_all(&archive_dir)?;

    let archive_id = archive_id_for_session(session_id);
    let target = archive_dir.join(format!("{archive_id}.json"));
    if target.exists() {
        let archive = read_archive(&target)?;
        StateManager::new(session_dir)
            .write_portable_work_graph_artifact(
                "archived",
                &archive.runtime_graph,
                Some(&archive.divergence),
            )
            .map_err(|error| ArchiveError::Storage(error.to_string()))?;
        return Ok(ArchiveCompletion {
            archive,
            path: target,
            created: false,
        });
    }

    let state = StateManager::new(session_dir.clone());
    let (plan_graph, plan_report) = match state.read_work_graph() {
        Ok(Some(graph)) => (
            Some(graph),
            source_report(
                ArchiveSourceKind::PlanGraph,
                "state/work-graph.json",
                true,
                1,
            ),
        ),
        Ok(None) => {
            let mut report = source_report(
                ArchiveSourceKind::PlanGraph,
                "state/work-graph.json",
                false,
                0,
            );
            source_omission(
                &mut report,
                WorkGraphOmissionReason::SourceUnreadable,
                "state/work-graph.json",
            );
            (None, report)
        }
        Err(error) => {
            let mut report = source_report(
                ArchiveSourceKind::PlanGraph,
                "state/work-graph.json",
                false,
                0,
            );
            source_omission(
                &mut report,
                WorkGraphOmissionReason::SourceUnreadable,
                "state/work-graph.json:invalid",
            );
            report.omissions[0].detail = format!(
                "work graph could not be read; runtime structure may be incomplete: {error}"
            );
            (None, report)
        }
    };

    let event_log_path = storage_base_dir.join(session_id).join("events.jsonl");
    let (events, event_report) = read_event_log(&event_log_path, session_id);
    let (completion_facts, completion_ledger_omission) =
        match read_node_completion_facts(&session_dir) {
            Ok(facts) => (facts, None),
            Err(error) => {
                let mut omission = WorkGraphOmission::new(
                    WorkGraphOmissionReason::SourceUnreadable,
                    1,
                    vec!["state/work-graph-completions.jsonl".to_string()],
                );
                omission.detail = format!(
                    "declared node completions could not be read and were omitted: {error}"
                );
                (Vec::new(), Some(omission))
            }
        };
    let (journal, journal_report, ledger, ledger_report) =
        read_run_sources(run_journal, session_id);
    let mutation_snapshot = mutation_log_snapshot(session_id);
    let deltas = mutation_snapshot.deltas;
    let mut mutation_report = source_report(
        ArchiveSourceKind::MutationLog,
        "memory/session-mutation-log",
        mutation_snapshot.tracked,
        deltas.len(),
    );
    if !mutation_snapshot.tracked {
        source_omission(
            &mut mutation_report,
            WorkGraphOmissionReason::ResolutionIncomplete,
            "mutation-log:not-observed-in-this-process",
        );
        mutation_report.omissions[0].detail =
            "the process did not observe a mutation boundary for this session; zero deltas cannot prove that no earlier structural mutations occurred"
                .to_string();
    } else if let (Some(plan), Some(first)) = (plan_graph.as_ref(), deltas.first()) {
        if plan.nodes != first.before.nodes || plan.edges != first.before.edges {
            source_omission(
                &mut mutation_report,
                WorkGraphOmissionReason::ResolutionIncomplete,
                "mutation-log:first-before-does-not-match-plan",
            );
            mutation_report
                .omissions
                .last_mut()
                .expect("the mismatch omission was just appended")
                .detail = "the first observed mutation starts after the persisted plan state; an earlier structural change may be absent"
                .to_string();
        }
    }

    // Source reporting owns the legacy-plan omission in an archive. Passing an
    // explicit empty derivation base avoids counting the same absent file twice.
    let derivation_base = plan_graph.clone().unwrap_or_default();
    let (agent_principals, hierarchy_omission): (BTreeMap<String, String>, _) =
        match state.read_hierarchy() {
            Ok(hierarchy) => (
                hierarchy
                    .into_iter()
                    .filter_map(|node| node.principal.map(|principal| (node.id, principal)))
                    .collect(),
                None,
            ),
            Err(error) => {
                let mut omission = WorkGraphOmission::new(
                    WorkGraphOmissionReason::SourceUnreadable,
                    1,
                    vec!["state/hierarchy.json".to_string()],
                );
                omission.detail = format!(
                    "agent hierarchy could not be read; lane attribution may be incomplete: {error}"
                );
                (BTreeMap::new(), Some(omission))
            }
        };
    let mut derivation = derive_runtime_graph_with_completion_facts(
        Some(&derivation_base),
        &events,
        &journal,
        &ledger,
        &deltas,
        &completion_facts,
        &agent_principals,
    );
    let sources = vec![
        plan_report.clone(),
        event_report,
        journal_report,
        ledger_report,
        mutation_report,
    ];
    for report in &sources {
        for omission in &report.omissions {
            merge_omission(&mut derivation.runtime_graph, omission);
        }
    }
    if let Some(omission) = completion_ledger_omission.as_ref() {
        merge_omission(&mut derivation.runtime_graph, omission);
    }
    if let Some(omission) = hierarchy_omission.as_ref() {
        merge_omission(&mut derivation.runtime_graph, omission);
    }
    let divergence = compute_divergence(plan_graph.as_ref(), &derivation.runtime_graph, &deltas);
    let archive = WorkGraphArchive {
        schema_version: WORK_GRAPH_ARCHIVE_SCHEMA_VERSION,
        archive_id,
        session_id: session_id.to_string(),
        archived_at: Utc::now(),
        plan_graph,
        runtime_graph: derivation.runtime_graph,
        deltas,
        outcomes: derivation.outcomes,
        divergence,
        sources,
    };
    archive
        .reconstruct_structural_history()
        .map_err(ArchiveError::InvalidArchive)?;

    let mut temp = NamedTempFile::new_in(&archive_dir)?;
    serde_json::to_writer_pretty(&mut temp, &archive)?;
    let completion = match temp.persist_noclobber(&target) {
        Ok(_) => ArchiveCompletion {
            path: target,
            archive,
            created: true,
        },
        Err(_error) if target.exists() => ArchiveCompletion {
            archive: read_archive(&target)?,
            path: target,
            created: false,
        },
        Err(error) => return Err(ArchiveError::Io(error.error)),
    };
    StateManager::new(session_dir)
        .write_portable_work_graph_artifact(
            "archived",
            &completion.archive.runtime_graph,
            Some(&completion.archive.divergence),
        )
        .map_err(|error| ArchiveError::Storage(error.to_string()))?;
    Ok(completion)
}

/// Synchronously run the ordered terminal pipeline used by the detached hook:
/// archive -> retro evaluation -> idempotent learning submission -> atomic report.
/// Every archive/evaluator/submission failure is represented in the report when
/// the report path remains writable.
pub fn complete_session_archive_and_retro(
    storage_base_dir: &Path,
    run_journal: Option<&RunJournalStore>,
    session_id: &str,
) -> Result<ArchiveRetroCompletion, ArchiveError> {
    validate_session_id(session_id)?;
    let storage = SessionStorage::new_with_base(storage_base_dir.to_path_buf())
        .map_err(|error| ArchiveError::Storage(error.to_string()))?;
    let session_dir = storage.session_dir(session_id);
    let archive_result = archive_completed_session(storage_base_dir, run_journal, session_id);
    let archive_id = archive_result
        .as_ref()
        .map(|completion| completion.archive.archive_id.clone())
        .unwrap_or_else(|_| archive_id_for_session(session_id));

    let provenance_path = retro_provenance_path(&session_dir);
    let provenance = fs::read_to_string(&provenance_path)
        .map_err(ArchiveError::Io)
        .and_then(|json| {
            serde_json::from_str::<RetroEvaluatorProvenance>(&json).map_err(ArchiveError::Json)
        });

    let mut report = match (&archive_result, provenance) {
        (Ok(_), Ok(provenance)) => match IndependentEvaluator::new(
            provenance.evaluator_id.clone(),
            provenance.planner_agent_ids.clone(),
            provenance.supervisor_agent_ids.clone(),
        ) {
            Ok(evaluator) => {
                evaluate_archive_corpus(&evaluator, &storage, &session_dir, &provenance)
                    .unwrap_or_else(|error| {
                        tracing::warn!(
                            session_id,
                            "Post-run work-graph evaluation failed: {error}"
                        );
                        RetroReport::unavailable(
                            provenance.evaluator_id,
                            RetroOmission::new(
                                RetroOmissionReason::ArchiveUnreadable,
                                "retro_completion",
                                format!("post-run evaluation failed: {error}"),
                                vec![session_id.to_string()],
                            )
                            .for_archive(&archive_id),
                        )
                    })
            }
            Err(error) => {
                tracing::warn!(
                    session_id,
                    "Retro evaluator provenance is not independent: {error}"
                );
                RetroReport::unavailable(
                    provenance.evaluator_id,
                    RetroOmission::new(
                        RetroOmissionReason::EvaluatorProvenanceUnavailable,
                        "retro_completion",
                        error.to_string(),
                        vec![provenance_path.display().to_string()],
                    )
                    .for_archive(&archive_id),
                )
            }
        },
        (Ok(_), Err(error)) => {
            tracing::warn!(
                session_id,
                "Retro evaluator provenance could not be read: {error}"
            );
            RetroReport::unavailable(
                "unavailable",
                RetroOmission::new(
                    RetroOmissionReason::EvaluatorProvenanceUnavailable,
                    "retro_completion",
                    format!("retro evaluator provenance could not be read: {error}"),
                    vec![provenance_path.display().to_string()],
                )
                .for_archive(&archive_id),
            )
        }
        (Err(error), _) => {
            tracing::warn!(
                session_id,
                "Work-graph archival failed before retro evaluation: {error}"
            );
            RetroReport::unavailable(
                "unavailable",
                RetroOmission::new(
                    RetroOmissionReason::ArchiveUnreadable,
                    "retro_completion",
                    format!("work-graph archival failed before retro evaluation: {error}"),
                    vec![session_id.to_string()],
                )
                .for_archive(&archive_id),
            )
        }
    };

    let mut submitted_learning_ids = Vec::new();
    for submission in report.learning_submissions.clone() {
        let serialized = serde_json::to_string(&submission)?;
        let learning_id = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("hive-manager:work-graph-retro-learning:{serialized}").as_bytes(),
        )
        .to_string();
        if submission.outcome != UNREVIEWED_OUTCOME {
            tracing::warn!(
                session_id,
                outcome = %submission.outcome,
                "Retro learning submission was not unreviewed"
            );
            report.omissions.push(
                RetroOmission::new(
                    RetroOmissionReason::LearningSubmissionFailed,
                    "learning_submission",
                    "retro learning outcome was not unreviewed and was not submitted",
                    vec![learning_id],
                )
                .for_archive(&archive_id),
            );
            continue;
        }
        let request = LearningSubmission {
            session: submission.session,
            task: submission.task,
            outcome: submission.outcome,
            keywords: submission.keywords,
            insight: submission.insight,
            files_touched: submission.files_touched,
        };
        match storage.submit_learning_session_with_id(&request.session, &request, &learning_id) {
            Ok(result) => submitted_learning_ids.push(result.learning_id),
            Err(error) => {
                tracing::warn!(session_id, "Retro learning submission failed: {error}");
                report.omissions.push(
                    RetroOmission::new(
                        RetroOmissionReason::LearningSubmissionFailed,
                        "learning_submission",
                        format!("unreviewed retro learning was not submitted: {error}"),
                        vec![learning_id],
                    )
                    .for_archive(&archive_id),
                );
            }
        }
    }

    let report_path = retro_report_path(&session_dir, &archive_id);
    write_atomic_json(&report_path, &report)?;
    Ok(ArchiveRetroCompletion {
        archive: archive_result.ok(),
        report_path,
        report,
        submitted_learning_ids,
    })
}

/// One-line, non-blocking terminal hook for SessionController. The archive and
/// retro run sequentially inside this thread, so evaluation cannot race archive
/// persistence. Failure is logged and can never change the completed session.
pub fn schedule_completed_session_archive_and_retro(
    storage_base_dir: PathBuf,
    run_journal: Option<RunJournalStore>,
    session_id: String,
) {
    let thread_name = format!("work-graph-archive-retro-{session_id}");
    let spawn = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            if let Err(error) = complete_session_archive_and_retro(
                &storage_base_dir,
                run_journal.as_ref(),
                &session_id,
            ) {
                tracing::warn!(
                    session_id,
                    "Failed to archive completed work graph: {error}"
                );
            }
        });
    if let Err(error) = spawn {
        tracing::warn!("Failed to schedule completed work-graph archive: {error}");
    }
}

/// Backward-compatible entry point. Production callers should use the composed
/// name above; retaining this wrapper keeps older integrations race-free.
pub fn schedule_completed_session_archive(
    storage_base_dir: PathBuf,
    run_journal: Option<RunJournalStore>,
    session_id: String,
) {
    schedule_completed_session_archive_and_retro(storage_base_dir, run_journal, session_id);
}

pub fn read_archive(path: &Path) -> Result<WorkGraphArchive, ArchiveError> {
    let archive: WorkGraphArchive = serde_json::from_str(&fs::read_to_string(path)?)?;
    if archive.schema_version != WORK_GRAPH_ARCHIVE_SCHEMA_VERSION {
        return Err(ArchiveError::InvalidArchive(format!(
            "unsupported schema version {}",
            archive.schema_version
        )));
    }
    archive
        .reconstruct_structural_history()
        .map_err(ArchiveError::InvalidArchive)?;
    Ok(archive)
}

pub fn list_archives(session_dir: &Path) -> Result<Vec<PathBuf>, ArchiveError> {
    let archive_dir = session_dir.join("archive").join("work-graphs");
    if !archive_dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(archive_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn read_event_log(path: &Path, session_id: &str) -> (Vec<Event>, ArchiveSourceReport) {
    let mut report = source_report(ArchiveSourceKind::EventLog, "events.jsonl", false, 0);
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => {
            source_omission(
                &mut report,
                WorkGraphOmissionReason::SourceUnreadable,
                "events.jsonl",
            );
            return (Vec::new(), report);
        }
    };
    report.available = true;

    let mut events = Vec::new();
    for (index, line) in std::io::BufReader::new(file).lines().enumerate() {
        let line_number = index + 1;
        match line {
            Ok(line) if line.trim().is_empty() => {}
            Ok(line) => match serde_json::from_str::<Event>(&line) {
                Ok(event) if event.session_id == session_id => events.push(event),
                Ok(_) => source_omission(
                    &mut report,
                    WorkGraphOmissionReason::ResolutionIncomplete,
                    &format!("events.jsonl:{line_number}:session-mismatch"),
                ),
                Err(_) => source_omission(
                    &mut report,
                    WorkGraphOmissionReason::SourceUnreadable,
                    &format!("events.jsonl:{line_number}"),
                ),
            },
            Err(_) => source_omission(
                &mut report,
                WorkGraphOmissionReason::SourceUnreadable,
                &format!("events.jsonl:{line_number}"),
            ),
        }
    }
    report.record_count = events.len();
    (events, report)
}

fn read_run_sources(
    store: Option<&RunJournalStore>,
    session_id: &str,
) -> (
    Vec<RunJournalEntry>,
    ArchiveSourceReport,
    Vec<LedgerEntry>,
    ArchiveSourceReport,
) {
    let mut journal_report = source_report(
        ArchiveSourceKind::RunJournal,
        "application_state.db:run_journal",
        false,
        0,
    );
    let mut ledger_report = source_report(
        ArchiveSourceKind::RunLedger,
        "application_state.db:run_ledger",
        false,
        0,
    );
    let Some(store) = store else {
        source_omission(
            &mut journal_report,
            WorkGraphOmissionReason::SourceUnreadable,
            "run_journal:not-attached",
        );
        source_omission(
            &mut ledger_report,
            WorkGraphOmissionReason::SourceUnreadable,
            "run_ledger:not-attached",
        );
        return (Vec::new(), journal_report, Vec::new(), ledger_report);
    };

    let journal = match store.read_journal(session_id) {
        Ok(entries) => {
            journal_report.available = true;
            journal_report.record_count = entries.len();
            entries
        }
        Err(error) => {
            source_omission(
                &mut journal_report,
                WorkGraphOmissionReason::SourceUnreadable,
                "run_journal:read-failed",
            );
            journal_report.omissions[0].detail =
                format!("run journal could not be read; runtime evidence is incomplete: {error}");
            Vec::new()
        }
    };
    let ledger = match store.read_ledger(session_id) {
        Ok(entries) => {
            ledger_report.available = true;
            ledger_report.record_count = entries.len();
            entries
        }
        Err(error) => {
            source_omission(
                &mut ledger_report,
                WorkGraphOmissionReason::SourceUnreadable,
                "run_ledger:read-failed",
            );
            ledger_report.omissions[0].detail =
                format!("run ledger could not be read; runtime evidence is incomplete: {error}");
            Vec::new()
        }
    };
    (journal, journal_report, ledger, ledger_report)
}

fn source_report(
    kind: ArchiveSourceKind,
    location: &str,
    available: bool,
    record_count: usize,
) -> ArchiveSourceReport {
    ArchiveSourceReport {
        kind,
        location: location.to_string(),
        available,
        record_count,
        omissions: Vec::new(),
    }
}

fn source_omission(
    report: &mut ArchiveSourceReport,
    reason: WorkGraphOmissionReason,
    example: &str,
) {
    if let Some(omission) = report
        .omissions
        .iter_mut()
        .find(|omission| omission.reason == reason)
    {
        omission.count = omission.count.saturating_add(1);
        if omission.examples.len() < 5 && !omission.examples.iter().any(|seen| seen == example) {
            omission.examples.push(example.to_string());
        }
    } else {
        report
            .omissions
            .push(WorkGraphOmission::new(reason, 1, vec![example.to_string()]));
    }
}

fn merge_omission(graph: &mut WorkGraph, omission: &WorkGraphOmission) {
    for index in 0..omission.count {
        let example = omission
            .examples
            .get(index.min(omission.examples.len().saturating_sub(1)))
            .map(String::as_str)
            .unwrap_or("source omission");
        record_omission(graph, omission.reason, example);
    }
}
