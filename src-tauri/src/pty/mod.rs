mod manager;
pub mod output_ring;
#[cfg(not(test))]
mod session;
#[cfg(test)]
#[path = "session_stub.rs"]
mod session;
#[cfg(test)]
#[path = "session.rs"]
#[allow(dead_code)]
mod real_session;

pub use manager::{PtyManager, PtySnapshot};
pub use session::{AgentConfig, AgentRole, AgentStatus, RoleDefinitionRef, WorkerRole};
