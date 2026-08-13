use serde::{Deserialize, Serialize};

/// Payload-free identity for every [`super::SessionState`] variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStateKind {
    Planning,
    PlanReady,
    Starting,
    SpawningWorker,
    WaitingForWorker,
    SpawningPlanner,
    WaitingForPlanner,
    SpawningFusionVariant,
    WaitingForFusionVariants,
    SpawningDebateRound,
    WaitingForDebateRound,
    SpawningJudge,
    Judging,
    AwaitingVerdictSelection,
    MergingWinner,
    SpawningEvaluator,
    QaInProgress,
    QaPassed,
    QaFailed,
    QaMaxRetriesExceeded,
    PrinceRemediation,
    QaInconclusive,
    Running,
    Paused,
    Completed,
    Closing,
    Closed,
    Failed,
}

/// Why a state-machine rule exists.
///
/// The four `Classify*`/`*QaIteration` values are metadata rules consumed by
/// the existing guards. The other values describe observable state changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionTrigger {
    ClassifyTerminal,
    ClassifyQaPhase,
    PreserveQaIteration,
    AdvanceQaFailureIteration,
    PlanProduced,
    PlanningFinished,
    WorkerSpawnStarted,
    WorkerSpawned,
    WorkerSequenceFinished,
    PlannerSpawned,
    FusionVariantSpawnStarted,
    FusionVariantSpawned,
    DebateRoundSpawnStarted,
    DebateRoundSpawned,
    JudgeSpawnStarted,
    JudgeSpawned,
    JudgeOutputReady,
    WinnerSelected,
    WinnerMerged,
    EvaluatorSpawnStarted,
    EvaluatorSpawnFinished,
    MilestoneReady,
    EvaluatorVerdictPass,
    EvaluatorVerdictFail,
    OperatorForcePass,
    OperatorForceFail,
    QaTimedOut,
    PrinceVerdict,
    CompleteRequested,
    StopRequested,
    CloseRequested,
    CloseFinished,
    /// Used only when the live mutator observes a pair absent from the table.
    ControllerMutation,
}

impl TransitionTrigger {
    pub const fn is_metadata(self) -> bool {
        matches!(
            self,
            Self::ClassifyTerminal
                | Self::ClassifyQaPhase
                | Self::PreserveQaIteration
                | Self::AdvanceQaFailureIteration
        )
    }
}

/// One exported row in the session state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionTransition {
    pub from_kind: SessionStateKind,
    pub to_kind: SessionStateKind,
    pub trigger: TransitionTrigger,
}

macro_rules! transition {
    ($from:ident => $to:ident, $trigger:ident) => {
        SessionTransition {
            from_kind: SessionStateKind::$from,
            to_kind: SessionStateKind::$to,
            trigger: TransitionTrigger::$trigger,
        }
    };
}

