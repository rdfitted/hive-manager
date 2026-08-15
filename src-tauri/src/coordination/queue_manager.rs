//! [`QueueManager`] — the coordination-layer subsystem over the durable run queue (#126).
//!
//! Wraps the SQLite-backed [`QueueRepo`] plus the [`EventBus`]. Every mutation persists to
//! the DB FIRST and emits the matching lifecycle event SECOND, so the UI never observes an
//! event that the database has not yet committed.
//!
//! The queue table is the source of truth for sub-agent runs; the in-memory
//! `Session.agents` Vec is a UI cache reconciled against the table on resume.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;

use crate::domain::event::{Event, EventType, Severity};
use crate::events::EventBus;
use crate::orchestrator::work_graph::plan_parse::promote_initial_ready_nodes;
use crate::orchestrator::work_graph::{NodeStatus, TaskGraph};
use crate::storage::queue::{
    QueueConflictCoverage, QueueConflictRow, QueueConflictWait, QueueRepo, QueueResolutionUpdate,
    QueueRow, QueueSnapshot, QueueStatus, SpawnFailureRelease,
};
use crate::storage::StorageError;

/// A `running` row whose heartbeat is older than this (millis) is treated as stuck and is
/// reclaimable. 90s = 3x the 30s maintenance interval, comfortably inside the 180s stall
/// threshold. Reclaim only flips the row back to claimable — it never kills a live PTY, so a
/// genuinely-working-but-quiet worker that keeps heartbeating is never reclaimed.
pub const STUCK_CUTOFF_MS: i64 = 90_000;

/// `STUCK_CUTOFF_MS` in whole seconds, for prompt prose.
pub const STUCK_CUTOFF_SECS: u64 = (STUCK_CUTOFF_MS as u64) / 1_000;

/// Rounding granularity for the derived cadence, so prompts read in round numbers.
const HEARTBEAT_CADENCE_GRANULARITY_SECS: u64 = 5;

/// Headroom multiplier between the cadence workers are INSTRUCTED to heartbeat at and
/// `STUCK_CUTOFF_MS` (#141). A worker obeying the upper bound of its instruction fits at
/// least this many beats inside one cutoff window, so it takes a genuinely missed beat —
/// not ordinary jitter — before `reclaim_stuck` can requeue it. Raising the factor tightens
/// the instructed cadence; it never relaxes the cutoff.
pub const HEARTBEAT_SAFETY_FACTOR: u64 = 2;

/// Upper bound of the instructed heartbeat cadence, in seconds. DERIVED from
/// `STUCK_CUTOFF_MS` — do not hand-edit, and never hardcode a cadence into a prompt
/// template. Prompts take this through [`heartbeat_cadence_label`], which is the only way
/// the instruction and the reclaim cutoff stay pinned to each other.
pub const HEARTBEAT_MAX_INTERVAL_SECS: u64 = derived_heartbeat_max_interval_secs();

/// Lower bound of the instructed cadence. Exists only so workers do not hammer the
/// heartbeat endpoint; the reclaim invariant rides on the upper bound.
pub const HEARTBEAT_MIN_INTERVAL_SECS: u64 = derived_heartbeat_min_interval_secs();

/// Largest cadence that still leaves `HEARTBEAT_SAFETY_FACTOR` beats STRICTLY inside the
/// cutoff window, rounded down to `HEARTBEAT_CADENCE_GRANULARITY_SECS`.
const fn derived_heartbeat_max_interval_secs() -> u64 {
    let budget = STUCK_CUTOFF_SECS / HEARTBEAT_SAFETY_FACTOR;
    let rounded =
        (budget / HEARTBEAT_CADENCE_GRANULARITY_SECS) * HEARTBEAT_CADENCE_GRANULARITY_SECS;
    // Exact division lands the worker ON the cutoff, which is the #141 defect. Step down one
    // granularity so the product is strictly below it.
    let strict = if rounded * HEARTBEAT_SAFETY_FACTOR < STUCK_CUTOFF_SECS {
        rounded
    } else {
        rounded.saturating_sub(HEARTBEAT_CADENCE_GRANULARITY_SECS)
    };

    if strict == 0 {
        1
    } else {
        strict
    }
}

const fn derived_heartbeat_min_interval_secs() -> u64 {
    let half = HEARTBEAT_MAX_INTERVAL_SECS / 2;
    let rounded = (half / HEARTBEAT_CADENCE_GRANULARITY_SECS) * HEARTBEAT_CADENCE_GRANULARITY_SECS;

    if rounded == 0 {
        HEARTBEAT_MAX_INTERVAL_SECS
    } else {
        rounded
    }
}

/// The one cadence phrase every prompt must use, e.g. `every 20-40s`. Single source of
/// truth: prose in a template can no longer drift away from `STUCK_CUTOFF_MS`.
pub fn heartbeat_cadence_label() -> String {
    format!("every {HEARTBEAT_MIN_INTERVAL_SECS}-{HEARTBEAT_MAX_INTERVAL_SECS}s")
}

/// Finalize a run once it has produced this many continuations (distinct status changes).
pub const MAX_CONTINUATIONS: i64 = 8;

/// Wall-clock window a run may hold a single status before `finalize_no_progress` retires
/// it. A liveness heartbeat carries a FIXED status per phase (see `heartbeat_snippet`), so
/// consecutive beats ALWAYS take the no-progress branch in `record_heartbeat` — the budget
/// must therefore be a wall-clock decision, not a beat count. Must exceed the longest
/// legitimate quiet stretch: codex indexes 8-12 minutes before emitting any output.
pub const NO_PROGRESS_WINDOW_SECS: u64 = 1_800;

