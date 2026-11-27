mod manager;
#[cfg(windows)]
mod conpty_windows;
pub mod output_ring;
pub mod screen;
pub mod trust_prompt;
#[cfg(not(test))]
mod session;
#[cfg(test)]
#[path = "session_stub.rs"]
mod session;
#[cfg(test)]
#[path = "session.rs"]
#[allow(dead_code)]
mod real_session;

pub use manager::{manager_trust, PtyManager, PtySnapshot};
pub use session::{AgentConfig, AgentRole, AgentStatus, RoleDefinitionRef, WorkerRole};
