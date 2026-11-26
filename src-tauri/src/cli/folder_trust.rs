//! Pre-trust a worker's own folder so its CLI never asks (#466).
//!
//! Workers spawn into fresh git worktrees, and two CLIs stop there on a folder-trust
//! prompt that looks like a stalled agent. Both accept a per-process override, so the
//! trust decision is made for this spawn only and nothing is written to the operator's
//! own configuration:
//!
//! - **codex** (verified on codex-cli 0.171.0): `-c projects={'<path>'={trust_level='trusted'}}`.
//!   The override is keyed by both the worktree and the repository root it belongs to,
//!   because codex applies trust at the repository root ("Trusting will apply to the
//!   repository root"). Either key alone suppressed the prompt in a live capture; the
//!   operator's `config.toml` was left unchanged.
//! - **agy** (verified on agy 1.5.18): `--settings <file>`, "settings for this session
//!   only, never saved", with `trustedWorkspaces: [<worktree>]`. The file lives in the
//!   agent's private state directory. `~/.gemini/antigravity-cli/settings.json` was left
//!   unchanged.
//!
//! Claude Code shows a trust prompt too, but its default option is "No, exit" and it has
//! no per-process trust override, so it is left to detection (`pty::trust_prompt`), which
//! surfaces it rather than answering. Cursor runs inside WSL and was not captured.
//!
//! Everything here is best-effort. A path the override cannot carry safely, or a failed
//! settings write, yields no flags, and the spawn falls back to detection.

use std::path::{Component, Path, PathBuf};

/// File name of the agy per-session settings overlay inside an agent's state directory.
pub const AGY_TRUST_SETTINGS_FILE: &str = "agy-trust-settings.json";

/// Flags that pre-trust `cwd` for `command`, placed in front of the CLI's own arguments.
///
/// `existing_args` are the arguments the caller already built; an operator who passes
/// their own `--settings` (agy) or `projects` override (codex) keeps it untouched.
pub fn trust_args(command: &str, cwd: &Path, store_dir: &Path, existing_args: &[&str]) -> Vec<String> {
    match cli_stem(command).as_deref() {
        Some("codex") => codex_trust_args(cwd, existing_args),
        Some("agy") | Some("antigravity") => agy_trust_args(cwd, store_dir, existing_args),
        _ => Vec::new(),
    }
}

/// [`trust_args`] at the PTY spawn choke point, where the working directory is optional.
pub fn spawn_trust_args(command: &str, cwd: Option<&str>, store_dir: &Path, args: &[&str]) -> Vec<String> {
    cwd.map(|cwd| trust_args(command, Path::new(cwd), store_dir, args)).unwrap_or_default()
}

fn cli_stem(command: &str) -> Option<String> {
    Path::new(command)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.to_ascii_lowercase())
}

fn codex_trust_args(cwd: &Path, existing_args: &[&str]) -> Vec<String> {
    let overridden = existing_args
        .windows(2)
        .any(|pair| matches!(pair[0], "-c" | "--config") && pair[1].trim_start().starts_with("projects"));
    if overridden {
        return Vec::new();
    }
    let mut keys = vec![cwd.to_path_buf()];
    if let Some(root) = git_repo_root(cwd) {
        if normalize_path(&root.to_string_lossy()) != normalize_path(&cwd.to_string_lossy()) {
            keys.push(root);
        }
    }
    let mut entries = Vec::new();
    for key in &keys {
        let key = key.to_string_lossy();
        if !is_safe_override_path(&key) {
            return Vec::new();
        }
        // TOML literal strings: no escapes, so Windows backslashes survive as written.
        entries.push(format!("'{key}'={{trust_level='trusted'}}"));
    }
    // One argv entry for the value: the Windows batch writer quotes args containing a
    // space, so a worktree under `D:\Code Projects\...` stays a single argument.
    vec!["-c".to_string(), format!("projects={{{}}}", entries.join(","))]
}

fn agy_trust_args(cwd: &Path, store_dir: &Path, existing_args: &[&str]) -> Vec<String> {
    if existing_args.iter().any(|arg| *arg == "--settings" || arg.starts_with("--settings=")) {
        return Vec::new();
    }
    let cwd = cwd.to_string_lossy();
    let settings = store_dir.join(AGY_TRUST_SETTINGS_FILE);
    if !is_safe_override_path(&cwd) || !is_safe_override_path(&settings.to_string_lossy()) {
        return Vec::new();
    }
    let body = serde_json::json!({ "trustedWorkspaces": [cwd] });
    let written = super::agent_store::ensure_store_dir(store_dir)
        .and_then(|()| std::fs::write(&settings, body.to_string()));
    if let Err(error) = written {
        tracing::warn!(
            settings = %settings.display(),
            %error,
            "could not write the agy trust overlay; the trust prompt is left to detection"
        );
        return Vec::new();
    }
    vec!["--settings".to_string(), settings.to_string_lossy().into_owned()]
}

