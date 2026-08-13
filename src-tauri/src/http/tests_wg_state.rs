//! State-transition tests for issue #219, owned by WS-2.

use std::io::Write;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

use crate::session::transitions::{
    has_rule, validate, validate_and_log, SessionStateKind as Kind, TransitionTrigger as Trigger,
};
use crate::session::SessionState;

/// Every payload-specific state change reached by `http/tests.rs`, including
/// its environment-dependent Fusion and Debate smoke paths.
const HTTP_TEST_TRANSITION_CORPUS: &[(Kind, Kind, Trigger)] = &[
    (Kind::Running, Kind::SpawningEvaluator, Trigger::EvaluatorSpawnStarted),
    (Kind::SpawningEvaluator, Kind::Running, Trigger::EvaluatorSpawnFinished),
    (Kind::Running, Kind::QaInProgress, Trigger::MilestoneReady),
    (
        Kind::QaInProgress,
        Kind::PrinceRemediation,
        Trigger::EvaluatorVerdictPass,
    ),
    (Kind::PrinceRemediation, Kind::Closing, Trigger::CloseRequested),
    (Kind::Running, Kind::Closing, Trigger::CloseRequested),
    (Kind::Closing, Kind::Closed, Trigger::CloseFinished),
    (Kind::QaFailed, Kind::SpawningEvaluator, Trigger::EvaluatorSpawnStarted),
    (Kind::SpawningEvaluator, Kind::QaFailed, Trigger::EvaluatorSpawnFinished),
    (Kind::QaFailed, Kind::QaInProgress, Trigger::MilestoneReady),
    (Kind::Running, Kind::QaPassed, Trigger::OperatorForcePass),
    (Kind::QaInProgress, Kind::QaPassed, Trigger::OperatorForcePass),
    (
        Kind::QaInProgress,
        Kind::QaPassed,
        Trigger::EvaluatorVerdictPass,
    ),
    (Kind::QaInProgress, Kind::QaFailed, Trigger::OperatorForceFail),
    (
        Kind::QaInProgress,
        Kind::QaFailed,
        Trigger::EvaluatorVerdictFail,
    ),
    (Kind::Running, Kind::Completed, Trigger::CompleteRequested),
    (Kind::QaPassed, Kind::Completed, Trigger::CompleteRequested),
    (
        Kind::Starting,
        Kind::SpawningFusionVariant,
        Trigger::FusionVariantSpawnStarted,
    ),
    (
        Kind::SpawningFusionVariant,
        Kind::WaitingForFusionVariants,
        Trigger::FusionVariantSpawned,
    ),
    (
        Kind::WaitingForFusionVariants,
        Kind::SpawningFusionVariant,
        Trigger::FusionVariantSpawnStarted,
    ),
    // The two-variant smoke fixture exercises this payload-specific row twice.
    (
        Kind::SpawningFusionVariant,
        Kind::WaitingForFusionVariants,
        Trigger::FusionVariantSpawned,
    ),
    (
        Kind::Starting,
        Kind::SpawningDebateRound,
        Trigger::DebateRoundSpawnStarted,
    ),
    (
        Kind::SpawningDebateRound,
        Kind::SpawningDebateRound,
        Trigger::DebateRoundSpawnStarted,
    ),
    (
        Kind::SpawningDebateRound,
        Kind::WaitingForDebateRound,
        Trigger::DebateRoundSpawned,
    ),
];

#[test]
fn all_http_test_transitions_are_accepted_by_the_table() {
    assert_eq!(HTTP_TEST_TRANSITION_CORPUS.len(), 24);

    let rejected = HTTP_TEST_TRANSITION_CORPUS
        .iter()
        .copied()
        .filter_map(|(from, to, trigger)| validate(from, to, trigger).err())
        .collect::<Vec<_>>();

    assert!(
        rejected.is_empty(),
        "HTTP transition corpus contained rejected rows: {rejected:?}"
    );
}

