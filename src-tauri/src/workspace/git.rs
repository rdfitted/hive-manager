//! Git-specific helpers for workspace management.
//!
//! Provides branch naming conventions and dirty state detection
//! for cell-based worktree operations.

use std::path::Path;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

use crate::domain::{CellType, SessionMode};

/// Generate a branch name for a cell based on session mode and cell type.
///
/// # Naming Conventions
///
/// - Hive mode: `hive/<session-id>/<cell-name>`
/// - Fusion candidate: `fusion/<session-id>/<candidate-name>`
/// - Resolver: `resolver/<session-id>`
pub fn generate_branch_name(
    session_id: &str,
    cell_name: &str,
    session_mode: SessionMode,
    cell_type: CellType,
) -> String {
    match (&session_mode, &cell_type) {
        (SessionMode::Hive, CellType::Hive) => {
            format!("hive/{}/{}", session_id, cell_name)
        }
        (SessionMode::Fusion, CellType::Hive) => {
            // Fusion candidates are isolated cells
            format!("fusion/{}/{}", session_id, cell_name)
        }
        (SessionMode::Hive, CellType::Resolver) | (SessionMode::Fusion, CellType::Resolver) => {
            format!("resolver/{}", session_id)
        }
    }
}

/// Check if a working directory has uncommitted changes.
///
/// Returns `true` if the directory is dirty (has uncommitted changes),
/// `false` if clean, or an error string if the check failed.
pub fn is_dirty(worktree_path: &Path) -> Result<bool, String> {
    let output = run_git(worktree_path, &["status", "--porcelain"])?;

    Ok(!output.trim().is_empty())
}

/// Get the current branch name from a working directory.
///
/// Returns the branch name, or "detached" if in detached HEAD state.
pub fn current_branch(worktree_path: &Path) -> Result<String, String> {
    let output = run_git(worktree_path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = output.trim();

    if branch.is_empty() || branch == "HEAD" {
        // Check if we're actually in detached HEAD state
        let ref_output = run_git(worktree_path, &["symbolic-ref", "-q", "HEAD"])?;
        if ref_output.trim().is_empty() {
            Ok("detached".to_string())
        } else {
            Ok(branch.to_string())
        }
    } else {
        Ok(branch.to_string())
    }
}

/// Get the current HEAD commit hash.
pub fn current_head(worktree_path: &Path) -> Result<String, String> {
    let output = run_git(worktree_path, &["rev-parse", "HEAD"])?;
    Ok(output.trim().to_string())
}

/// Check if a branch exists locally.
pub fn branch_exists(worktree_path: &Path, branch_name: &str) -> Result<bool, String> {
    match run_git(
        worktree_path,
        &["rev-parse", "--verify", &format!("refs/heads/{}", branch_name)],
    ) {
        Ok(output) => Ok(!output.trim().is_empty()),
        Err(_) => Ok(false),
    }
}

/// Run a git command in the specified directory.
fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(cwd);

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;

    if !output.status.success() {
        // Some git commands fail with specific meanings we can detect
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // For verification commands, empty output usually means "doesn't exist"
        if args.iter().any(|a| *a == "--verify") && stderr.is_empty() {
            return Ok(String::new());
        }
        return Err(if !stderr.is_empty() {
            stderr
        } else {
            "Git command failed".to_string()
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_branch_name_hive() {
        let branch = generate_branch_name(
            "session-123",
            "worker-1",
            SessionMode::Hive,
            CellType::Hive,
        );
        assert_eq!(branch, "hive/session-123/worker-1");
    }

    #[test]
    fn test_generate_branch_name_fusion_candidate() {
        let branch = generate_branch_name(
            "session-456",
            "candidate-a",
            SessionMode::Fusion,
            CellType::Hive,
        );
        assert_eq!(branch, "fusion/session-456/candidate-a");
    }

    #[test]
    fn test_generate_branch_name_resolver() {
        let branch = generate_branch_name(
            "session-789",
            "resolver",
            SessionMode::Hive,
            CellType::Resolver,
        );
        assert_eq!(branch, "resolver/session-789");
    }

    #[test]
    fn test_generate_branch_name_fusion_resolver() {
        let branch = generate_branch_name(
            "session-abc",
            "judge",
            SessionMode::Fusion,
            CellType::Resolver,
        );
        assert_eq!(branch, "resolver/session-abc");
    }
}
