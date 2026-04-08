//! Git Worktree management for isolated agent workspaces.
//!
//! This module provides helpers for creating and managing git worktrees,
//! allowing agents to work in isolated directories while sharing the same
//! repository.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Error type for worktree operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeError {
    /// Error message
    pub message: String,
}

impl WorktreeError {
    /// Create a new worktree error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WorktreeError {}

impl From<String> for WorktreeError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for WorktreeError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// Information about a git worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeInfo {
    /// Path to the worktree directory
    pub path: PathBuf,
    /// Branch name (or "detached" for detached HEAD)
    pub branch: String,
    /// Current HEAD commit hash
    pub head: String,
    /// Whether this is a bare worktree
    pub is_bare: bool,
}

/// Manager for git worktree operations.
///
/// Provides methods for creating, listing, and removing worktrees
/// to support isolated agent workspaces.
///
/// # Example
///
/// ```ignore
/// use hive_manager::runtime::WorktreeManager;
///
/// let manager = WorktreeManager::new("/project");
///
/// // Create a worktree for an agent
/// let info = manager.create_worktree("agent-1-feature", "feature/agent-1-work")?;
///
/// // Later, clean up
/// manager.remove_worktree(&info.path)?;
/// ```
pub struct WorktreeManager {
    /// Root project path (main repository)
    project_path: PathBuf,
    /// Base directory for generated agent worktrees.
    worktree_base: PathBuf,
}

impl WorktreeManager {
    /// Create a new WorktreeManager for the given project.
    pub fn new(project_path: impl Into<PathBuf>) -> Self {
        let project_path = project_path.into();
        let worktree_base = project_path.join(".hive-manager").join("worktrees");
        Self::with_worktree_base(project_path, worktree_base)
    }

    /// Create a new WorktreeManager with an explicit worktree base path.
    pub fn with_worktree_base(
        project_path: impl Into<PathBuf>,
        worktree_base: impl Into<PathBuf>,
    ) -> Self {
        Self {
            project_path: project_path.into(),
            worktree_base: worktree_base.into(),
        }
    }

    /// Run a git command in the project directory.
    fn run_git(&self, args: &[&str]) -> Result<String, WorktreeError> {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(&self.project_path);

        // Prevent console window on Windows
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let output = cmd
            .output()
            .map_err(|e| WorktreeError::new(format!("Failed to run git: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let message = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                "Git command failed".to_string()
            };
            return Err(WorktreeError::new(message));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Create a new worktree.
    ///
    /// # Arguments
    ///
    /// * `worktree_path` - Path where the worktree should be created
    /// * `branch` - Branch name for the worktree (will be created if doesn't exist)
    ///
    /// # Returns
    ///
    /// Information about the created worktree.
    pub fn create_worktree(
        &self,
        worktree_path: impl AsRef<Path>,
        branch: &str,
    ) -> Result<WorktreeInfo, WorktreeError> {
        let worktree_path = worktree_path.as_ref();

        // Create the worktree
        self.run_git(&[
            "worktree",
            "add",
            &worktree_path.to_string_lossy(),
            "-b",
            branch,
        ])?;

        // Get info about the created worktree
        let worktrees = self.list_worktrees()?;
        worktrees
            .into_iter()
            .find(|w| w.path == worktree_path)
            .ok_or_else(|| WorktreeError::new("Worktree was created but not found in list"))
    }

    /// Create a worktree at a generated path based on agent ID.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - Unique agent identifier
    /// * `branch_prefix` - Prefix for the branch name
    ///
    /// # Returns
    ///
    /// Information about the created worktree.
    pub fn create_agent_worktree(
        &self,
        agent_id: &str,
        branch_prefix: &str,
    ) -> Result<WorktreeInfo, WorktreeError> {
        let worktree_name = format!("worktree-{}", agent_id);
        std::fs::create_dir_all(&self.worktree_base).map_err(|e| {
            WorktreeError::new(format!(
                "Failed to create worktree base directory '{}': {}",
                self.worktree_base.display(),
                e
            ))
        })?;
        let worktree_path = self.worktree_base.join(&worktree_name);

        // Generate branch name
        let branch = format!("{}/{}", branch_prefix, agent_id);

        self.create_worktree(&worktree_path, &branch)
    }

    /// List all worktrees in the repository.
    pub fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, WorktreeError> {
        let output = self.run_git(&["worktree", "list", "--porcelain"])?;
        parse_worktree_list(&output)
    }

    /// Remove a worktree.
    ///
    /// # Arguments
    ///
    /// * `worktree_path` - Path to the worktree to remove
    /// * `force` - Force removal even if there are uncommitted changes
    pub fn remove_worktree(
        &self,
        worktree_path: impl AsRef<Path>,
        force: bool,
    ) -> Result<(), WorktreeError> {
        let worktree_path = worktree_path.as_ref();
        let path_str = worktree_path.to_string_lossy();

        let mut args = vec!["worktree", "remove", &path_str];
        if force {
            args.push("--force");
        }

        match self.run_git(&args) {
            Ok(_) => Ok(()),
            Err(e) => {
                // Check for Windows-specific "in use" error
                #[cfg(windows)]
                {
                    let lower = e.message.to_lowercase();
                    if lower.contains("in use")
                        || lower.contains("being used")
                        || lower.contains("permission denied")
                        || lower.contains("access is denied")
                    {
                        return Err(WorktreeError::new(format!(
                            "Failed to remove worktree because files may still be open. \
                             Close terminals/editors using '{}' and retry. Git error: {}",
                            worktree_path.display(),
                            e.message
                        )));
                    }
                }
                Err(e)
            }
        }
    }

    /// Prune stale worktree references.
    ///
    /// This cleans up worktree entries for directories that no longer exist.
    pub fn prune_worktrees(&self) -> Result<(), WorktreeError> {
        self.run_git(&["worktree", "prune"])?;
        Ok(())
    }

    /// Get the current branch of the main repository.
    pub fn current_branch(&self) -> Result<String, WorktreeError> {
        let output = self.run_git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
        let branch = output.trim();
        if branch.is_empty() {
            return Err(WorktreeError::new("Unable to determine current branch"));
        }
        Ok(branch.to_string())
    }

    /// Check if the working directory is clean (no uncommitted changes).
    pub fn is_clean(&self) -> Result<bool, WorktreeError> {
        let output = self.run_git(&["status", "--porcelain"])?;
        Ok(output.trim().is_empty())
    }

    /// Get the project path.
    pub fn project_path(&self) -> &Path {
        &self.project_path
    }

    /// Get the configured worktree base path.
    pub fn worktree_base(&self) -> &Path {
        &self.worktree_base
    }
}

/// Parse the output of `git worktree list --porcelain`.
fn parse_worktree_list(output: &str) -> Result<Vec<WorktreeInfo>, WorktreeError> {
    let mut worktrees = Vec::new();
    let mut current: Option<WorktreeInfo> = None;

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            if let Some(info) = current.take() {
                if info.path.as_os_str().is_empty() {
                    return Err(WorktreeError::new(
                        "Unexpected git worktree output: missing path",
                    ));
                }
                worktrees.push(info);
            }
            continue;
        }

        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(info) = current.take() {
                if info.path.as_os_str().is_empty() {
                    return Err(WorktreeError::new(
                        "Unexpected git worktree output: missing path",
                    ));
                }
                worktrees.push(info);
            }

            current = Some(WorktreeInfo {
                path: PathBuf::from(path),
                branch: String::new(),
                head: String::new(),
                is_bare: false,
            });
            continue;
        }

        let entry = current
            .as_mut()
            .ok_or_else(|| WorktreeError::new(format!("Unexpected git worktree output: {}", line)))?;

        if let Some(head) = line.strip_prefix("HEAD ") {
            entry.head = head.to_string();
        } else if let Some(branch) = line.strip_prefix("branch ") {
            entry.branch = branch
                .strip_prefix("refs/heads/")
                .unwrap_or(branch)
                .to_string();
        } else if line == "bare" {
            entry.is_bare = true;
        } else if line == "detached" && entry.branch.is_empty() {
            entry.branch = "detached".to_string();
        }
    }

