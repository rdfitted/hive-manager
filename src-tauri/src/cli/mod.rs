// CLI registry module - infrastructure for future CLI management features
pub mod agent_store;
pub mod health;
mod registry;

pub use registry::{CliBehavior, CliRegistry};