/// Finalize a run once it has produced this many consecutive no-progress heartbeats.
/// DERIVED from the instructed cadence (#141): the cadence and this budget are coupled in
/// OPPOSITE directions, so tightening `HEARTBEAT_SAFETY_FACTOR` must raise the beat count
/// rather than shrink the wall-clock budget. Hardcoding it is how a healthy worker that
/// simply beats faster gets flipped to the TERMINAL `finalized` state mid-run — after which
/// `record_heartbeat` matches no rows and `reclaim_stuck` can never recover it.
///
/// Ceiling division so the realised budget is never shorter than the window.
pub const MAX_NO_PROGRESS_CONTINUATIONS: i64 = NO_PROGRESS_WINDOW_SECS
    .div_ceil(HEARTBEAT_MIN_INTERVAL_SECS)
    as i64;

/// How many times a single queue row may be claimed for a spawn that then fails
/// before the row is retired as terminally `failed` (#175d). Bounds the retry
/// loop that releasing the claim would otherwise make unbounded.
pub const MAX_SPAWN_ATTEMPTS: i64 = 3;

/// Outcome of releasing a claim after a failed spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseAfterFailure {
    /// Row returned to `queued`; a retry may claim it.
    Released,
    /// Spawn budget exhausted; the row is terminally `failed` and only the
    /// explicit release route can revive it.
    Exhausted { attempts: i64 },
    /// We no longer held the claim (someone else reclaimed it first).
    NotHeld,
}

/// Outcome of an operator/agent claim release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseOutcome {
    Released { previous: QueueStatus },
    /// The exact claim is still constructing its worktree/PTY in this process.
    SpawnInFlight { epoch: i64 },
    AlreadyQueued,
    Terminal { status: QueueStatus },
    NoRow,
}

/// Result of coupling a manual-release side effect to the spawn-reservation guard.
///
/// The preparation closure is executed only while no matching spawn is in flight, and the
/// queue release happens before that same guard is dropped. This lets the HTTP layer discard a
/// dead roster slot without a claim-and-register operation racing into the gap.
#[derive(Debug)]
pub enum GuardedRelease<T, E> {
    SpawnInFlight(SpawnInFlightReservation),
    PreparationFailed(E),
    Complete { prepared: T, outcome: ReleaseOutcome },
}

/// An in-process reservation protecting the interval between an atomic queue claim and the
/// successful spawn handoff. It is intentionally not persisted: process death drops it so the
/// existing startup reconciliation can recover the durable `running` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnInFlightReservation {
    pub queue_id: String,
    pub epoch: i64,
    pub worker_id: String,
}

/// Clears a just-registered reservation if its claim future is cancelled before returning the
/// epoch to the spawn caller. Once disarmed, the caller owns the normal handoff/failure lifecycle.
struct ReservationReturnGuard<'a> {
    reservations: &'a Mutex<HashMap<String, SpawnInFlightReservation>>,
    reservation: SpawnInFlightReservation,
    armed: bool,
}

impl<'a> ReservationReturnGuard<'a> {
    fn new(
        reservations: &'a Mutex<HashMap<String, SpawnInFlightReservation>>,
        reservation: SpawnInFlightReservation,
    ) -> Self {
        Self {
            reservations,
            reservation,
            armed: true,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for ReservationReturnGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let reservation = &self.reservation;
            let mut reservations = self.reservations.lock();
            if reservations.get(&reservation.queue_id).is_some_and(|current| {
                current.epoch == reservation.epoch && current.worker_id == reservation.worker_id
            }) {
                reservations.remove(&reservation.queue_id);
            }
        }
    }
}

/// Result of the atomic claim UPDATE. Dependency diagnostics are gathered only after a loss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    Claimed { epoch: i64 },
    ResolutionIncomplete { task_id: String, reason: String },
    /// Retryable serialization wait. This post-loss snapshot is advisory by construction.
    ConflictsPending { conflicts: Vec<QueueConflictWait> },
    /// Retryable queue wait. The advisory prerequisite snapshot may be stale or empty when
    /// readiness changed after the loss, or when another task briefly reserved the worker slot.
    DependenciesPending { task_ids: Vec<String> },
    AlreadyClaimed,
}

impl ClaimOutcome {
    /// Compatibility helper for existing queue-mechanics callers.
    pub fn is_some(&self) -> bool {
        matches!(self, Self::Claimed { .. })
    }

    /// Compatibility helper for existing queue-mechanics callers.
    pub fn is_none(&self) -> bool {
        !self.is_some()
    }
}

/// Coordination-layer façade over the durable queue. Cheaply clonable.
#[derive(Clone)]
pub struct QueueManager {
    repo: Arc<QueueRepo>,
    event_bus: Arc<EventBus>,
    /// Serializes claim-and-register with every path that could reclaim the same claim.
    spawn_in_flight: Arc<Mutex<HashMap<String, SpawnInFlightReservation>>>,
}