    if let Some(info) = current.take() {
        if info.path.as_os_str().is_empty() {
            return Err(WorktreeError::new(
                "Unexpected git worktree output: missing path",
            ));
        }
        worktrees.push(info);
    }

    Ok(worktrees)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree_error() {
        let err = WorktreeError::new("Test error");
        assert_eq!(err.message, "Test error");
        assert_eq!(format!("{}", err), "Test error");
    }

    #[test]
    fn test_worktree_error_from_string() {
        let err: WorktreeError = "String error".into();
        assert_eq!(err.message, "String error");
    }

    #[test]
    fn test_parse_worktree_list_empty() {
        let result = parse_worktree_list("");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
    }

    #[test]
    fn test_parse_worktree_list_single() {
        let output = r#"worktree /path/to/main
HEAD abc123
branch refs/heads/main

"#;

        let result = parse_worktree_list(output);
        assert!(result.is_ok());

        let worktrees = result.unwrap();
        assert_eq!(worktrees.len(), 1);

        let wt = &worktrees[0];
        assert_eq!(wt.path, PathBuf::from("/path/to/main"));
        assert_eq!(wt.head, "abc123");
        assert_eq!(wt.branch, "main");
        assert!(!wt.is_bare);
    }

    #[test]
    fn test_parse_worktree_list_multiple() {
        let output = r#"worktree /path/to/main
HEAD abc123
branch refs/heads/main

worktree /path/to/feature
HEAD def456
branch refs/heads/feature-branch

"#;

        let result = parse_worktree_list(output);
        assert!(result.is_ok());

        let worktrees = result.unwrap();
        assert_eq!(worktrees.len(), 2);

        assert_eq!(worktrees[0].branch, "main");
        assert_eq!(worktrees[1].branch, "feature-branch");
    }

    #[test]
    fn test_parse_worktree_list_detached() {
        let output = r#"worktree /path/to/detached
HEAD abc123
detached

"#;

        let result = parse_worktree_list(output);
        assert!(result.is_ok());

        let worktrees = result.unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch, "detached");
    }

    #[test]
    fn test_parse_worktree_list_bare() {
        let output = r#"worktree /path/to/bare
HEAD abc123
bare

"#;

        let result = parse_worktree_list(output);
        assert!(result.is_ok());

        let worktrees = result.unwrap();
        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].is_bare);
    }

    #[test]
    fn test_worktree_manager_creation() {
        let manager = WorktreeManager::new("/project");
        assert_eq!(manager.project_path(), Path::new("/project"));
        assert_eq!(
            manager.worktree_base(),
            Path::new("/project/.hive-manager/worktrees")
        );
    }
}
