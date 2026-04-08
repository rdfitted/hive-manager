use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Workspace {
    pub strategy: WorkspaceStrategy,
    pub repo_path: PathBuf,
    pub base_branch: String,
    pub branch_name: String,
    pub worktree_path: Option<PathBuf>,
    pub is_dirty: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStrategy {
    /// Shared worktree for multiple agents in a HiveCell
    SharedCell,
    /// Isolated worktree for a single cell (Fusion candidates)
    IsolatedCell,
    /// No worktree needed (ResolverCell - recommendation-only)
    None,
}
