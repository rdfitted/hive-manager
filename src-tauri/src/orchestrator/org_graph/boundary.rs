//! Verification-boundary semantics.
//!
//! A [`ContextBoundary`] is the maximum inherited context a verifier may receive.
//! `None` is therefore the strongest isolation, while `Full` is the weakest.

use super::{ContextBoundary, SignalClass};

/// Return the least-isolated boundary permitted for a signal class.
pub const fn required_context_boundary(signal_class: SignalClass) -> ContextBoundary {
    match signal_class {
        SignalClass::Mechanical => ContextBoundary::Full,
        SignalClass::Judgmental => ContextBoundary::Artifact,
    }
}

/// Whether the declared boundary provides enough isolation for the signal class.
pub const fn context_boundary_satisfies(
    signal_class: SignalClass,
    actual: ContextBoundary,
) -> bool {
    match (signal_class, actual) {
        (SignalClass::Mechanical, _) => true,
        (SignalClass::Judgmental, ContextBoundary::None | ContextBoundary::Artifact) => true,
        (SignalClass::Judgmental, ContextBoundary::Full) => false,
    }
}

/// Whether a verification duty names a real, non-whitespace signal.
pub fn verification_duty_has_named_signal(signal_name: Option<&str>) -> bool {
    match signal_name {
        Some(signal_name) => !signal_name.trim().is_empty(),
        None => false,
    }
}

/// Whether a verification duty explicitly declares its signal class.
pub const fn verification_duty_declares_signal_class(
    signal_class: Option<SignalClass>,
) -> bool {
    signal_class.is_some()
}

/// Whether artifact context may be included in a composed prompt.
pub const fn includes_artifact_context(boundary: ContextBoundary) -> bool {
    match boundary {
        ContextBoundary::None => false,
        ContextBoundary::Artifact | ContextBoundary::Full => true,
    }
}

/// Whether spawner conversation may be included in a composed prompt.
pub const fn includes_spawner_conversation(boundary: ContextBoundary) -> bool {
    match boundary {
        ContextBoundary::None | ContextBoundary::Artifact => false,
        ContextBoundary::Full => true,
    }
}