/// Paths the override can carry: no quote that would end a TOML literal string, no
/// batch-file metacharacter, and no control character.
fn is_safe_override_path(path: &str) -> bool {
    !path.is_empty()
        && !path.chars().any(|c| matches!(c, '\'' | '"' | '%' | '!' | '^' | '\r' | '\n') || c.is_control())
}

/// The repository root a working tree belongs to: the tree itself for a normal clone,
/// or the main checkout for a linked worktree (`.git` file → `gitdir` → `commondir`).
/// Reads files only; never runs git.
pub fn git_repo_root(cwd: &Path) -> Option<PathBuf> {
    let dot_git = cwd.join(".git");
    let metadata = std::fs::metadata(&dot_git).ok()?;
    if metadata.is_dir() {
        return Some(lexical_normalize(cwd));
    }
    let pointer = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir = pointer.lines().find_map(|line| line.strip_prefix("gitdir:"))?.trim();
    let gitdir = absolutize(cwd, Path::new(gitdir));
    let common = match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(common) => absolutize(&gitdir, Path::new(common.trim())),
        Err(_) => gitdir,
    };
    let common = lexical_normalize(&common);
    if common.file_name().is_some_and(|name| name == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

fn absolutize(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Comparable form of a path as a CLI prints it: `\\?\` dropped, separators unified,
/// trailing separator removed, and case folded on Windows.
pub fn normalize_path(path: &str) -> String {
    let path = path.trim();
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let mut normalized = path.replace('/', "\\");
    while normalized.len() > 1 && normalized.ends_with('\\') && !normalized.ends_with(":\\") {
        normalized.pop();
    }
    if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linked_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        let worktree = temp.path().join("repo-wt");
        let admin = repo.join(".git").join("worktrees").join("repo-wt");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        (temp, repo, worktree)
    }

    #[test]
    fn linked_worktree_resolves_to_its_main_checkout() {
        let (_temp, repo, worktree) = linked_worktree();
        assert_eq!(normalize_path(&git_repo_root(&worktree).unwrap().to_string_lossy()),
            normalize_path(&repo.to_string_lossy()));
        assert_eq!(normalize_path(&git_repo_root(&repo).unwrap().to_string_lossy()),
            normalize_path(&repo.to_string_lossy()));
        assert!(git_repo_root(&repo.join("missing")).is_none());
    }

    #[test]
    fn codex_override_trusts_worktree_and_repo_root_in_one_argument() {
        let (_temp, repo, worktree) = linked_worktree();
        let args = trust_args("codex", &worktree, Path::new("unused"), &[]);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        let value = &args[1];
        assert!(value.starts_with("projects={'"), "{value}");
        assert!(value.contains(&format!("'{}'={{trust_level='trusted'}}", worktree.display())), "{value}");
        assert!(value.contains(&format!("'{}'={{trust_level='trusted'}}", repo.display())), "{value}");
        assert!(!value.contains('"'), "double quotes would break the batch quoting: {value}");
    }

    #[test]
    fn codex_override_steps_aside_for_an_operator_projects_flag_and_unsafe_paths() {
        let cwd = Path::new(r"D:\Code Projects\x");
        assert!(trust_args("codex", cwd, Path::new("s"), &["-c", "projects.x.trust_level=untrusted"]).is_empty());
        assert!(trust_args("codex.exe", Path::new(r"D:\it's"), Path::new("s"), &[]).is_empty());
        assert!(trust_args("codex", Path::new(r"D:\100%done"), Path::new("s"), &[]).is_empty());
        let plain = trust_args(r"C:\bin\codex.exe", cwd, Path::new("s"), &[]);
        assert_eq!(plain[1], r"projects={'D:\Code Projects\x'={trust_level='trusted'}}");
    }

    #[test]
    fn agy_overlay_names_only_the_worktree_and_lives_in_the_agent_store() {
        let temp = tempfile::TempDir::new().unwrap();
        let store = temp.path().join("store");
        let cwd = temp.path().join("worker-1");
        let args = trust_args("agy", &cwd, &store, &["--model", "m"]);
        assert_eq!(args[0], "--settings");
        assert_eq!(PathBuf::from(&args[1]), store.join(AGY_TRUST_SETTINGS_FILE));
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
        assert_eq!(written, serde_json::json!({ "trustedWorkspaces": [cwd.to_string_lossy()] }));
        assert!(trust_args("agy", &cwd, &store, &["--settings", "mine.json"]).is_empty());
    }

    #[test]
    fn other_clis_get_nothing() {
        for cli in ["claude", "wsl", "cursor"] {
            assert!(trust_args(cli, Path::new("/w"), Path::new("/s"), &[]).is_empty(), "{cli}");
        }
    }

    #[test]
    fn printed_paths_compare_across_case_prefix_and_separators() {
        assert_eq!(normalize_path(r"\\?\D:\Code Projects\hive\"), normalize_path("D:/Code Projects/hive"));
        if cfg!(windows) {
            assert_eq!(normalize_path(r"d:\hm\repo"), normalize_path(r"D:\HM\Repo"));
        }
        assert_ne!(normalize_path(r"D:\a\worker-1"), normalize_path(r"D:\a\worker-2"));
    }
}
