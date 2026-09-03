// CLI registry module - infrastructure for future CLI management features
pub mod agent_store;
pub mod health;
mod registry;
pub mod tier_ladder;

pub use registry::{CliBehavior, CliRegistry};
pub use tier_ladder::{
    resolve_tier_ladder, ProviderTierLadder, ResolvedTier, ResolvedTierLadder, TierLadder,
    TierLadderResolutionIssue, TierLadderResolutionIssueKind, TierLadderSource,
};
