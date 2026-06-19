mod manager;
#[cfg(not(all(test, windows)))]
mod session;
#[cfg(all(test, windows))]
#[path = "session_stub.rs"]
mod session;

pub use manager::PtyManager;
pub use session::{AgentConfig, AgentRole, AgentStatus, WorkerRole};
