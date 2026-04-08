//! Workspace management module.
//!
//! This module provides cell-based workspace management, mapping
//! Cell + SessionMode to appropriate worktree strategies.
//!
//! # Architecture
//!
//! - [`manager`] - `WorkspaceManager` for high-level cell-based operations
//! - [`git`] - Git-specific helpers (branch naming, dirty state)
//!
//! # Workspace Rules
//!
//! | Session Mode | Cell Type    | Strategy       | Description                          |
//! |-------------|--------------|----------------|--------------------------------------|
//! | Hive        | HiveCell     | SharedCell     | One shared worktree per HiveCell     |
//! | Fusion      | Candidate    | IsolatedCell   | One worktree per candidate cell      |
//! | Any         | ResolverCell | None           | No worktree (recommendation-only)    |
//!
//! # Branch Naming
//!
//! - Hive: `hive/<session-id>/<cell-name>`
//! - Fusion candidate: `fusion/<session-id>/<candidate-name>`
//! - Resolver: `resolver/<session-id>`

pub mod git;
pub mod manager;

pub use manager::{WorkspaceError, WorkspaceManager, WorkspaceStatus};