impl QueueManager {
    pub fn new(repo: Arc<QueueRepo>, event_bus: Arc<EventBus>) -> Self {
        Self {
            repo,
            event_bus,
            spawn_in_flight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Current millis-since-epoch.
    fn now_ms() -> i64 {
        Utc::now().timestamp_millis()
    }

    /// Enqueue a worker run BEFORE spawning it. Persists a `queued` row, then emits
    /// `WorkerQueued`. Idempotent on `id` — a duplicate enqueue does not overwrite an
    /// already-claimed row.
    ///
    /// The arguments map 1:1 onto the queue row's identity columns; bundling them into a
    /// struct would add ceremony without clarifying the call site.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_worker(
        &self,
        id: &str,
        session_id: &str,
        worker_id: &str,
        role_type: &str,
        cli: &str,
        payload: serde_json::Value,
        task_id: Option<String>,
    ) -> Result<(), StorageError> {
        self.enqueue_worker_with_dependencies(
            id,
            session_id,
            worker_id,
            role_type,
            cli,
            payload,
            task_id,
            &[],
        )
        .await
    }

    /// Enqueue a worker and its graph-derived prerequisites in one transaction.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_worker_with_dependencies(
        &self,
        id: &str,
        session_id: &str,
        worker_id: &str,
        role_type: &str,
        cli: &str,
        payload: serde_json::Value,
        task_id: Option<String>,
        prerequisite_task_ids: &[String],
    ) -> Result<(), StorageError> {
        let now = Self::now_ms();
        let row = QueueRow {
            id: id.to_string(),
            task_id,
            session_id: session_id.to_string(),
            worker_id: worker_id.to_string(),
            role_type: role_type.to_string(),
            cli: cli.to_string(),
            status: QueueStatus::Queued,
            payload,
            attempts: 0,
            continuation_count: 0,
            no_progress_count: 0,
            last_status: None,
            heartbeat_at: None,
            assignment_id: 0,
            blocked_reason: None,
            created_at: now,
            updated_at: now,
        };
        self.repo
            .enqueue_with_dependencies(&row, prerequisite_task_ids)?;
        self.emit(
            session_id,
            worker_id,
            row.task_id.as_deref(),
            EventType::WorkerQueued,
            Severity::Info,
        )
            .await;
        Ok(())
    }