#[test]
fn state_kind_covers_all_28_payload_and_unit_variants() {
    let cases = [
        (SessionState::Planning, Kind::Planning),
        (SessionState::PlanReady, Kind::PlanReady),
        (SessionState::Starting, Kind::Starting),
        (SessionState::SpawningWorker(3), Kind::SpawningWorker),
        (SessionState::WaitingForWorker(3), Kind::WaitingForWorker),
        (SessionState::SpawningPlanner(2), Kind::SpawningPlanner),
        (SessionState::WaitingForPlanner(2), Kind::WaitingForPlanner),
        (
            SessionState::SpawningFusionVariant(1),
            Kind::SpawningFusionVariant,
        ),
        (
            SessionState::WaitingForFusionVariants,
            Kind::WaitingForFusionVariants,
        ),
        (
            SessionState::SpawningDebateRound(1),
            Kind::SpawningDebateRound,
        ),
        (
            SessionState::WaitingForDebateRound(1),
            Kind::WaitingForDebateRound,
        ),
        (SessionState::SpawningJudge, Kind::SpawningJudge),
        (SessionState::Judging, Kind::Judging),
        (
            SessionState::AwaitingVerdictSelection,
            Kind::AwaitingVerdictSelection,
        ),
        (SessionState::MergingWinner, Kind::MergingWinner),
        (SessionState::SpawningEvaluator, Kind::SpawningEvaluator),
        (
            SessionState::QaInProgress { iteration: None },
            Kind::QaInProgress,
        ),
        (SessionState::QaPassed, Kind::QaPassed),
        (
            SessionState::QaFailed { iteration: 1 },
            Kind::QaFailed,
        ),
        (
            SessionState::QaMaxRetriesExceeded,
            Kind::QaMaxRetriesExceeded,
        ),
        (SessionState::PrinceRemediation, Kind::PrinceRemediation),
        (SessionState::QaInconclusive, Kind::QaInconclusive),
        (SessionState::Running, Kind::Running),
        (SessionState::Paused, Kind::Paused),
        (SessionState::Completed, Kind::Completed),
        (SessionState::Closing, Kind::Closing),
        (SessionState::Closed, Kind::Closed),
        (SessionState::Failed("boom".to_string()), Kind::Failed),
    ];

    assert_eq!(cases.len(), 28);
    for (state, expected_kind) in cases {
        assert_eq!(state.kind(), expected_kind, "wrong kind for {state:?}");
    }
}

#[test]
fn table_metadata_preserves_the_existing_guard_memberships() {
    let terminal = [
        Kind::QaMaxRetriesExceeded,
        Kind::Completed,
        Kind::Closed,
        Kind::Failed,
    ];
    let qa_phase = [
        Kind::SpawningEvaluator,
        Kind::QaInProgress,
        Kind::QaPassed,
        Kind::QaFailed,
        Kind::QaInconclusive,
        Kind::QaMaxRetriesExceeded,
        Kind::PrinceRemediation,
    ];

    for kind in terminal {
        assert!(has_rule(kind, kind, Trigger::ClassifyTerminal));
    }
    for kind in qa_phase {
        assert!(has_rule(kind, kind, Trigger::ClassifyQaPhase));
    }

    // These look terminal but were deliberately absent from the old guard.
    assert!(!has_rule(
        Kind::QaInconclusive,
        Kind::QaInconclusive,
        Trigger::ClassifyTerminal,
    ));
    assert!(!has_rule(
        Kind::QaPassed,
        Kind::QaPassed,
        Trigger::ClassifyTerminal,
    ));
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer poisoned").extend(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for LogBuffer {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn illegal_transition_is_rejected_and_logged() {
    let from = Kind::Closed;
    let to = Kind::PlanReady;
    let trigger = Trigger::PlanProduced;

    assert!(validate(from, to, trigger).is_err());

    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_writer(logs.clone())
        .finish();
    let observed = tracing::subscriber::with_default(subscriber, || {
        validate_and_log(from, to, trigger)
    });

    assert!(observed.is_err());
    let output = String::from_utf8(
        logs.0
            .lock()
            .expect("log buffer poisoned")
            .to_vec(),
    )
    .expect("log output is UTF-8");
    assert!(output.contains("from_kind=Closed"), "missing from-kind: {output}");
    assert!(
        output.contains("to_kind=PlanReady"),
        "missing to-kind: {output}"
    );
    assert!(
        output.contains("trigger=PlanProduced"),
        "missing trigger: {output}"
    );
}
