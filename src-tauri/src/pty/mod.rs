mod manager;
pub mod output_ring;
#[cfg(not(all(test, windows)))]
mod session;
#[cfg(all(test, windows))]
#[path = "session_stub.rs"]
mod session;
#[cfg(all(test, windows))]
#[path = "session.rs"]
#[allow(dead_code)]
mod real_session;

pub use manager::{PtyManager, PtySnapshot};
pub use session::{AgentConfig, AgentRole, AgentStatus, RoleDefinitionRef, WorkerRole};
