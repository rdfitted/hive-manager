// CLI registry module - infrastructure for future CLI management features
pub mod health;
mod registry;

pub use registry::{CliBehavior, CliRegistry};
