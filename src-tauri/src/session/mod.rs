pub(crate) mod cell_status;
mod controller;
mod fusion_judgment;
mod polling_intervals;
mod prompt_contract;
pub mod retirement;
pub mod slop_gate;
pub mod transitions;
pub mod work_state;

#[allow(unused_imports)]
pub use controller::{
    AddWorkerError, AddWorkerRejection, AddWorkerRejectionReason, AddWorkerReservation, AgentInfo,
    AuthStrategy, CompletionBlockedError, CompletionError, DebateDebaterConfig, DebateDebaterStatus,
    DebateLaunchConfig, FusionLaunchConfig, FusionVariantConfig, FusionVariantStatus,
    HiveLaunchConfig, PlanningPhaseError, QaWorkerConfig, ResearchLaunchConfig, Session,
    SessionController, SessionState, SessionType, SwarmLaunchConfig, DEFAULT_MAX_QA_ITERATIONS,
};
#[allow(unused_imports)]
pub use transitions::{SessionStateKind, SessionTransition, TransitionTrigger};
