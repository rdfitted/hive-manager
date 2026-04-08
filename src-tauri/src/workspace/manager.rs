//! WorkspaceManager - Cell-based worktree lifecycle management.
//!
//! This module provides a higher-level manager that maps Cell + SessionMode
//! to worktree strategy, delegating to the existing WorktreeManager for
//! git operations.

use std::path::PathBuf;

use crate::domain::{Cell, CellType, Session, SessionMode, Workspace, WorkspaceStrategy};
use crate::runtime::WorktreeManager;

use super::git;

/// Error type for workspace operations.
#[derive(Debug, Clone)]
pub struct WorkspaceError {
    pub message: String,
}

impl WorkspaceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WorkspaceError {}

impl From<String> for WorkspaceError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for WorkspaceError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// Status information for a workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceStatus {
    /// Whether the workspace has uncommitted changes
    pub is_dirty: bool,
    /// Current branch name
    pub branch: String,
    /// Current HEAD commit
    pub head: String,
    /// Path to the worktree (if isolated)
    pub worktree_path: Option<PathBuf>,
}

/// Manager for cell-based workspace operations.
///
/// Provides methods for creating and managing workspaces based on
/// cell type and session mode, delegating git operations to
/// WorktreeManager.
///
/// # Workspace Rules
///
/// - Hive mode → `SharedCell` → one shared worktree per HiveCell
/// - Fusion mode → `IsolatedCell` → one worktree per candidate cell
/// - ResolverCell → `None` → no worktree (recommendation-only)
///
/// # Example
///
/// ```ignore
/// use hive_manager::workspace::WorkspaceManager;
/// use hive_manager::runtime::WorktreeManager;
///
/// let worktree_mgr = WorktreeManager::new("/project");
/// let workspace_mgr = WorkspaceManager::new(worktree_mgr);
///
/// // Create a workspace for a cell
/// let workspace = workspace_mgr.create_cell_workspace(&session, &cell)?;
///
/// // Inspect the workspace
/// let status = workspace_mgr.inspect_workspace(&workspace)?;
///
/// // Clean up when done
/// workspace_mgr.cleanup_cell_workspace(&workspace)?;
/// ```
pub struct WorkspaceManager {
    worktree_manager: WorktreeManager,
}

impl WorkspaceManager {
    /// Create a new WorkspaceManager with the given WorktreeManager.
    pub fn new(worktree_manager: WorktreeManager) -> Self {
        Self { worktree_manager }
    }

    /// Determine the workspace strategy for a cell.
    ///
    /// Returns `None` for Resolver cells (no workspace needed).
    pub fn determine_strategy(session: &Session, cell: &Cell) -> Option<WorkspaceStrategy> {
        match (&session.mode, &cell.cell_type) {
            (SessionMode::Hive, CellType::Hive) => Some(WorkspaceStrategy::SharedCell),
            (SessionMode::Fusion, CellType::Hive) => Some(WorkspaceStrategy::IsolatedCell),
            (SessionMode::Hive, CellType::Resolver) | (SessionMode::Fusion, CellType::Resolver) => {
                None
            }
        }
    }

