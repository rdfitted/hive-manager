//! Execution context threaded into every action.

use std::sync::Arc;

use serde::Serialize;

use crate::http::state::AppState;

/// Who is invoking an action. Exposed on [`ActionContext`] so an action's
/// `run` can branch on provenance (e.g. tighten behavior for `Http`/`Agent`
/// callers while keeping `Frontend` ergonomic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Caller {
    /// Dispatched from a Tauri `#[command]` (the Svelte UI via `invoke`).
    Frontend,
    /// Dispatched by the in-process agent loop.
    Agent,
    /// Dispatched by a CLI entrypoint.
    Cli,
    /// Dispatched over the HTTP API.
    Http,
}

/// Everything an [`Action`](crate::actions::Action) needs to run: the calling
/// surface plus the single shared [`AppState`].
///
/// The same `Arc<AppState>` is shared by BOTH the Tauri `.manage()`d registry
/// and the HTTP server, so an action sees identical state regardless of caller.
#[derive(Clone)]
pub struct ActionContext {
    pub caller: Caller,
    pub state: Arc<AppState>,
}

impl ActionContext {
    pub fn new(caller: Caller, state: Arc<AppState>) -> Self {
        Self { caller, state }
    }
}
