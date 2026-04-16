//! Git-specific helpers for workspace management.
//!
//! Provides branch naming conventions and dirty state detection
//! for cell-based worktree operations.

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

use crate::domain::{CellType, SessionMode};
use crate::runtime::WorktreeManager;
use crate::session::{Session, SessionType};

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

/// Fetch the latest state of a branch from origin.
/// Returns Ok(()) on success, Err on failure (e.g. no remote, network issues).
pub fn fetch_origin_branch(project_path: &Path, branch: &str) -> Result<(), String> {
    run_git(project_path, &["fetch", "origin", branch])
        .map(|_| ())
}

/// Determine the best base ref for creating a new worktree.
/// Tries to fetch origin and use `origin/<default>`, falling back to `"HEAD"`
/// if there is no remote or the fetch fails. Emits a tracing warning on
/// fallback so operators can see when fresh-base resolution has degraded.
///
/// Does NOT mutate local branch refs — worktrees branch directly from the
/// remote tracking ref, so the local `main` pointer is left untouched to avoid
/// corrupting the main checkout or orphaning local-only commits.
pub fn resolve_fresh_base(project_path: &Path) -> String {
    let main_branch = detect_main_branch(project_path);

    // Try to fetch the latest from origin and use the remote tracking ref
    // directly as the base. No local ref mutation needed — `git worktree add`
    // accepts remote tracking branches as the base.
    if fetch_origin_branch(project_path, &main_branch).is_ok() {
        let remote_ref = format!("origin/{}", main_branch);
        if run_git(
            project_path,
            &["rev-parse", "--verify", &format!("refs/remotes/{}", remote_ref)],
        )
        .is_ok()
        {
            return remote_ref;
        }
    }

    // Fallback: use whatever local HEAD points at. This can reintroduce the
    // stale-base problem silently, so warn loudly.
    tracing::warn!(
        project_path = %project_path.display(),
        main_branch = %main_branch,
        "resolve_fresh_base: falling back to local HEAD — fetch failed or no remote. \
         Worktrees may branch from stale state."
    );
    "HEAD".to_string()
}

/// Detect the main branch name. Prefers the remote default (via
/// `git symbolic-ref refs/remotes/origin/HEAD`) over local heuristics so that
/// repos with non-standard defaults (e.g. `develop`, `trunk`) are handled
/// correctly. Falls back to local `main` / `master`, then to `"main"`.
fn detect_main_branch(project_path: &Path) -> String {
    // 1. Preferred: ask git what the remote default is.
    if let Ok(output) = run_git(project_path, &["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        let trimmed = output.trim();
        if let Some(name) = trimmed.strip_prefix("refs/remotes/origin/") {
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }

    // 2. Fallback: check local branches.
    if branch_exists(project_path, "main").unwrap_or(false) {
        return "main".to_string();
    }
    if branch_exists(project_path, "master").unwrap_or(false) {
        return "master".to_string();
    }

    // 3. Last resort.
    "main".to_string()
}

pub fn create_session_worktree(
    session_id: &str,
    cell_id: &str,
    branch: &str,
    base_branch: &str,
    project_path: &Path,
) -> Result<(PathBuf, String), String> {
    let worktree_path = project_path
        .join(".hive-manager")
        .join("worktrees")
        .join(session_id)
        .join(cell_id);

    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create worktree parent dir: {}", e))?;
    }

    let manager = WorktreeManager::new(project_path);
    manager
        .prune_worktrees()
        .map_err(|err| format!("worktree prune: {}", err.message))?;

    let worktree_str = worktree_path.to_string_lossy().to_string();
    if branch_exists(project_path, branch)? {
        run_git(project_path, &["worktree", "add", &worktree_str, branch])?;
    } else {
        run_git(
            project_path,
            &["worktree", "add", &worktree_str, "-b", branch, base_branch],
        )?;
    }

    Ok((worktree_path, worktree_str))
}

/// Remove a single session worktree under `.hive-manager/worktrees/{session}/{cell_id}`.
/// Used when PTY spawn fails after `create_session_worktree` so branches/worktrees are not left behind.
pub fn remove_session_worktree_cell(
    project_path: &Path,
    session_id: &str,
    cell_id: &str,
) -> Result<(), String> {
    let worktree_path = project_path
        .join(".hive-manager")
        .join("worktrees")
        .join(session_id)
        .join(cell_id);
    let manager = WorktreeManager::new(project_path);
    let _ = manager.prune_worktrees();
    if !worktree_path.exists() {
        return Ok(());
    }

    if let Err(err) = manager.remove_worktree(&worktree_path, true) {
        if !is_missing_worktree_error(&err.message) {
            return Err(err.message);
        }
    }
    let _ = manager.prune_worktrees();
    Ok(())
}

pub fn cleanup_session_worktrees(session: &Session) -> Result<(), String> {
    let manager = WorktreeManager::new(&session.project_path);
    let worktrees = manager
        .list_worktrees()
        .map_err(|e| format!("worktree list: {}", e.message))?;

    let session_prefixes = match &session.session_type {
        SessionType::Fusion { .. } => vec![session.project_path.join(".hive-fusion").join(&session.id)],
        _ => vec![session
            .project_path
            .join(".hive-manager")
            .join("worktrees")
            .join(&session.id)],
    };

    let mut cleanup_errors = Vec::new();
    for worktree in worktrees {
        if !session_prefixes.iter().any(|prefix| worktree.path.starts_with(prefix)) {
            continue;
        }

        if let Err(err) = manager.remove_worktree(&worktree.path, true) {
            if is_missing_worktree_error(&err.message) {
                tracing::debug!(
                    "Ignoring missing worktree during cleanup: {} ({})",
                    worktree.path.display(),
                    err.message
                );
            } else {
                cleanup_errors.push(format!("{}: {}", worktree.path.display(), err.message));
            }
        }
    }

    if let Err(err) = manager.prune_worktrees() {
        cleanup_errors.push(format!("worktree prune: {}", err.message));
    }

    if cleanup_errors.is_empty() {
        Ok(())
    } else {
        Err(cleanup_errors.join(" | "))
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

fn is_missing_worktree_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("is not a working tree")
        || lower.contains("is not a git repository")
        || lower.contains("could not remove reference")
        || lower.contains("no such file or directory")
        || lower.contains("cannot find the path specified")
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