    /// Create a workspace for a cell.
    ///
    /// For Resolver cells, this returns a workspace with `None` strategy
    /// and no worktree path.
    ///
    /// # Arguments
    ///
    /// * `session` - The session the cell belongs to
    /// * `cell` - The cell to create a workspace for
    ///
    /// # Returns
    ///
    /// A `Workspace` describing the created workspace, or an error.
    pub fn create_cell_workspace(
        &self,
        session: &Session,
        cell: &Cell,
    ) -> Result<Workspace, WorkspaceError> {
        let strategy = Self::determine_strategy(session, cell);

        // Get the base branch from the main repo
        let base_branch = self
            .worktree_manager
            .current_branch()
            .map_err(|e| WorkspaceError::new(e.message))?;

        // Generate the branch name for this cell
        let branch_name = git::generate_branch_name(
            &session.id,
            &cell.name,
            session.mode.clone(),
            cell.cell_type.clone(),
        );

        match strategy {
            None => {
                // Resolver cell - no worktree, just return a marker workspace
                Ok(Workspace {
                    strategy: WorkspaceStrategy::None,
                    repo_path: self.worktree_manager.project_path().to_path_buf(),
                    base_branch,
                    branch_name,
                    worktree_path: None,
                    is_dirty: false,
                })
            }
            Some(WorkspaceStrategy::SharedCell) => {
                // Shared worktree - multiple agents share the same directory
                let worktree_path = self
                    .worktree_manager
                    .worktree_base()
                    .join(&session.id)
                    .join(&cell.id);

                // Check if worktree already exists for this cell
                if worktree_path.exists() {
                    let current_branch = git::current_branch(&worktree_path)?;
                    if current_branch != branch_name {
                        return Err(WorkspaceError::new(format!(
                            "Existing shared worktree '{}' is on branch '{}' instead of '{}'",
                            worktree_path.display(),
                            current_branch,
                            branch_name
                        )));
                    }

                    // Reuse existing worktree
                    let is_dirty = git::is_dirty(&worktree_path)?;
                    return Ok(Workspace {
                        strategy: WorkspaceStrategy::SharedCell,
                        repo_path: self.worktree_manager.project_path().to_path_buf(),
                        base_branch,
                        branch_name,
                        worktree_path: Some(worktree_path),
                        is_dirty,
                    });
                }

                // Create the worktree
                let info = self
                    .worktree_manager
                    .create_worktree(&worktree_path, &branch_name)
                    .map_err(|e| WorkspaceError::new(e.message))?;

                Ok(Workspace {
                    strategy: WorkspaceStrategy::SharedCell,
                    repo_path: self.worktree_manager.project_path().to_path_buf(),
                    base_branch,
                    branch_name: info.branch,
                    worktree_path: Some(info.path),
                    is_dirty: false,
                })
            }
            Some(WorkspaceStrategy::IsolatedCell) => {
                // Isolated worktree - each cell gets its own directory
                let worktree_path = self
                    .worktree_manager
                    .worktree_base()
                    .join("isolated")
                    .join(&session.id)
                    .join(&cell.id);

                // Create the worktree
                let info = self
                    .worktree_manager
                    .create_worktree(&worktree_path, &branch_name)
                    .map_err(|e| WorkspaceError::new(e.message))?;

                Ok(Workspace {
                    strategy: WorkspaceStrategy::IsolatedCell,
                    repo_path: self.worktree_manager.project_path().to_path_buf(),
                    base_branch,
                    branch_name: info.branch,
                    worktree_path: Some(info.path),
                    is_dirty: false,
                })
            }
            Some(WorkspaceStrategy::None) => {
                // Should not happen, but handle gracefully
                Ok(Workspace {
                    strategy: WorkspaceStrategy::None,
                    repo_path: self.worktree_manager.project_path().to_path_buf(),
                    base_branch,
                    branch_name,
                    worktree_path: None,
                    is_dirty: false,
                })
            }
        }
    }

    /// Clean up a cell's workspace.
    ///
    /// For workspaces with worktrees, this removes the worktree.
    /// For `None` strategy workspaces, this is a no-op.
    pub fn cleanup_cell_workspace(&self, workspace: &Workspace) -> Result<(), WorkspaceError> {
        match (&workspace.strategy, &workspace.worktree_path) {
            (WorkspaceStrategy::None, _) => Ok(()),
            (_, None) => Ok(()),
            (_, Some(path)) => {
                // Force removal to handle dirty worktrees
                self.worktree_manager
                    .remove_worktree(path, true)
                    .map_err(|e| WorkspaceError::new(e.message))?;
                Ok(())
            }
        }
    }

    /// Clean up all workspaces for a session.
    ///
    /// This finds and removes all worktrees under the session's
    /// worktree base directory.
    pub fn cleanup_session_workspaces(&self, session_id: &str) -> Result<(), WorkspaceError> {
        // List all worktrees and filter by session
        let worktrees = self
            .worktree_manager
            .list_worktrees()
            .map_err(|e| WorkspaceError::new(e.message))?;

        let worktree_base = self.worktree_manager.worktree_base();

        // Remove worktrees that are in our base path and match the session
        for wt in worktrees {
            // Check if this worktree is under our worktree base
            if wt.path.starts_with(worktree_base) {
                // Check if the branch name contains the session ID
                // Branch format: hive/<session-id>/... or fusion/<session-id>/...
                if wt.branch.contains(&format!("/{}/", session_id))
                    || wt.branch.starts_with(&format!("resolver/{}", session_id))
                {
                    self.worktree_manager
                        .remove_worktree(&wt.path, true)
                        .map_err(|e| WorkspaceError::new(e.message))?;
                }
            }
        }

        // Prune stale references
        self.worktree_manager
            .prune_worktrees()
            .map_err(|e| WorkspaceError::new(e.message))?;

        Ok(())
    }

