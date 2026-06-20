//! Unified Action contract.
//!
//! An [`Action`] is one unit of work, addressable by a stable dotted name
//! (`session.list`, `git.pull`, ...) and dispatched uniformly over
//! `serde_json::Value`. The SAME action is invoked from every surface:
//!
//! - Tauri `#[command]` wrappers dispatch with [`Caller::Frontend`];
//! - the Axum HTTP layer dispatches with [`Caller::Http`] (the generic
//!   `POST /api/actions/{name}` entrypoint is the future agent/MCP surface).
//!
//! Validation runs ONCE, in [`ActionRegistry::dispatch`], before `run`
//! regardless of caller. Each action exports a JSON Schema (via `schemars`) so
//! the registry is surfaceable as agent/MCP tools at `GET /api/actions`.
//!
//! Action outputs are intentionally plain `serde_json::Value` so a future
//! `{ renderer?, data }` result envelope (#127) can wrap them without changing
//! this contract.

pub mod context;
pub mod coordination;
pub mod error;
pub mod git;
pub mod pty;
pub mod registry;
pub mod render;
pub mod session;

#[cfg(test)]
mod tests;

pub use context::{ActionContext, Caller};
pub use error::{ActionError, ActionStatus};
pub use registry::{build_registry, Action, ActionRegistry};