/// Queryable source of truth for session-state classification and transitions.
///
/// Self-rows with metadata triggers encode the old unary `matches!` guards so
/// those classifications remain exportable alongside the state-change graph.
pub const SESSION_STATE_TRANSITIONS: &[SessionTransition] = &[
    // Terminal-state classification (intentionally excludes QaInconclusive).
    transition!(QaMaxRetriesExceeded => QaMaxRetriesExceeded, ClassifyTerminal),
    transition!(Completed => Completed, ClassifyTerminal),
    transition!(Closed => Closed, ClassifyTerminal),
    transition!(Failed => Failed, ClassifyTerminal),
    // QA-phase classification.
    transition!(SpawningEvaluator => SpawningEvaluator, ClassifyQaPhase),
    transition!(QaInProgress => QaInProgress, ClassifyQaPhase),
    transition!(QaPassed => QaPassed, ClassifyQaPhase),
    transition!(QaFailed => QaFailed, ClassifyQaPhase),
    transition!(QaInconclusive => QaInconclusive, ClassifyQaPhase),
    transition!(QaMaxRetriesExceeded => QaMaxRetriesExceeded, ClassifyQaPhase),
    transition!(PrinceRemediation => PrinceRemediation, ClassifyQaPhase),
    // Payload-bearing QA helper classifications.
    transition!(QaFailed => QaInProgress, PreserveQaIteration),
    transition!(QaInProgress => QaInProgress, PreserveQaIteration),
    transition!(QaFailed => QaFailed, AdvanceQaFailureIteration),
    transition!(QaInProgress => QaFailed, AdvanceQaFailureIteration),
    // Planning and sequential Hive execution.
    transition!(Planning => PlanReady, PlanProduced),
    transition!(Planning => Running, PlanningFinished),
    transition!(PlanReady => Running, PlanningFinished),
    transition!(Running => SpawningWorker, WorkerSpawnStarted),
    transition!(WaitingForWorker => SpawningWorker, WorkerSpawnStarted),
    transition!(SpawningWorker => WaitingForWorker, WorkerSpawned),
    transition!(WaitingForWorker => Running, WorkerSequenceFinished),
    transition!(Running => WaitingForPlanner, PlannerSpawned),
    transition!(WaitingForPlanner => WaitingForPlanner, PlannerSpawned),
    // Fusion and Debate execution.
    transition!(Starting => SpawningFusionVariant, FusionVariantSpawnStarted),
    transition!(WaitingForFusionVariants => SpawningFusionVariant, FusionVariantSpawnStarted),
    transition!(SpawningFusionVariant => WaitingForFusionVariants, FusionVariantSpawned),
    transition!(Starting => SpawningDebateRound, DebateRoundSpawnStarted),
    transition!(WaitingForDebateRound => SpawningDebateRound, DebateRoundSpawnStarted),
    transition!(SpawningDebateRound => SpawningDebateRound, DebateRoundSpawnStarted),
    transition!(SpawningDebateRound => WaitingForDebateRound, DebateRoundSpawned),
    transition!(WaitingForFusionVariants => SpawningJudge, JudgeSpawnStarted),
    transition!(WaitingForDebateRound => SpawningJudge, JudgeSpawnStarted),
    transition!(SpawningJudge => Judging, JudgeSpawned),
    transition!(Judging => AwaitingVerdictSelection, JudgeOutputReady),
    transition!(AwaitingVerdictSelection => MergingWinner, WinnerSelected),
    transition!(MergingWinner => Completed, WinnerMerged),
    // Evaluator lifecycle. SpawningEvaluator is a temporary state; the tail
    // restores the captured pre-spawn state before a milestone opens QA.
    transition!(Running => SpawningEvaluator, EvaluatorSpawnStarted),
    transition!(QaFailed => SpawningEvaluator, EvaluatorSpawnStarted),
    transition!(QaInProgress => SpawningEvaluator, EvaluatorSpawnStarted),
    transition!(SpawningEvaluator => Running, EvaluatorSpawnFinished),
    transition!(SpawningEvaluator => QaFailed, EvaluatorSpawnFinished),
    transition!(SpawningEvaluator => QaInProgress, EvaluatorSpawnFinished),
    transition!(Running => QaInProgress, MilestoneReady),
    transition!(QaFailed => QaInProgress, MilestoneReady),
    transition!(QaInProgress => QaInProgress, MilestoneReady),
    // QA and Prince verdicts.
    transition!(QaInProgress => PrinceRemediation, EvaluatorVerdictPass),
    transition!(QaInProgress => PrinceRemediation, EvaluatorVerdictFail),
    transition!(QaInProgress => QaPassed, EvaluatorVerdictPass),
    transition!(QaInProgress => QaFailed, EvaluatorVerdictFail),
    transition!(QaInProgress => QaMaxRetriesExceeded, EvaluatorVerdictFail),
    transition!(Running => QaPassed, OperatorForcePass),
    transition!(QaInProgress => QaPassed, OperatorForcePass),
    transition!(QaInconclusive => QaPassed, OperatorForcePass),
    transition!(PrinceRemediation => QaPassed, OperatorForcePass),
    transition!(QaInProgress => QaFailed, OperatorForceFail),
    transition!(QaInconclusive => QaFailed, OperatorForceFail),
    transition!(PrinceRemediation => QaFailed, OperatorForceFail),
    transition!(QaInProgress => QaMaxRetriesExceeded, OperatorForceFail),
    transition!(QaInconclusive => QaMaxRetriesExceeded, OperatorForceFail),
    transition!(PrinceRemediation => QaMaxRetriesExceeded, OperatorForceFail),
    transition!(QaInProgress => QaInconclusive, QaTimedOut),
    transition!(PrinceRemediation => QaPassed, PrinceVerdict),
    transition!(PrinceRemediation => QaInconclusive, PrinceVerdict),
    // Completion and cleanup paths exercised by the existing HTTP corpus.
    transition!(Running => Completed, CompleteRequested),
    transition!(QaPassed => Completed, CompleteRequested),
    transition!(Running => Completed, StopRequested),
    transition!(QaPassed => Completed, StopRequested),
    transition!(Running => Closing, CloseRequested),
    transition!(QaInProgress => Closing, CloseRequested),
    transition!(QaPassed => Closing, CloseRequested),
    transition!(QaFailed => Closing, CloseRequested),
    transition!(QaMaxRetriesExceeded => Closing, CloseRequested),
    transition!(PrinceRemediation => Closing, CloseRequested),
    transition!(QaInconclusive => Closing, CloseRequested),
    transition!(Completed => Closing, CloseRequested),
    transition!(Closing => Closed, CloseFinished),
];