    /// Inspect a workspace and return its current status.
    ///
    /// Returns information about the workspace including:
    /// - Whether it has uncommitted changes
    /// - Current branch
    /// - Current HEAD commit
    pub fn inspect_workspace(
        &self,
        workspace: &Workspace,
    ) -> Result<WorkspaceStatus, WorkspaceError> {
        match (&workspace.strategy, &workspace.worktree_path) {
            (WorkspaceStrategy::None, _) => {
                Ok(WorkspaceStatus {
                    is_dirty: false,
                    branch: String::new(),
                    head: String::new(),
                    worktree_path: None,
                })
            }
            (_, None) => {
                // Workspace without a worktree - check main repo
                let is_dirty = git::is_dirty(&workspace.repo_path)?;
                let branch = git::current_branch(&workspace.repo_path)?;
                let head = git::current_head(&workspace.repo_path)?;

                Ok(WorkspaceStatus {
                    is_dirty,
                    branch,
                    head,
                    worktree_path: None,
                })
            }
            (_, Some(path)) => {
                let is_dirty = git::is_dirty(path)?;
                let branch = git::current_branch(path)?;
                let head = git::current_head(path)?;

                Ok(WorkspaceStatus {
                    is_dirty,
                    branch,
                    head,
                    worktree_path: Some(path.clone()),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_session(mode: SessionMode) -> Session {
        use chrono::Utc;
        use crate::domain::{LaunchConfig, SessionStatus};

        Session {
            id: "test-session-123".to_string(),
            name: "Test Session".to_string(),
            objective: "Test objective".to_string(),
            project_path: PathBuf::from("/test/project"),
            mode,
            status: SessionStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            cells: vec![],
            launch_config: LaunchConfig {
                plan_source: None,
                default_cli: "claude".to_string(),
                default_model: None,
                worker_count: 1,
                variant_count: None,
                with_planning: false,
                with_evaluator: false,
                smoke_test: false,
            },
            artifacts: vec![],
            events: vec![],
        }
    }

    fn make_test_cell(cell_type: CellType, name: &str) -> Cell {
        Cell {
            id: format!("cell-{}", name),
            session_id: "test-session-123".to_string(),
            cell_type,
            name: name.to_string(),
            status: crate::domain::CellStatus::Running,
            objective: "Test cell objective".to_string(),
            workspace: crate::domain::Workspace {
                strategy: WorkspaceStrategy::SharedCell,
                repo_path: PathBuf::from("/test/project"),
                base_branch: "main".to_string(),
                branch_name: "test-branch".to_string(),
                worktree_path: None,
                is_dirty: false,
            },
            agents: vec![],
            artifacts: None,
            events: vec![],
            depends_on: vec![],
        }
    }

    #[test]
    fn test_determine_strategy_hive() {
        let session = make_test_session(SessionMode::Hive);
        let cell = make_test_cell(CellType::Hive, "worker-1");

        let strategy = WorkspaceManager::determine_strategy(&session, &cell);
        assert_eq!(strategy, Some(WorkspaceStrategy::SharedCell));
    }

    #[test]
    fn test_determine_strategy_fusion() {
        let session = make_test_session(SessionMode::Fusion);
        let cell = make_test_cell(CellType::Hive, "candidate-a");

        let strategy = WorkspaceManager::determine_strategy(&session, &cell);
        assert_eq!(strategy, Some(WorkspaceStrategy::IsolatedCell));
    }

    #[test]
    fn test_determine_strategy_resolver() {
        let session = make_test_session(SessionMode::Hive);
        let cell = make_test_cell(CellType::Resolver, "resolver");

        let strategy = WorkspaceManager::determine_strategy(&session, &cell);
        assert_eq!(strategy, None);
    }

    #[test]
    fn test_workspace_error() {
        let err = WorkspaceError::new("Test error");
        assert_eq!(err.message, "Test error");
        assert_eq!(format!("{}", err), "Test error");
    }
}
