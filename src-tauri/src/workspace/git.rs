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
/// - Debate debater: `debate/<session-id>/<debater-name>`
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
        (SessionMode::Debate, CellType::Hive) => {
            // Debate debaters are isolated cells
            format!("debate/{}/{}", session_id, cell_name)
        }
        (SessionMode::Hive, CellType::Resolver)
        | (SessionMode::Fusion, CellType::Resolver)
        | (SessionMode::Debate, CellType::Resolver) => {
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
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{}", branch_name),
        ],
    ) {
        Ok(output) => Ok(!output.trim().is_empty()),
        Err(_) => Ok(false),
    }
}

/// Fetch the latest state of a branch from origin.
/// Returns Ok(()) on success, Err on failure (e.g. no remote, network issues).
pub fn fetch_origin_branch(project_path: &Path, branch: &str) -> Result<(), String> {
    run_git(project_path, &["fetch", "origin", branch]).map(|_| ())
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
    let remote_ref = format!("origin/{}", main_branch);
    let failure_cause: String = match fetch_origin_branch(project_path, &main_branch) {
        Ok(()) => {
            match run_git(
                project_path,
                &[
                    "rev-parse",
                    "--verify",
                    &format!("refs/remotes/{}", remote_ref),
                ],
            ) {
                Ok(_) => return remote_ref,
                Err(err) => format!("fetched but remote tracking ref verify failed: {}", err),
            }
        }
        Err(err) => format!("fetch failed: {}", err),
    };

    // Fallback: use whatever local HEAD points at. This can reintroduce the
    // stale-base problem silently, so warn loudly with the concrete cause so
    // operators can distinguish offline vs auth vs missing-branch.
    tracing::warn!(
        project_path = %project_path.display(),
        main_branch = %main_branch,
        cause = %failure_cause,
        "resolve_fresh_base: falling back to local HEAD. Worktrees may branch from stale state."
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

/// Hive-owned directories that may sit directly under a project root and hold
/// generated worktrees. Only these are eligible for a lint-config boundary — see
/// [`worktree_config_boundary_root`] for why the whitelist is load-bearing.
const HIVE_WORKTREE_ROOTS: &[&str] = &[".hive-manager", ".hive-fusion", ".hive-debate"];

/// Legacy (`.eslintrc.*`) config filenames. Flat config (`eslint.config.js`) does
/// not cascade and is deliberately absent — see the module note on #177.
const ESLINTRC_NAMES: &[&str] = &[
    ".eslintrc",
    ".eslintrc.json",
    ".eslintrc.js",
    ".eslintrc.cjs",
    ".eslintrc.yml",
    ".eslintrc.yaml",
];

/// The hive-owned directory directly under `project_path` that contains
/// `worktree_path`, or `None` when the worktree is not under one.
///
/// The [`HIVE_WORKTREE_ROOTS`] whitelist — rather than "just take the first
/// component" — is load-bearing. `actions/git/mod.rs::git.worktree_add` accepts a
/// caller-supplied worktree path with no containment check, so a generic
/// derivation could write a `root: true` config into an arbitrary *tracked*
/// source directory of the operator's repo and silently disable every lint rule
/// beneath it.
pub(crate) fn worktree_config_boundary_root(
    project_path: &Path,
    worktree_path: &Path,
) -> Option<PathBuf> {
    let relative = worktree_path.strip_prefix(project_path).ok()?;
    let first = relative.components().next()?;
    let name = first.as_os_str().to_str()?;
    if HIVE_WORKTREE_ROOTS.contains(&name) {
        Some(project_path.join(name))
    } else {
        None
    }
}

/// True iff this worktree can lint itself — it carries an eslintrc-family file,
/// or a `package.json` with an `eslintConfig` key.
///
/// Gating on this matters: writing a boundary above a worktree that has no config
/// of its own would turn a loud `exit 2` into a silent **false green** (ESLint with
/// an empty ruleset exits 0 with no output), which is strictly worse for a
/// QA-gated pipeline.
fn worktree_has_eslint_config(worktree_path: &Path) -> bool {
    if ESLINTRC_NAMES
        .iter()
        .any(|name| worktree_path.join(name).exists())
    {
        return true;
    }
    std::fs::read_to_string(worktree_path.join("package.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .is_some_and(|pkg| pkg.get("eslintConfig").is_some())
}

/// Write an inert `{"root": true}` ESLint boundary into the hive-owned directory
/// above a generated worktree (#177).
///
/// Worktrees live *inside* the parent repository, so ESLint 8's `.eslintrc`
/// cascade walks up out of the worktree, finds the parent repo's config, and
/// resolves the same plugin from two `node_modules` trees — failing with
/// "couldn't determine the plugin ... uniquely" (exit 2) before linting a single
/// file. A `root: true` config in an ancestor halts that upward walk.
///
/// Verified against ESLint 8.57.1: no boundary => exit 2; a `{}` boundary => still
/// exit 2 (an empty object does **not** stop the cascade); `{"root": true}` => exit
/// 0 with the worktree's own config and plugins still in force; and the parent
/// repo's own files are unaffected (the cascade only ever walks upward).
///
/// Best effort by design: never returns an error and never fails a launch.
pub(crate) fn ensure_worktree_config_boundary(project_path: &Path, worktree_path: &Path) {
    let Some(root) = worktree_config_boundary_root(project_path, worktree_path) else {
        // Fusion/debate paths round-trip through `to_string_lossy()` -> `PathBuf`,
        // so a lost prefix match should be diagnosable rather than silent.
        tracing::warn!(
            project_path = %project_path.display(),
            worktree_path = %worktree_path.display(),
            "no hive-owned boundary root for worktree; skipping lint boundary"
        );
        return;
    };

    if !worktree_has_eslint_config(worktree_path) {
        tracing::debug!(
            worktree_path = %worktree_path.display(),
            "worktree has no eslint config; not writing a lint boundary"
        );
        return;
    }

    let boundary = root.join(".eslintrc.json");
    if boundary.exists() {
        tracing::debug!(path = %boundary.display(), "lint boundary already present");
        return;
    }

    if let Err(e) = std::fs::create_dir_all(&root) {
        tracing::warn!(path = %root.display(), error = %e, "failed to create boundary dir");
        return;
    }
    if let Err(e) = std::fs::write(&boundary, "{\"root\": true}\n") {
        tracing::warn!(path = %boundary.display(), error = %e, "failed to write lint boundary");
    }
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

    // #177: must run AFTER `worktree add` — the gate inspects the checked-out tree.
    ensure_worktree_config_boundary(project_path, &worktree_path);

    let task_dir = worktree_path.join(".hive-manager").join("tasks");
    if let Err(e) = std::fs::create_dir_all(&task_dir) {
        let _ = run_git(
            project_path,
            &["worktree", "remove", "--force", &worktree_str],
        );
        let _ = manager.prune_worktrees();
        return Err(format!("Failed to create worktree task dir: {}", e));
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
        SessionType::Fusion { .. } => {
            vec![session.project_path.join(".hive-fusion").join(&session.id)]
        }
        SessionType::Debate { .. } => {
            vec![session.project_path.join(".hive-debate").join(&session.id)]
        }
        _ => vec![session
            .project_path
            .join(".hive-manager")
            .join("worktrees")
            .join(&session.id)],
    };

    let mut cleanup_errors = Vec::new();
    for worktree in worktrees {
        if !session_prefixes
            .iter()
            .any(|prefix| worktree.path.starts_with(prefix))
        {
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
    use tempfile::TempDir;

    /// #177 T1. The whitelist is what stops us writing a `root: true` config into
    /// an operator's own source tree, so the `None` arms matter as much as the
    /// `Some` ones.
    #[test]
    fn worktree_config_boundary_root_whitelists_hive_roots_only() {
        let repo = Path::new("/repo");

        assert_eq!(
            worktree_config_boundary_root(repo, Path::new("/repo/.hive-manager/worktrees/sid/primary")),
            Some(PathBuf::from("/repo/.hive-manager"))
        );
        // Isolated-cell layout nests one level deeper; still the same boundary.
        assert_eq!(
            worktree_config_boundary_root(
                repo,
                Path::new("/repo/.hive-manager/worktrees/isolated/sid/cell")
            ),
            Some(PathBuf::from("/repo/.hive-manager"))
        );
        assert_eq!(
            worktree_config_boundary_root(repo, Path::new("/repo/.hive-fusion/sid/variant-a")),
            Some(PathBuf::from("/repo/.hive-fusion"))
        );
        assert_eq!(
            worktree_config_boundary_root(repo, Path::new("/repo/.hive-debate/sid/debater-a")),
            Some(PathBuf::from("/repo/.hive-debate"))
        );

        // The worktree IS the repo root -> no component -> None. Without this the
        // shim would land at `<repo>/.eslintrc.json` and disable the operator's
        // own lint config.
        assert_eq!(worktree_config_boundary_root(repo, repo), None);
        // Outside the repo entirely.
        assert_eq!(
            worktree_config_boundary_root(repo, Path::new("/elsewhere/wt")),
            None
        );
        // Inside the repo but NOT a hive-owned root: must be refused.
        assert_eq!(
            worktree_config_boundary_root(repo, Path::new("/repo/tmp/wt")),
            None
        );
    }

    /// #177 T2. Content equality is load-bearing: verified against ESLint 8.57.1,
    /// a `{}` boundary does NOT stop the cascade, only `{"root": true}` does.
    #[test]
    fn ensure_worktree_config_boundary_writes_an_inert_root_marker() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path();
        let worktree = repo.join(".hive-manager").join("worktrees").join("sid").join("primary");
        std::fs::create_dir_all(&worktree).unwrap();
        // The worktree can lint itself.
        std::fs::write(worktree.join(".eslintrc.json"), "{\"plugins\":[\"x\"]}").unwrap();

        ensure_worktree_config_boundary(repo, &worktree);

        let boundary = repo.join(".hive-manager").join(".eslintrc.json");
        assert!(boundary.exists(), "boundary should be written");
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&boundary).unwrap()).unwrap();
        assert_eq!(parsed, serde_json::json!({ "root": true }));
        // It must sit ABOVE the worktree, not inside it.
        assert!(worktree.starts_with(boundary.parent().unwrap()));
    }

    /// #177 T3 — the highest-value test here. Writing a boundary above a worktree
    /// that has no ESLint config of its own converts a loud `exit 2` into a silent
    /// false green (empty ruleset exits 0), which is worse for a QA gate.
    #[test]
    fn boundary_is_not_written_when_the_worktree_has_no_eslint_config() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path();
        let worktree = repo.join(".hive-manager").join("worktrees").join("sid").join("primary");
        std::fs::create_dir_all(&worktree).unwrap();

        ensure_worktree_config_boundary(repo, &worktree);

        assert!(!repo.join(".hive-manager").join(".eslintrc.json").exists());
    }

    /// A `package.json` carrying `eslintConfig` also counts as lintable.
    #[test]
    fn boundary_is_written_for_package_json_eslint_config() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path();
        let worktree = repo.join(".hive-manager").join("worktrees").join("sid").join("primary");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join("package.json"),
            "{\"name\":\"x\",\"eslintConfig\":{\"plugins\":[\"y\"]}}",
        )
        .unwrap();

        ensure_worktree_config_boundary(repo, &worktree);

        assert!(repo.join(".hive-manager").join(".eslintrc.json").exists());

        // A package.json WITHOUT eslintConfig must not trigger it.
        let temp2 = TempDir::new().unwrap();
        let repo2 = temp2.path();
        let wt2 = repo2.join(".hive-manager").join("worktrees").join("sid").join("primary");
        std::fs::create_dir_all(&wt2).unwrap();
        std::fs::write(wt2.join("package.json"), "{\"name\":\"x\"}").unwrap();
        ensure_worktree_config_boundary(repo2, &wt2);
        assert!(!repo2.join(".hive-manager").join(".eslintrc.json").exists());
    }

    /// #177 T4. Idempotent, and never clobbers a file the operator wrote.
    #[test]
    fn boundary_is_idempotent_and_never_clobbers_an_operator_file() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path();
        let hive = repo.join(".hive-manager");
        let worktree = hive.join("worktrees").join("sid").join("primary");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(".eslintrc.json"), "{}").unwrap();

        let operator = "{\"root\":true,\"rules\":{\"no-debugger\":\"error\"}}";
        std::fs::create_dir_all(&hive).unwrap();
        std::fs::write(hive.join(".eslintrc.json"), operator).unwrap();

        ensure_worktree_config_boundary(repo, &worktree);
        let wt_b = hive.join("worktrees").join("sid").join("second");
        std::fs::create_dir_all(&wt_b).unwrap();
        std::fs::write(wt_b.join(".eslintrc.json"), "{}").unwrap();
        ensure_worktree_config_boundary(repo, &wt_b);

        assert_eq!(
            std::fs::read_to_string(hive.join(".eslintrc.json")).unwrap(),
            operator,
            "operator content must survive untouched"
        );

        // Deleted -> recreated (no process-wide latch).
        std::fs::remove_file(hive.join(".eslintrc.json")).unwrap();
        ensure_worktree_config_boundary(repo, &worktree);
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(hive.join(".eslintrc.json")).unwrap())
                .unwrap();
        assert_eq!(parsed, serde_json::json!({ "root": true }));
    }

    #[test]
    fn test_generate_branch_name_hive() {
        let branch =
            generate_branch_name("session-123", "worker-1", SessionMode::Hive, CellType::Hive);
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