/// Return whether the exact exported rule exists.
pub fn has_rule(
    from_kind: SessionStateKind,
    to_kind: SessionStateKind,
    trigger: TransitionTrigger,
) -> bool {
    SESSION_STATE_TRANSITIONS.iter().any(|transition| {
        transition.from_kind == from_kind
            && transition.to_kind == to_kind
            && transition.trigger == trigger
    })
}

/// Return the first state-change rule for a pair, excluding guard metadata.
pub fn transition_for(
    from_kind: SessionStateKind,
    to_kind: SessionStateKind,
) -> Option<&'static SessionTransition> {
    SESSION_STATE_TRANSITIONS.iter().find(|transition| {
        transition.from_kind == from_kind
            && transition.to_kind == to_kind
            && !transition.trigger.is_metadata()
    })
}

/// Pick the table's canonical trigger for a live mutator that only knows the
/// state pair. Callers with stronger context should pass that trigger directly.
pub fn trigger_for(
    from_kind: SessionStateKind,
    to_kind: SessionStateKind,
) -> Option<TransitionTrigger> {
    transition_for(from_kind, to_kind).map(|transition| transition.trigger)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IllegalTransition {
    pub from_kind: SessionStateKind,
    pub to_kind: SessionStateKind,
    pub trigger: TransitionTrigger,
}

impl std::fmt::Display for IllegalTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "illegal session transition {:?} -> {:?} ({:?})",
            self.from_kind, self.to_kind, self.trigger
        )
    }
}

impl std::error::Error for IllegalTransition {}

/// Reject an absent table row without panicking.
pub fn validate(
    from_kind: SessionStateKind,
    to_kind: SessionStateKind,
    trigger: TransitionTrigger,
) -> Result<(), IllegalTransition> {
    if has_rule(from_kind, to_kind, trigger) {
        Ok(())
    } else {
        Err(IllegalTransition {
            from_kind,
            to_kind,
            trigger,
        })
    }
}

/// Validate and emit the live-app diagnostic required for an absent row.
///
/// The controller deliberately proceeds after this returns `Err`; #219 is a
/// behavior-preserving visibility change, while strict enforcement remains at
/// the query API until the production transition corpus is proven complete.
pub fn validate_and_log(
    from_kind: SessionStateKind,
    to_kind: SessionStateKind,
    trigger: TransitionTrigger,
) -> Result<(), IllegalTransition> {
    validate(from_kind, to_kind, trigger).map_err(|error| {
        tracing::warn!(
            from_kind = ?error.from_kind,
            to_kind = ?error.to_kind,
            trigger = ?error.trigger,
            "Illegal session state transition observed; applying for backward compatibility"
        );
        error
    })
}