    /// Enqueue/reconcile all graph scheduling facts in the same transaction as the queue row.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_worker_with_scheduling(
        &self,
        id: &str,
        session_id: &str,
        worker_id: &str,
        role_type: &str,
        cli: &str,
        payload: serde_json::Value,
        task_id: Option<String>,
        prerequisite_task_ids: &[String],
        resolution: &QueueResolutionUpdate,
        conflicts: &[QueueConflictRow],
        conflict_coverage: Option<&QueueConflictCoverage>,
        reconcile_conflict_task_id: Option<&str>,
    ) -> Result<(), StorageError> {
        let now = Self::now_ms();
        let row = QueueRow {
            id: id.to_string(),
            task_id,
            session_id: session_id.to_string(),
            worker_id: worker_id.to_string(),
            role_type: role_type.to_string(),
            cli: cli.to_string(),
            status: QueueStatus::Queued,
            payload,
            attempts: 0,
            continuation_count: 0,
            no_progress_count: 0,
            last_status: None,
            heartbeat_at: None,
            assignment_id: 0,
            blocked_reason: None,
            created_at: now,
            updated_at: now,
        };
        self.repo.enqueue_with_scheduling(
            &row,
            prerequisite_task_ids,
            resolution,
            conflicts,
            conflict_coverage,
            reconcile_conflict_task_id,
        )?;
        self.emit(
            session_id,
            worker_id,
            row.task_id.as_deref(),
            EventType::WorkerQueued,
            Severity::Info,
        )
            .await;
        Ok(())
    }

    /// Atomically claim a ready queued (or stale-running) row, flipping it to `running`.
    ///
    /// The UPDATE loss is authoritative. Only after it loses do we perform an explanatory
    /// read distinguishing a still-queued retry from an already-held claim. The diagnostic
    /// read never gates or retries the UPDATE and its prerequisite IDs are advisory by
    /// construction; an empty list remains retryable after a readiness/worker-slot race.
    pub async fn claim_and_reserve_spawn(
        &self,
        id: &str,
        session_id: &str,
        worker_id: &str,
        task_id: Option<&str>,
    ) -> Result<ClaimOutcome, StorageError> {
        self.claim_and_spawn_inner(id, session_id, worker_id, true, task_id)
            .await
    }

    /// Test-only legacy claim seam for constructing leaked/orphaned rows without pretending a
    /// real spawn is still in flight. Production has no unreserved claim API.
    #[cfg(test)]
    pub async fn claim_and_spawn(
        &self,
        id: &str,
        session_id: &str,
        worker_id: &str,
    ) -> Result<ClaimOutcome, StorageError> {
        self.claim_and_spawn_inner(id, session_id, worker_id, false, None)
            .await
    }

    async fn claim_and_spawn_inner(
        &self,
        id: &str,
        session_id: &str,
        worker_id: &str,
        reserve_spawn: bool,
        claimed_task_id: Option<&str>,
    ) -> Result<ClaimOutcome, StorageError> {
        let now = Self::now_ms();
        let cutoff = now - STUCK_CUTOFF_MS;
        // This mutex is the prevention boundary around the durable atomic UPDATE. An existing
        // marker may only deny a retry; it never authorizes a claim. On a win the marker is
        // inserted before the critical section ends, so maintenance/manual recovery cannot
        // observe a won-but-unreserved spawn interval.
        let epoch = {
            let mut spawn_in_flight = self.spawn_in_flight.lock();
            if reserve_spawn
                && (spawn_in_flight.contains_key(id)
                    || spawn_in_flight
                        .values()
                        .any(|reservation| reservation.worker_id == worker_id))
            {
                None
            } else {
                let epoch = self
                    .repo
                    .try_claim_for_worker(id, Some(worker_id), cutoff, now)?;
                if reserve_spawn {
                    if let Some(epoch) = epoch {
                        spawn_in_flight.insert(
                            id.to_string(),
                            SpawnInFlightReservation {
                                queue_id: id.to_string(),
                                epoch,
                                worker_id: worker_id.to_string(),
                            },
                        );
                    }
                }
                epoch
            }
        };
        if let Some(epoch) = epoch {
            // Do not perform a fallible diagnostic read after registration. Any error there
            // would strand a fail-closed reservation without returning its epoch to the spawn
            // caller. The explicit enqueue binding is already known at this boundary.
            if reserve_spawn {
                // Event persistence remains ordered before the handler can spawn. If this future
                // is cancelled during that I/O, the guard removes only this exact reservation:
                // no caller received the epoch, so no spawn can be in flight. After the await,
                // disarming transfers cleanup responsibility to handoff/failure paths.
                let return_guard = ReservationReturnGuard::new(
                    &self.spawn_in_flight,
                    SpawnInFlightReservation {
                        queue_id: id.to_string(),
                        epoch,
                        worker_id: worker_id.to_string(),
                    },
                );
                self.emit(
                    session_id,
                    worker_id,
                    claimed_task_id,
                    EventType::WorkerClaimed,
                    Severity::Info,
                )
                    .await;
                return_guard.disarm();
            } else {
                self.emit(
                    session_id,
                    worker_id,
                    claimed_task_id,
                    EventType::WorkerClaimed,
                    Severity::Info,
                )
                    .await;
            }
            Ok(ClaimOutcome::Claimed { epoch })
        } else {
            // Losing the atomic UPDATE is authoritative. Reads below are diagnostic only and
            // cannot authorize or retry a claim.
            let row_after_claim = self.repo.get_row(id)?;
            let task_id = row_after_claim
                .as_ref()
                .and_then(|row| row.task_id.clone());
            let queued_after_loss = row_after_claim
                .as_ref()
                .is_some_and(|row| row.status == QueueStatus::Queued);
            let resolution_issue = queued_after_loss
                .then(|| self.repo.resolution_issue(id))
                .transpose()?
                .flatten();
            let conflicts = if queued_after_loss && resolution_issue.is_none() {
                self.repo.pending_conflicts(id)?
            } else {
                Vec::new()
            };
            let task_ids = if queued_after_loss
                && resolution_issue.is_none()
                && conflicts.is_empty()
            {
                self.repo.pending_dependencies(id)?
            } else {
                Vec::new()
            };
            self.emit(
                session_id,
                worker_id,
                task_id.as_deref(),
                EventType::WorkerClaimFailed,
                if queued_after_loss {
                    Severity::Info
                } else {
                    Severity::Warning
                },
            )
            .await;
            // These reads only explain the already-lost UPDATE. Conflict/dependency snapshots
            // can become stale immediately and must never feed a retry or authorize a claim.
            if let Some(issue) = resolution_issue {
                Ok(ClaimOutcome::ResolutionIncomplete {
                    task_id: issue.task_id,
                    reason: issue.reason,
                })
            } else if !conflicts.is_empty() {
                Ok(ClaimOutcome::ConflictsPending { conflicts })
            } else if queued_after_loss {
                Ok(ClaimOutcome::DependenciesPending { task_ids })
            } else {
                Ok(ClaimOutcome::AlreadyClaimed)
            }
        }
    }

    /// Final fenced handoff after worktree/PTY creation succeeds.
    ///
    /// The reservation is cleared only if the durable row still matches the same running epoch
    /// and worker. A zero-row update or storage error retains the marker and therefore fails
    /// closed: retries, release, and maintenance cannot reclaim through an uncertain handoff.
    pub fn complete_spawn_handoff(
        &self,
        id: &str,
        epoch: i64,
        worker_id: &str,
    ) -> Result<bool, StorageError> {
        let mut spawn_in_flight = self.spawn_in_flight.lock();
        let exact = spawn_in_flight.get(id).is_some_and(|reservation| {
            reservation.epoch == epoch && reservation.worker_id == worker_id
        });
        if !exact {
            return Ok(false);
        }

        let refreshed = self
            .repo
            .refresh_claimed_spawn(id, epoch, worker_id, Self::now_ms())?;
        if refreshed {
            spawn_in_flight.remove(id);
        }
        Ok(refreshed)
    }

    /// Snapshot one reservation for diagnostics/tests. The returned copy cannot mutate the
    /// manager's fail-closed marker.
    pub fn spawn_in_flight(&self, id: &str) -> Option<SpawnInFlightReservation> {
        self.spawn_in_flight.lock().get(id).cloned()
    }

    /// Find a protected spawn by its reserved worker slot. This is a deny-only lookup used by
    /// the manual release handler before it touches roster or PTY state.
    pub fn spawn_in_flight_for_worker(
        &self,
        worker_id: &str,
    ) -> Option<SpawnInFlightReservation> {
        self.spawn_in_flight
            .lock()
            .values()
            .find(|reservation| reservation.worker_id == worker_id)
            .cloned()
    }

    /// Release a claim whose spawn failed (#175d).
    ///
    /// Without this, a spawn that failed after winning the claim left the row
    /// `running` forever: retries inside the 90s freshness window lost the claim
    /// (409), the 30s maintenance pass then reclaimed it, the next retry won and
    /// failed again — a permanent 409/500 oscillation with the worker index
    /// pinned, and no API to recover.
    ///
    /// Past [`MAX_SPAWN_ATTEMPTS`] the row is marked terminally `failed` instead.
    /// Releasing without a cap converts a bounded oscillation into an unbounded
    /// retry loop, and each iteration re-runs worktree creation and branch
    /// deletion against the operator's real repository.
    pub async fn release_after_failed_spawn(
        &self,
        _session_id: &str,
        worker_id: &str,
        id: &str,
        epoch: i64,
    ) -> Result<ReleaseAfterFailure, StorageError> {
        let now = Self::now_ms();
        let result = {
            let mut spawn_in_flight = self.spawn_in_flight.lock();
            if let Some(reservation) = spawn_in_flight.get(id) {
                if reservation.epoch != epoch || reservation.worker_id != worker_id {
                    return Ok(ReleaseAfterFailure::NotHeld);
                }
            }

            let result = match self.repo.release_failed_spawn(
                id,
                epoch,
                worker_id,
                MAX_SPAWN_ATTEMPTS,
                now,
            )? {
                SpawnFailureRelease::Requeued { .. } => ReleaseAfterFailure::Released,
                SpawnFailureRelease::Exhausted { failures } => {
                    ReleaseAfterFailure::Exhausted { attempts: failures }
                }
                SpawnFailureRelease::NotHeld => ReleaseAfterFailure::NotHeld,
            };

            // This spawn path is ending. Clear only its exact reservation; a stale epoch-1
            // failure can never erase the marker protecting an already-won epoch 2.
            if spawn_in_flight.get(id).is_some_and(|reservation| {
                reservation.epoch == epoch && reservation.worker_id == worker_id
            }) {
                spawn_in_flight.remove(id);
            }
            result
        };

        match result {
            ReleaseAfterFailure::Exhausted { attempts } => {
                self.emit_for_row(id, EventType::WorkerFinalized, Severity::Warning)
                .await;
                Ok(ReleaseAfterFailure::Exhausted { attempts })
            }
            ReleaseAfterFailure::Released => {
                // Reuse the event `reconcile` already emits for the identical repair.
                self.emit_for_row(id, EventType::WorkerReclaimed, Severity::Warning)
                    .await;
                Ok(ReleaseAfterFailure::Released)
            }
            ReleaseAfterFailure::NotHeld => Ok(ReleaseAfterFailure::NotHeld),
        }
    }

    /// Perform handler-owned release preparation and the durable queue release under the same
    /// mutex used by claim-and-register.
    ///
    /// The closure must be short and synchronous. It may reject or mutate roster/PTY-adjacent
    /// state, but a new spawn reservation cannot appear between that mutation and the queue
    /// release. Existing in-flight reservations fail closed before the closure is invoked.
    pub async fn release_claim_with_preparation<T, E, F>(
        &self,
        session_id: &str,
        worker_id: &str,
        prepare: F,
    ) -> Result<GuardedRelease<T, E>, StorageError>
    where
        T: Send,
        E: Send,
        F: FnOnce() -> Result<T, E> + Send,
    {
        let (result, reclaimed_id) = {
            let spawn_in_flight = self.spawn_in_flight.lock();
            let rows = self.repo.rows_for_session(session_id)?;
            // Task-backed rows retain historical worker bindings when terminal. Prefer the
            // live claim, then failed recovery target, before any older terminal row.
            let row = rows
                .iter()
                .find(|row| row.worker_id == worker_id && row.status == QueueStatus::Running)
                .or_else(|| {
                    rows.iter()
                        .find(|row| row.worker_id == worker_id && row.status == QueueStatus::Failed)
                })
                .or_else(|| rows.iter().find(|row| row.worker_id == worker_id));
            if let Some(reservation) = row.and_then(|row| spawn_in_flight.get(&row.id)) {
                return Ok(GuardedRelease::SpawnInFlight(reservation.clone()));
            }

            let prepared = match prepare() {
                Ok(prepared) => prepared,
                Err(error) => return Ok(GuardedRelease::PreparationFailed(error)),
            };
            let (outcome, reclaimed_id) = match row {
                None => (ReleaseOutcome::NoRow, None),
                Some(row) => match row.status {
                    QueueStatus::Running | QueueStatus::Failed => {
                        let previous = row.status;
                        if self.repo.release_claim_manual(&row.id, Self::now_ms())? {
                            (
                                ReleaseOutcome::Released { previous },
                                Some(row.id.clone()),
                            )
                        } else {
                            (ReleaseOutcome::NoRow, None)
                        }
                    }
                    QueueStatus::Queued => (ReleaseOutcome::AlreadyQueued, None),
                    other => (ReleaseOutcome::Terminal { status: other }, None),
                },
            };
            (
                GuardedRelease::Complete { prepared, outcome },
                reclaimed_id,
            )
        };

        if let Some(id) = reclaimed_id {
            self.emit_for_row(&id, EventType::WorkerReclaimed, Severity::Warning)
                .await;
        }
        Ok(result)
    }

    /// Operator/agent recovery for an orphaned claim (#175e).
    pub async fn release_claim(
        &self,
        session_id: &str,
        worker_id: &str,
    ) -> Result<ReleaseOutcome, StorageError> {
        let (outcome, reclaimed_id) = {
            let spawn_in_flight = self.spawn_in_flight.lock();
            let rows = self.repo.rows_for_session(session_id)?;
            // Task-backed queue rows keep their historical worker binding when terminal, so a
            // reused slot can legitimately have older finalized/failed rows. Recovery must target
            // the live claim first; a terminal historical row must never mask it.
            let row = rows
                .iter()
                .find(|row| row.worker_id == worker_id && row.status == QueueStatus::Running)
                .or_else(|| {
                    rows.iter()
                        .find(|row| row.worker_id == worker_id && row.status == QueueStatus::Failed)
                })
                .or_else(|| rows.iter().find(|row| row.worker_id == worker_id));
            let Some(row) = row else {
                return Ok(ReleaseOutcome::NoRow);
            };

            if let Some(reservation) = spawn_in_flight.get(&row.id) {
                if row.status == QueueStatus::Running
                    && row.attempts == reservation.epoch
                    && row.worker_id == reservation.worker_id
                {
                    return Ok(ReleaseOutcome::SpawnInFlight {
                        epoch: reservation.epoch,
                    });
                }
            }

            match row.status {
                QueueStatus::Running | QueueStatus::Failed => {
                    let previous = row.status;
                    if self.repo.release_claim_manual(&row.id, Self::now_ms())? {
                        (
                            ReleaseOutcome::Released { previous },
                            Some(row.id.clone()),
                        )
                    } else {
                        (ReleaseOutcome::NoRow, None)
                    }
                }
                QueueStatus::Queued => (ReleaseOutcome::AlreadyQueued, None),
                other => (ReleaseOutcome::Terminal { status: other }, None),
            }
        };

        if let Some(id) = reclaimed_id {
            self.emit_for_row(&id, EventType::WorkerReclaimed, Severity::Warning)
                .await;
        }
        Ok(outcome)
    }

    /// Record a heartbeat against the queue row, advancing continuation / no-progress
    /// counters. A completed heartbeat atomically finalizes the row and emits the matching
    /// lifecycle event. Returns `true` if a row was updated.
    pub async fn record_heartbeat(
        &self,
        session_id: &str,
        worker_id: &str,
        status: &str,
    ) -> Result<bool, StorageError> {
        self.record_heartbeat_for_assignment(session_id, worker_id, None, status)
            .await
    }

    /// Record a heartbeat for one durable assignment, falling back deterministically only
    /// when an older caller omits `assignment_id`.
    pub async fn record_heartbeat_for_assignment(
        &self,
        session_id: &str,
        worker_id: &str,
        assignment_id: Option<i64>,
        status: &str,
    ) -> Result<bool, StorageError> {
        let now = Self::now_ms();
        let updated_row_id = self
            .repo
            .record_heartbeat_for_assignment(
                session_id,
                worker_id,
                assignment_id,
                status,
                now,
            )?;
        if status == "completed" {
            if let Some(row_id) = updated_row_id.as_deref() {
                self.emit_for_row(row_id, EventType::WorkerFinalized, Severity::Info)
                    .await;
            }
        }
        Ok(updated_row_id.is_some())
    }

    /// Maintenance: flip stale `running` rows back to `queued`. Emits `WorkerReclaimed`
    /// per reclaimed row.
    pub async fn reclaim_stuck(&self) -> Result<Vec<String>, StorageError> {
        let now = Self::now_ms();
        let cutoff = now - STUCK_CUTOFF_MS;
        self.reclaim_stuck_at(cutoff, now).await
    }

    /// Refresh every exact in-flight claim before the stale-row maintenance UPDATE.
    ///
    /// Both operations occur under the same manager mutex used by claim-and-register. If any
    /// reservation cannot be refreshed with all fences (`id + running + epoch + worker`), the
    /// maintenance pass returns without reclaiming anything. That conservative response is
    /// intentional: an inconsistent marker may only fail closed.
    pub(crate) async fn reclaim_stuck_at(
        &self,
        cutoff: i64,
        now: i64,
    ) -> Result<Vec<String>, StorageError> {
        let ids = {
            let spawn_in_flight = self.spawn_in_flight.lock();
            for reservation in spawn_in_flight.values() {
                if !self.repo.refresh_claimed_spawn(
                    &reservation.queue_id,
                    reservation.epoch,
                    &reservation.worker_id,
                    now,
                )? {
                    return Ok(Vec::new());
                }
            }
            self.repo.reclaim_stuck(cutoff, now)?
        };
        for id in &ids {
            self.emit_for_row(id, EventType::WorkerReclaimed, Severity::Warning)
                .await;
        }
        Ok(ids)
    }

    /// Maintenance: finalize runs over the continuation / no-progress budgets. Emits
    /// `WorkerFinalized` per finalized row.
    pub async fn finalize_no_progress(&self) -> Result<Vec<String>, StorageError> {
        let now = Self::now_ms();
        let ids = self
            .repo
            .finalize_no_progress(MAX_CONTINUATIONS, MAX_NO_PROGRESS_CONTINUATIONS, now)?;
        for id in &ids {
            self.emit_for_row(id, EventType::WorkerFinalized, Severity::Info)
                .await;
        }
        Ok(ids)
    }

    /// Run both maintenance passes (called from the 30s background task).
    pub async fn run_maintenance(&self) -> Result<(), StorageError> {
        self.reclaim_stuck().await?;
        self.finalize_no_progress().await?;
        Ok(())
    }

    /// Repair orphaned `running` rows on resume: any `running` row whose worker is NOT in
    /// `live_worker_ids` (its PTY did not survive the crash) is flipped back to `queued`.
    /// Returns the reclaimed ids.
    pub async fn reconcile(
        &self,
        session_id: &str,
        live_worker_ids: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let now = Self::now_ms();
        let reclaimed = {
            let spawn_in_flight = self.spawn_in_flight.lock();
            let rows = self.repo.rows_for_session(session_id)?;
            let mut reclaimed = Vec::new();
            for row in rows {
                let orphaned = row.status == QueueStatus::Running
                    && !spawn_in_flight.contains_key(&row.id)
                    && !live_worker_ids.iter().any(|w| w == &row.worker_id);
                if orphaned && self.repo.requeue_running(&row.id, now)? {
                    reclaimed.push(row.id);
                }
            }
            reclaimed
        };
        for id in &reclaimed {
            self.emit_for_row(id, EventType::WorkerReclaimed, Severity::Warning)
                .await;
        }
        Ok(reclaimed)
    }

    /// Counts + rows for a session's queue (dashboard endpoint).
    pub fn queue_snapshot(&self, session_id: &str) -> Result<QueueSnapshot, StorageError> {
        self.repo.snapshot(session_id)
    }

    #[cfg(test)]
    pub fn spawn_failure_count(&self, id: &str) -> Result<i64, StorageError> {
        self.repo.spawn_failure_count(id)
    }

    /// Overlay authoritative queue status onto an in-memory graph clone for analytical
    /// readiness consumers. SQL remains the sole claim authority and never reads this JSON
    /// projection.
    pub fn project_queue_statuses(
        &self,
        session_id: &str,
        graph: &TaskGraph,
    ) -> Result<TaskGraph, StorageError> {
        let mut projected = graph.clone();
        let mut latest: BTreeMap<&str, &QueueRow> = BTreeMap::new();
        let rows = self.repo.rows_for_session(session_id)?;
        for row in &rows {
            let Some(task_id) = row.task_id.as_deref() else {
                continue;
            };
            let replace = latest.get(task_id).is_none_or(|existing| {
                (row.updated_at, row.created_at, row.id.as_str())
                    > (existing.updated_at, existing.created_at, existing.id.as_str())
            });
            if replace {
                latest.insert(task_id, row);
            }
        }

        for node in &mut projected.nodes {
            if let Some(row) = latest.get(node.id.as_str()) {
                node.status = match row.status {
                    QueueStatus::Queued => NodeStatus::Ready,
                    QueueStatus::Running => NodeStatus::Running,
                    QueueStatus::Finalized => NodeStatus::Completed,
                    QueueStatus::Failed => NodeStatus::Failed,
                    QueueStatus::Blocked => NodeStatus::Blocked,
                };
            } else if matches!(
                node.status,
                NodeStatus::Running
                    | NodeStatus::Completed
                    | NodeStatus::Failed
                    | NodeStatus::Blocked
                    | NodeStatus::Cancelled
            ) {
                // A persisted runtime-looking status without a queue row cannot satisfy the SQL
                // prerequisite lookup. Reset it conservatively before readiness promotion.
                node.status = NodeStatus::Pending;
            }
        }
        promote_initial_ready_nodes(&mut projected);
        Ok(projected)
    }

    /// Cancel a task and block all downstream queue rows with a visible reason.
    pub fn cancel_task(
        &self,
        session_id: &str,
        task_id: &str,
        reason: &str,
    ) -> Result<Vec<String>, StorageError> {
        self.repo
            .cancel_task_and_descendants(session_id, task_id, reason, Self::now_ms())
    }

    /// Borrow the underlying repo (tests / resume reconcile lookups).
    pub fn repo(&self) -> &Arc<QueueRepo> {
        &self.repo
    }

    /// Publish a queue lifecycle event AFTER the DB commit.
    async fn emit(
        &self,
        session_id: &str,
        worker_id: &str,
        task_id: Option<&str>,
        event_type: EventType,
        severity: Severity,
    ) {
        let event = Event {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            cell_id: None,
            agent_id: Some(worker_id.to_string()),
            event_type,
            timestamp: Utc::now(),
            payload: serde_json::json!({ "worker_id": worker_id, "task_id": task_id }),
            severity,
        };
        if let Err(e) = self.event_bus.publish(event).await {
            tracing::warn!("Failed to publish queue event: {e}");
        }
    }

    /// Emit using the session/worker resolved from a row id (used by maintenance passes,
    /// which only have the id).
    async fn emit_for_row(&self, id: &str, event_type: EventType, severity: Severity) {
        match self.repo.get_row(id) {
            Ok(Some(row)) => {
                self.emit(
                    &row.session_id,
                    &row.worker_id,
                    row.task_id.as_deref(),
                    event_type,
                    severity,
                )
                    .await
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("Failed to load queue row {id} for event: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::application_state::ApplicationStateDb;
    use serde_json::json;
    use tempfile::TempDir;

    fn manager() -> (TempDir, QueueManager) {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(ApplicationStateDb::open(dir.path()).unwrap());
        let repo = Arc::new(QueueRepo::new(db));
        repo.ensure_schema().unwrap();
        let event_bus = EventBus::new(dir.path().to_path_buf());
        (dir, QueueManager::new(repo, event_bus))
    }

    #[tokio::test]
    async fn test_enqueue_then_claim_lifecycle() {
        let (_dir, mgr) = manager();
        mgr.enqueue_worker(
            "s1-worker-1",
            "s1",
            "s1-worker-1",
            "backend",
            "codex",
            json!({ "model": "gpt-5.5" }),
            None,
        )
        .await
        .unwrap();

        let snap = mgr.queue_snapshot("s1").unwrap();
        assert_eq!(snap.queued, 1);

        // First claim wins, second loses (already running, fresh).
        assert!(mgr.claim_and_spawn("s1-worker-1", "s1", "s1-worker-1").await.unwrap().is_some());
        assert!(mgr.claim_and_spawn("s1-worker-1", "s1", "s1-worker-1").await.unwrap().is_none());

        let snap = mgr.queue_snapshot("s1").unwrap();
        assert_eq!(snap.running, 1);
        assert_eq!(snap.queued, 0);
    }

    #[tokio::test]
    async fn test_queue_events_emitted() {
        let (_dir, mgr) = manager();
        // Subscribe BEFORE the operations so we capture every event.
        let mut rx = mgr.event_bus.subscribe();

        mgr.enqueue_worker("r1", "s1", "s1-worker-1", "backend", "codex", json!({}), None)
            .await
            .unwrap();
        mgr.claim_and_spawn("r1", "s1", "s1-worker-1").await.unwrap();

        let e1 = rx.recv().await.unwrap();
        assert_eq!(e1.event_type, EventType::WorkerQueued);
        assert_eq!(e1.session_id, "s1");
        assert_eq!(e1.agent_id.as_deref(), Some("s1-worker-1"));
        let e2 = rx.recv().await.unwrap();
        assert_eq!(e2.event_type, EventType::WorkerClaimed);

        // A lost claim emits WorkerClaimFailed.
        mgr.claim_and_spawn("r1", "s1", "s1-worker-1").await.unwrap();
        let e3 = rx.recv().await.unwrap();
        assert_eq!(e3.event_type, EventType::WorkerClaimFailed);
    }

    /// #141 follow-up: the instructed cadence and the no-progress budget are coupled in
    /// OPPOSITE directions — a faster cadence spends the beat budget sooner. A liveness
    /// heartbeat carries a fixed status per phase, so every beat after the first takes the
    /// `last_status = ?4` branch in `record_heartbeat` and climbs `no_progress_count`. Pin
    /// the budget in WALL-CLOCK terms so tightening the cadence can never flip a healthy
    /// worker to the terminal `finalized` state mid-run.
    #[test]
    fn no_progress_budget_outlives_a_long_quiet_worker() {
        // Route through a call so the assertions exercise real values rather than being
        // const-folded into `assert!(true)`.
        fn derived_budget() -> (u64, u64, u64) {
            (
                MAX_NO_PROGRESS_CONTINUATIONS as u64,
                HEARTBEAT_MIN_INTERVAL_SECS,
                NO_PROGRESS_WINDOW_SECS,
            )
        }

        let (max_beats, min_interval, window) = derived_budget();

        // The floor: a worker obeying the FAST end of the instructed cadence burns the beat
        // budget quickest, so that product is the shortest survivable quiet stretch.
        let shortest_budget_secs = max_beats * min_interval;

        assert!(
            shortest_budget_secs >= window,
            "a worker obeying the instructed cadence is finalized after {shortest_budget_secs}s \
             of legitimate quiet work, short of the {window}s no-progress window"
        );
        // 12 minutes is the observed codex indexing ceiling — a run must survive it silently.
        assert!(
            shortest_budget_secs > 12 * 60,
            "budget {shortest_budget_secs}s is shorter than a normal codex indexing pass"
        );
    }

    /// #141 defect B: the instructed cadence used to be prose ("every 60-90s") whose upper
    /// bound landed EXACTLY on the 90s cutoff, so ordinary jitter reclaimed a healthy worker.
    /// Assert the relationship between the two values, not the literals — the point is that
    /// they cannot drift apart.
    #[test]
    fn instructed_cadence_keeps_safety_margin_under_stuck_cutoff() {
        // Read the constants through a call so the assertions exercise real values instead
        // of being folded into `assert!(true)`.
        fn derived_cadence() -> (u64, u64, u64, u64) {
            (
                HEARTBEAT_MIN_INTERVAL_SECS,
                HEARTBEAT_MAX_INTERVAL_SECS,
                HEARTBEAT_SAFETY_FACTOR,
                STUCK_CUTOFF_MS as u64,
            )
        }

        let (min, max, factor, cutoff_ms) = derived_cadence();

        assert!(
            factor >= 2,
            "a factor below 2 leaves no room for a single missed beat"
        );
        assert!(
            max * factor * 1_000 < cutoff_ms,
            "instructed max cadence {max}s x{factor} must stay strictly under \
             STUCK_CUTOFF_MS ({cutoff_ms}ms)"
        );
        assert!(
            min > 0 && min <= max,
            "cadence bounds {min}-{max}s must describe a usable range"
        );
        assert_eq!(
            heartbeat_cadence_label(),
            format!("every {min}-{max}s"),
            "the prompt-facing label must report the derived bounds verbatim"
        );
    }

    #[tokio::test]
    async fn test_reconcile_repairs_orphaned_running() {
        let (_dir, mgr) = manager();
        mgr.enqueue_worker("r1", "s1", "s1-worker-1", "backend", "codex", json!({}), None)
            .await
            .unwrap();
        mgr.claim_and_spawn("r1", "s1", "s1-worker-1").await.unwrap();
        // After a crash there is no live PTY for s1-worker-1 → reconcile requeues it.
        let reclaimed = mgr.reconcile("s1", &[]).await.unwrap();
        assert_eq!(reclaimed, vec!["r1".to_string()]);
        assert_eq!(mgr.queue_snapshot("s1").unwrap().queued, 1);

        // If the worker is still live, reconcile leaves it running.
        mgr.claim_and_spawn("r1", "s1", "s1-worker-1").await.unwrap();
        let reclaimed = mgr.reconcile("s1", &["s1-worker-1".to_string()]).await.unwrap();
        assert!(reclaimed.is_empty());
        assert_eq!(mgr.queue_snapshot("s1").unwrap().running, 1);
    }
}
