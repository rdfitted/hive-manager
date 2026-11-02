//! Work completion is independent of process liveness. Missing proof stays working.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::pty::{AgentRole, AgentStatus};
use crate::storage::queue::{QueueRow, QueueStatus};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkState {
    #[default]
    Working,
    Finished,
    Retired,
}

pub struct WorkEvidence<'a> {
    pub agent_id: &'a str,
    pub role: &'a AgentRole,
    pub status: &'a AgentStatus,
    pub alive: bool,
    pub spawned_at: Option<DateTime<Utc>>,
    pub heartbeat: Option<(&'a str, DateTime<Utc>)>,
    pub heartbeat_is_self: bool,
    /// None means the queue could not be read; an empty slice proves no rows exist.
    pub queue: Option<&'a [QueueRow]>,
    pub verdict_at: Option<DateTime<Utc>>,
}

pub fn awaiting_injected_turn_since(status: &AgentStatus) -> Option<DateTime<Utc>> {
    let AgentStatus::WaitingForInput(reason) = status else {
        return None;
    };
    DateTime::parse_from_rfc3339(reason.strip_prefix("awaiting_injected_turn:")?)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

/// Shared by API/roster projections and retirement eligibility. This function never stops
/// a process or changes queue state. Retirement can layer its proven state over this result.
pub fn work_state(evidence: &WorkEvidence<'_>) -> WorkState {
    if matches!(evidence.status, AgentStatus::Error(_))
        || matches!(evidence.status, AgentStatus::WaitingForInput(reason)
            if reason.starts_with("injected_delivery_failed:"))
    {
        return WorkState::Working;
    }
    if matches!(evidence.status, AgentStatus::WaitingForInput(reason)
        if reason.starts_with("awaiting_injected_turn")
            && awaiting_injected_turn_since(evidence.status).is_none())
    {
        return WorkState::Working;
    }
    if let Some(injected_at) = awaiting_injected_turn_since(evidence.status) {
        if evidence.heartbeat.is_none_or(|(_, at)| injected_at > at) {
            return WorkState::Working;
        }
    }
    if matches!(evidence.status, AgentStatus::Completed) && !evidence.alive {
        return WorkState::Finished;
    }
    let Some(spawned_at) = evidence.spawned_at else {
        return WorkState::Working;
    };
    let completed = evidence
        .heartbeat
        .is_some_and(|(status, at)| status == "completed" && at >= spawned_at);
    let verdict = evidence.verdict_at.is_some_and(|at| at >= spawned_at);
    let finished = match evidence.role {
        AgentRole::Queen => completed && evidence.heartbeat_is_self,
        AgentRole::Evaluator | AgentRole::Prince => {
            // A follow-up after the verdict re-arms the current instance as well.
            let latest_proof = evidence
                .verdict_at
                .filter(|at| *at >= spawned_at)
                .into_iter()
                .chain(
                    evidence
                        .heartbeat
                        .filter(|(status, at)| {
                            evidence.heartbeat_is_self
                                && *status == "completed"
                                && *at >= spawned_at
                        })
                        .map(|(_, at)| at),
                )
                .max();
            let resumed = evidence.heartbeat.is_some_and(|(status, at)| {
                status == "working" && latest_proof.is_some_and(|proof| at > proof)
            });
            ((completed && evidence.heartbeat_is_self) || verdict)
                && !resumed
                && !awaiting_injected_turn_since(evidence.status)
                    .is_some_and(|injected_at| latest_proof.is_none_or(|at| injected_at > at))
        }
        AgentRole::MasterPlanner | AgentRole::ScratchShell => false,
        _ => {
            completed
                && evidence.queue.is_some_and(|rows| {
                    let latest = rows
                        .iter()
                        .filter(|row| row.worker_id == evidence.agent_id)
                        .map(|row| (row.assignment_id, row.created_at))
                        .max();
                    rows.iter()
                        .filter(|row| {
                            row.worker_id == evidence.agent_id
                                && Some((row.assignment_id, row.created_at)) == latest
                        })
                        .all(|row| row.status == QueueStatus::Finalized)
                })
        }
    };
    if finished {
        WorkState::Finished
    } else {
        WorkState::Working
    }
}

/// Read only durable verdicts attributed to the expected seat/session. An absent, malformed,
/// or unreadable record supplies no proof. The predicate fences its time to the instance.
pub fn verdict_at(
    session: &super::Session,
    agent: &super::AgentInfo,
    storage: Option<&crate::storage::SessionStorage>,
) -> Option<DateTime<Utc>> {
    match agent.role {
        AgentRole::Evaluator => {
            let record = crate::http::handlers::evaluator::read_qa_verdict_record(
                &storage?.session_dir(&session.id),
            )
            .ok()??;
            if record.session_id != session.id {
                return None;
            }
            DateTime::parse_from_rfc3339(&record.timestamp)
                .ok()
                .map(|at| at.with_timezone(&Utc))
        }
        AgentRole::Prince => {
            let path = session
                .project_path
                .join(".hive-manager")
                .join(&session.id)
                .join("peer/prince-verdict.json");
            let record: crate::coordination::PeerMessageRecord =
                serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
            (record.kind == "prince-verdict" && record.from == agent.id).then_some(record.timestamp)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(
        role: &AgentRole,
        heartbeat: Option<(&str, DateTime<Utc>)>,
        verdict_at: Option<DateTime<Utc>>,
        spawned_at: DateTime<Utc>,
    ) -> WorkState {
        work_state(&WorkEvidence {
            agent_id: "seat",
            role,
            status: &AgentStatus::Running,
            alive: true,
            spawned_at: Some(spawned_at),
            heartbeat,
            heartbeat_is_self: true,
            queue: Some(&[]),
            verdict_at,
        })
    }

    #[test]
    fn each_seat_requires_current_instance_proof() {
        let spawn = Utc::now();
        let before = spawn - chrono::Duration::seconds(1);
        let after = spawn + chrono::Duration::seconds(1);
        for role in [
            AgentRole::Queen,
            AgentRole::Evaluator,
            AgentRole::Prince,
            AgentRole::Worker {
                index: 1,
                parent: None,
            },
            AgentRole::QaWorker {
                index: 1,
                parent: None,
            },
            AgentRole::Planner { index: 1 },
            AgentRole::Fusion {
                variant: "a".into(),
            },
            AgentRole::Judge {
                session_id: "s".into(),
            },
        ] {
            assert_eq!(
                check(&role, None, None, spawn),
                WorkState::Working,
                "{role:?}"
            );
            assert_eq!(
                check(&role, Some(("completed", before)), Some(before), spawn),
                WorkState::Working,
                "predecessor proof must not count: {role:?}"
            );
            assert_eq!(
                check(&role, Some(("completed", after)), None, spawn),
                WorkState::Finished,
                "{role:?}"
            );
            assert_eq!(
                check(&role, Some(("idle", after)), None, spawn),
                WorkState::Working
            );
        }
        for role in [AgentRole::Evaluator, AgentRole::Prince] {
            assert_eq!(check(&role, None, Some(after), spawn), WorkState::Finished);
        }
        assert_eq!(
            check(
                &AgentRole::MasterPlanner,
                Some(("completed", after)),
                None,
                spawn
            ),
            WorkState::Working
        );
    }

    #[test]
    fn errors_and_pending_injections_override_completion() {
        let spawn = Utc::now();
        let complete = spawn + chrono::Duration::seconds(1);
        for status in [
            AgentStatus::Error("exited_during_startup:101".into()),
            AgentStatus::Error("failed".into()),
            AgentStatus::WaitingForInput("injected_delivery_failed:test".into()),
            AgentStatus::WaitingForInput("awaiting_injected_turn".into()),
            AgentStatus::WaitingForInput(format!(
                "awaiting_injected_turn:{}",
                (complete + chrono::Duration::seconds(1)).to_rfc3339()
            )),
        ] {
            assert_eq!(
                work_state(&WorkEvidence {
                    agent_id: "seat",
                    role: &AgentRole::Queen,
                    status: &status,
                    alive: true,
                    spawned_at: Some(spawn),
                    heartbeat: Some(("completed", complete)),
                    heartbeat_is_self: true,
                    queue: Some(&[]),
                    verdict_at: None
                }),
                WorkState::Working
            );
        }
    }

    #[test]
    fn completed_roster_requires_dead_process_and_unknown_instance_stays_working() {
        let mut evidence = WorkEvidence {
            agent_id: "seat",
            role: &AgentRole::Queen,
            status: &AgentStatus::Completed,
            alive: true,
            spawned_at: None,
            heartbeat: None,
            heartbeat_is_self: true,
            queue: None,
            verdict_at: None,
        };
        assert_eq!(work_state(&evidence), WorkState::Working);
        evidence.alive = false;
        assert_eq!(work_state(&evidence), WorkState::Finished);
    }

    #[test]
    fn worker_needs_latest_assignment_finalized_and_queue_readable() {
        let at = Utc::now();
        let mut rows = vec![serde_json::from_value::<QueueRow>(serde_json::json!({
            "id":"r", "task_id":"T1", "session_id":"s", "worker_id":"seat",
            "role_type":"backend", "cli":"codex", "status":"finalized", "payload":{},
            "attempts":1, "continuation_count":0, "no_progress_count":0, "last_status":null,
            "heartbeat_at":null, "assignment_id":1, "blocked_reason":null, "created_at":1,
            "updated_at":1
        }))
        .unwrap()];
        let role = AgentRole::Worker {
            index: 1,
            parent: None,
        };
        let check_rows = |queue: Option<&[QueueRow]>| {
            work_state(&WorkEvidence {
                agent_id: "seat",
                role: &role,
                status: &AgentStatus::Running,
                alive: true,
                spawned_at: Some(at),
                heartbeat: Some(("completed", at)),
                heartbeat_is_self: true,
                queue,
                verdict_at: None,
            })
        };
        assert_eq!(check_rows(None), WorkState::Working);
        assert_eq!(check_rows(Some(&rows)), WorkState::Finished);
        let mut newer = rows[0].clone();
        newer.assignment_id = 2;
        newer.status = QueueStatus::Running;
        rows.push(newer);
        assert_eq!(check_rows(Some(&rows)), WorkState::Working);
        let original_assignment = rows[0].assignment_id;
        rows[1].assignment_id = original_assignment;
        rows.reverse();
        assert_eq!(
            check_rows(Some(&rows)),
            WorkState::Working,
            "ambiguous ordering is not proof"
        );
    }

    #[test]
    fn queen_mark_is_not_self_completion_for_protected_seats() {
        let at = Utc::now();
        for role in [AgentRole::Queen, AgentRole::Evaluator, AgentRole::Prince] {
            assert_eq!(
                work_state(&WorkEvidence {
                    agent_id: "seat",
                    role: &role,
                    status: &AgentStatus::Running,
                    alive: true,
                    spawned_at: Some(at),
                    heartbeat: Some(("completed", at)),
                    heartbeat_is_self: false,
                    queue: Some(&[]),
                    verdict_at: None
                }),
                WorkState::Working
            );
        }
        assert_eq!(
            check(
                &AgentRole::Evaluator,
                Some(("working", at + chrono::Duration::seconds(1))),
                Some(at),
                at
            ),
            WorkState::Working
        );
        for role in [AgentRole::Evaluator, AgentRole::Prince] {
            let injected = at + chrono::Duration::seconds(1);
            let status = AgentStatus::WaitingForInput(format!(
                "awaiting_injected_turn:{}",
                injected.to_rfc3339()
            ));
            assert_eq!(
                work_state(&WorkEvidence {
                    agent_id: "seat",
                    role: &role,
                    status: &status,
                    alive: true,
                    spawned_at: Some(at),
                    heartbeat: Some(("completed", injected + chrono::Duration::seconds(1))),
                    heartbeat_is_self: false,
                    queue: Some(&[]),
                    verdict_at: Some(at)
                }),
                WorkState::Working
            );
        }
    }
}
