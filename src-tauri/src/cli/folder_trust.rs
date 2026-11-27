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
//! Only the seat's own Hive-managed worktree is pre-trusted: a linked git worktree at
//! `<project>/.hive-manager/worktrees/<session>/<name>` where `<session>` is the agent's
//! session and the worktree's git metadata points back into that same `<project>`.
//! Any other folder, such as the operator's own project checkout a Queen starts in,
//! keeps the CLI's own consent prompt.
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
pub fn trust_args(command: &str, cwd: &Path, session_id: &str, store_dir: &Path, existing_args: &[&str]) -> Vec<String> {
    if managed_worktree_project(cwd, session_id).is_none() {
        return Vec::new();
    }
    match cli_stem(command).as_deref() {
        Some("codex") => codex_trust_args(cwd, existing_args),
        Some("agy") | Some("antigravity") => agy_trust_args(cwd, store_dir, existing_args),
        _ => Vec::new(),
    }
}

/// [`trust_args`] at the PTY spawn choke point. The session is the agent id's UUID
/// prefix (`<session>-<seat>`), and `project_of` gives that session's recorded project.
/// The worktree must be the one named for this seat in exactly that project; an id
/// without a session, another seat's worktree, an unknown session or any other project
/// gets no pre-trust.
pub fn spawn_trust_args(
    command: &str, agent_id: &str, cwd: Option<&str>, project_of: &dyn Fn(&str) -> Option<PathBuf>,
    store_dir: &Path, args: &[&str],
) -> Vec<String> {
    let session = agent_id.get(..36).filter(|id| uuid::Uuid::parse_str(id).is_ok() && agent_id[36..].starts_with('-'));
    let (Some(cwd), Some(session)) = (cwd.map(Path::new), session) else {
        return Vec::new();
    };
    // The worktree must be this seat's own: Hive Manager names it after the seat
    // (`<session>-worker-2` gets `worker-2`, the Queen gets `queen`).
    let seat = &agent_id[37..];
    if cwd.file_name().and_then(|name| name.to_str()) != Some(seat) {
        return Vec::new();
    }
    let (Some(recorded), Some(project)) = (project_of(session), managed_worktree_project(cwd, session)) else {
        return Vec::new();
    };
    if !same_path(&recorded, &project) {
        return Vec::new();
    }
    trust_args(command, cwd, session, store_dir, args)
}

fn cli_stem(command: &str) -> Option<String> {
    Path::new(command)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.to_ascii_lowercase())
}

/// Whether the operator's own arguments already decide folder trust for this CLI: a
/// codex `projects` override or an agy `--settings`. Hive Manager then neither
/// pre-trusts nor answers the prompt.
pub fn operator_sets_trust(command: &str, args: &[&str]) -> bool {
    let sets_projects = |value: &str| value.trim_start().starts_with("projects");
    match cli_stem(command).as_deref() {
        Some("codex") => {
            args.windows(2).any(|pair| matches!(pair[0], "-c" | "--config") && sets_projects(pair[1]))
                || args.iter().any(|arg| {
                    let attached = arg.strip_prefix("--config=").or_else(|| arg.strip_prefix("-c=")).or_else(|| arg.strip_prefix("-c"));
                    attached.is_some_and(sets_projects)
                })
        }
        Some("agy") | Some("antigravity") => args.iter().any(|arg| *arg == "--settings" || arg.starts_with("--settings=")),
        _ => false,
    }
}

/// [`operator_sets_trust`], or, for codex, any sign in the operator's own `config.toml`
/// that they set a project's trust by hand. Hive Manager then neither pre-trusts nor
/// answers the prompt.
pub fn operator_decides_trust(command: &str, args: &[&str]) -> bool {
    operator_sets_trust(command, args) || (cli_stem(command).as_deref() == Some("codex") && codex_config_may_deny())
}

/// Whether the operator's codex `config.toml` might deny trust anywhere. A missing file
/// denies nothing; a file that cannot be read counts as a denial.
fn codex_config_may_deny() -> bool {
    let Some(home) = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| {
        std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|home| PathBuf::from(home).join(".codex"))
    }) else {
        return false;
    };
    match std::fs::read_to_string(home.join("config.toml")) {
        Ok(config) => config_text_may_deny(&config),
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Fails closed on the raw text instead of parsing TOML. Any of these counts as a
/// possible denial:
/// - a quoted `untrusted` value in any case or quote style (`"untrusted"`, `'UNTRUSTED'`,
///   `'''untrusted'''`), so a bare word such as an env-var name does not count;
/// - a `\u`/`\U` escape inside a double-quoted string, which could spell one;
/// - a line ending in a line-continuation backslash, which could split one.
///
/// Single-quoted strings have no escapes, so Windows paths such as `'C:\Users\x'` and
/// escaped backslashes such as `"C:\\Users"` are not escapes here. A false positive only
/// costs a consent prompt, which is surfaced; a parser that missed one valid spelling
/// would override an explicit denial.
fn config_text_may_deny(config: &str) -> bool {
    let lower = config.to_ascii_lowercase();
    if lower.contains("\"untrusted\"") || lower.contains("'untrusted'") {
        return true;
    }
    config.lines().any(|line| {
        let trimmed = line.trim_end();
        let continuation = (trimmed.len() - trimmed.trim_end_matches('\\').len()) % 2 == 1;
        continuation || has_unicode_escape(line)
    })
}

/// Whether a line has a `\u`/`\U` escape inside a double-quoted string.
fn has_unicode_escape(line: &str) -> bool {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '#' => return false,
            '\'' => {
                i = chars[i + 1..].iter().position(|c| *c == '\'').map_or(chars.len(), |end| i + 1 + end + 1);
            }
            '"' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j] != '"' {
                    if chars[j] == '\\' && j + 1 < chars.len() {
                        if matches!(chars[j + 1], 'u' | 'U') {
                            return true;
                        }
                        j += 2;
                        continue;
                    }
                    j += 1;
                }
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    false
}

/// Whether two paths name the same file or directory on disk, by canonical path. Any
/// error (missing, unreadable) is "no", so trust is never granted on a guess.
fn same_path(a: &Path, b: &Path) -> bool {
    matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

fn codex_trust_args(cwd: &Path, existing_args: &[&str]) -> Vec<String> {
    if operator_decides_trust("codex", existing_args) {
        return Vec::new();
    }
    let mut keys = vec![cwd.to_path_buf()];
    if let Some(root) = git_repo_root(cwd) {
        if !same_path(&root, cwd) {
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
    if operator_sets_trust("agy", existing_args) {
        return Vec::new();
    }
    let cwd = cwd.to_string_lossy();
    let settings = store_dir.join(AGY_TRUST_SETTINGS_FILE);
    if !is_safe_override_path(&cwd) || !is_safe_override_path(&settings.to_string_lossy()) {
        return Vec::new();
    }
    let body = serde_json::json!({ "trustedWorkspaces": [cwd] });
    // Atomic, so agy never reads a half-written overlay while a same-id respawn rewrites it.
    let written = super::agent_store::ensure_store_dir(store_dir).and_then(|()| {
        let mut temp = tempfile::NamedTempFile::new_in(store_dir)?;
        std::io::Write::write_all(&mut temp, body.to_string().as_bytes())?;
        temp.persist(&settings).map(|_| ()).map_err(|error| error.error)
    });
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

/// The project a worktree belongs to, when `cwd` is a worktree Hive Manager made for a
/// seat of `session_id`: `<project>/.hive-manager/worktrees/<session_id>/<name>`, whose
/// `.git` is a file and whose git metadata names `<project>` as its main checkout.
/// Path shape alone is never enough: the metadata must agree with the path.
pub fn managed_worktree_project(cwd: &Path, session_id: &str) -> Option<PathBuf> {
    let parts: Vec<&std::ffi::OsStr> = cwd.components().map(|part| part.as_os_str()).collect();
    let tail = parts.get(parts.len().checked_sub(4)?..)?;
    if tail[0] != ".hive-manager" || tail[1] != "worktrees" || tail[2] != session_id {
        return None;
    }
    if !std::fs::metadata(cwd.join(".git")).is_ok_and(|meta| meta.is_file()) {
        return None;
    }
    let project = lexical_normalize(cwd).ancestors().nth(4)?.to_path_buf();
    let root = git_repo_root(cwd)?;
    if !same_path(&root, &project) {
        return None;
    }
    // The worktree must be registered for this very directory: its admin entry under the
    // project's `.git/worktrees/` points back at this `.git`, so a copied pointer file
    // in some other directory does not qualify.
    let admin = worktree_admin_dir(cwd)?;
    if !same_path(admin.parent()?, &project.join(".git").join("worktrees")) {
        return None;
    }
    let back = std::fs::read_to_string(admin.join("gitdir")).ok()?;
    let back = absolutize(&admin, Path::new(back.trim()));
    same_path(&back, &cwd.join(".git")).then_some(project)
}

/// The admin directory a linked worktree's `.git` file points at.
fn worktree_admin_dir(cwd: &Path) -> Option<PathBuf> {
    let pointer = std::fs::read_to_string(cwd.join(".git")).ok()?;
    let gitdir = pointer.lines().find_map(|line| line.strip_prefix("gitdir:"))?.trim();
    Some(lexical_normalize(&absolutize(cwd, Path::new(gitdir))))
}

/// Paths the override can carry: no quote that would end a TOML literal string, no
/// cmd.exe metacharacter (the Windows batch writer quotes only on a space, `"`, `&` or
/// `|`), and no control character. Anything else is left to detection.
fn is_safe_override_path(path: &str) -> bool {
    !path.is_empty()
        && !path.chars().any(|c| {
            matches!(c, '\'' | '"' | '%' | '!' | '^' | '&' | '|' | '<' | '>' | '(' | ')') || c.is_control()
        })
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
/// trailing separator removed, and case folded on Windows. For display and loose
/// matching only; trust decisions compare canonical paths ([`same_path`]).
pub fn normalize_path(path: &str) -> String {
    let path = path.trim();
    if cfg!(windows) {
        let path = path.strip_prefix(r"\\?\").unwrap_or(path);
        let mut normalized = path.replace('/', "\\");
        while normalized.len() > 1 && normalized.ends_with('\\') && !normalized.ends_with(":\\") {
            normalized.pop();
        }
        normalized.to_lowercase()
    } else {
        // A backslash is an ordinary filename character on Unix: keep it literal.
        let mut normalized = path.to_string();
        while normalized.len() > 1 && normalized.ends_with('/') {
            normalized.pop();
        }
        normalized
    }
}

/// Lay out a Hive-managed linked worktree under `root`, as a hive creates one:
/// `<root>/<project>/.hive-manager/worktrees/<session>/<name>` with a `.git` file
/// pointing into the main checkout. Returns `(project, worktree)`.
#[cfg(test)]
pub(crate) fn managed_worktree_fixture(root: &Path, project: &str, session: &str, name: &str) -> (PathBuf, PathBuf) {
    let repo = root.join(project);
    let worktree = repo.join(".hive-manager").join("worktrees").join(session).join(name);
    let admin = repo.join(".git").join("worktrees").join(name);
    std::fs::create_dir_all(&admin).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(worktree.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    std::fs::write(admin.join("commondir"), "../..\n").unwrap();
    // git writes the reverse pointer with forward slashes on Windows too.
    std::fs::write(admin.join("gitdir"), format!("{}\n", worktree.join(".git").display().to_string().replace('\\', "/"))).unwrap();
    (repo, worktree)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn managed() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::TempDir::new().unwrap();
        let (repo, worktree) = managed_worktree_fixture(temp.path(), "repo", "s1", "worker-1");
        (temp, repo, worktree)
    }

    #[test]
    fn linked_worktree_resolves_to_its_main_checkout() {
        let (_temp, repo, worktree) = managed();
        assert_eq!(normalize_path(&git_repo_root(&worktree).unwrap().to_string_lossy()),
            normalize_path(&repo.to_string_lossy()));
        assert!(git_repo_root(&repo.join("missing")).is_none());
    }

    #[test]
    fn only_the_seats_own_managed_worktree_counts() {
        let (temp, repo, worktree) = managed();
        let project = managed_worktree_project(&worktree, "s1").unwrap();
        assert_eq!(normalize_path(&project.to_string_lossy()), normalize_path(&repo.to_string_lossy()));
        // Another session's worktree is not this seat's.
        assert!(managed_worktree_project(&worktree, "s2").is_none());
        // The operator's own checkout (where a Queen may start) is not managed.
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert!(managed_worktree_project(&repo, "s1").is_none());
        // The right shape without a linked `.git` file is not managed either.
        let bare = temp.path().join("x").join(".hive-manager").join("worktrees").join("s1").join("worker-2");
        std::fs::create_dir_all(&bare).unwrap();
        assert!(managed_worktree_project(&bare, "s1").is_none());
        assert!(managed_worktree_project(&worktree.join("nested"), "s1").is_none());
        // The right shape whose git metadata points at some other repository.
        let (_, elsewhere) = managed_worktree_fixture(temp.path(), "other", "s1", "worker-3");
        let planted = repo.join(".hive-manager").join("worktrees").join("s1").join("worker-3");
        std::fs::create_dir_all(&planted).unwrap();
        std::fs::copy(elsewhere.join(".git"), planted.join(".git")).unwrap();
        assert!(managed_worktree_project(&planted, "s1").is_none(), "metadata must agree with the path");
        // worker-1's own pointer copied into an unregistered worker-4 directory.
        let copied = repo.join(".hive-manager").join("worktrees").join("s1").join("worker-4");
        std::fs::create_dir_all(&copied).unwrap();
        std::fs::copy(worktree.join(".git"), copied.join(".git")).unwrap();
        assert!(managed_worktree_project(&copied, "s1").is_none(), "the worktree must be registered for this directory");
    }

    #[test]
    fn a_codex_config_that_might_deny_anything_fails_closed() {
        for config in [
            "[projects.'D:\\repo']\ntrust_level = \"untrusted\"\n",
            "[\"projects\".'D:\\repo']\n\"trust_level\" = 'UNTRUSTED'\n",
            "[projects.'D:\\repo']\ntrust_level = '''untrusted'''\n",
            "projects = { 'x' = { trust_level = \"un\\u0074rusted\" } }\n",
            "note = \"\\U0001F600\"\n",
            "trust_level = \"\"\"un\\\ntrusted\"\"\"\n",
        ] {
            assert!(config_text_may_deny(config), "{config}");
        }
        // The shapes in a real Windows config: literal-string paths, escaped backslashes,
        // and the word in an identifier rather than a value.
        let ordinary = "[projects.'C:\\Users\\me']\ntrust_level = \"trusted\"\n\
            notify = [ \"C:\\\\Users\\\\me\\\\app.exe\" ]\n\
            NODE_REPL_UNTRUSTED_ENV_ALLOWLIST = \"X\"\n# a comment with \"\\u\" in it\n";
        assert!(!config_text_may_deny(ordinary), "{ordinary}");
    }

    #[test]
    fn path_identity_is_the_filesystem_not_the_spelling() {
        let temp = tempfile::TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        let sibling = temp.path().join("repo2");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(same_path(&repo, &temp.path().join(".").join("repo")));
        assert!(!same_path(&repo, &sibling));
        assert!(!same_path(&repo, &temp.path().join("missing")), "an error is never a match");
    }

    #[cfg(unix)]
    #[test]
    fn a_trailing_space_names_a_different_checkout_on_unix() {
        let temp = tempfile::TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        let spaced = temp.path().join("repo ");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&spaced).unwrap();
        assert!(!same_path(&repo, &spaced));
    }

    #[test]
    fn spawn_trust_needs_the_session_in_the_agent_id() {
        let temp = tempfile::TempDir::new().unwrap();
        let session = "00000000-0000-4000-8000-000000000466";
        let (_repo, worktree) = managed_worktree_fixture(temp.path(), "repo", session, "worker-1");
        let cwd = worktree.to_string_lossy().into_owned();
        let store = temp.path().join("store");
        let recorded = |id: &str| (id == session).then(|| temp.path().join("repo"));
        let id = format!("{session}-worker-1");
        assert_eq!(spawn_trust_args("codex", &id, Some(&cwd), &recorded, &store, &[]).len(), 2);
        assert!(spawn_trust_args("codex", "scratch:terminal", Some(&cwd), &recorded, &store, &[]).is_empty());
        assert!(spawn_trust_args("codex", "00000000-0000-4000-8000-000000000999-worker-1", Some(&cwd), &recorded, &store, &[]).is_empty());
        assert!(spawn_trust_args("codex", &id, None, &recorded, &store, &[]).is_empty());
        // The session's recorded project is some other checkout: no pre-trust.
        let elsewhere = |_: &str| Some(temp.path().join("other-project"));
        assert!(spawn_trust_args("codex", &id, Some(&cwd), &elsewhere, &store, &[]).is_empty());
        // No roster answer (unknown session, or a spawn racing the roster lock): none either.
        assert!(spawn_trust_args("codex", &id, Some(&cwd), &|_: &str| None, &store, &[]).is_empty());
        // Another seat of the same session spawned into worker-1's worktree: none.
        assert!(spawn_trust_args("codex", &format!("{session}-worker-2"), Some(&cwd), &recorded, &store, &[]).is_empty());
    }

    #[test]
    fn codex_override_trusts_worktree_and_repo_root_in_one_argument() {
        let (_temp, repo, worktree) = managed();
        let args = trust_args("codex", &worktree, "s1", Path::new("unused"), &[]);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        let value = &args[1];
        assert!(value.starts_with("projects={'"), "{value}");
        assert!(value.contains(&format!("'{}'={{trust_level='trusted'}}", worktree.display())), "{value}");
        assert!(value.contains(&format!("'{}'={{trust_level='trusted'}}", repo.display())), "{value}");
        assert!(!value.contains('"'), "double quotes would break the batch quoting: {value}");
    }

    #[test]
    fn nothing_is_pre_trusted_outside_a_managed_worktree() {
        let (_temp, repo, _worktree) = managed();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        for cli in ["codex", "agy"] {
            assert!(trust_args(cli, &repo, "s1", &repo.join("store"), &[]).is_empty(), "{cli} on the project checkout");
        }
    }

    #[test]
    fn codex_override_steps_aside_for_an_operator_projects_flag_and_unsafe_paths() {
        let (_temp, _repo, worktree) = managed();
        assert!(trust_args("codex", &worktree, "s1", Path::new("s"), &["-c", "projects.x.trust_level=untrusted"]).is_empty());
        assert!(!is_safe_override_path(r"D:\it's"));
        assert!(!is_safe_override_path(r"D:\100%done"));
        for path in [r"D:\R&D\wt", r"D:\a|b", r"D:\a<b", r"D:\a>b", r"D:\x (1)\wt"] {
            assert!(!is_safe_override_path(path), "{path}");
        }
        assert!(is_safe_override_path(r"D:\Code Projects\x"));
    }

    #[test]
    fn every_codex_config_spelling_counts_as_an_operator_override() {
        for args in [
            vec!["-c", "projects={}"],
            vec!["--config", "projects.x.trust_level=untrusted"],
            vec!["--config=projects.x.trust_level=untrusted"],
            vec!["-c=projects={}"],
            vec!["-cprojects={}"],
        ] {
            assert!(operator_sets_trust("codex", &args), "{args:?}");
        }
        assert!(!operator_sets_trust("codex", &["-c", "model=x", "--config=sqlite_home=y"]));
        assert!(operator_sets_trust("agy", &["--settings=x.json"]));
    }

    #[test]
    fn codex_is_recognised_through_a_windows_executable_path() {
        let (_temp, _repo, worktree) = managed();
        assert_eq!(trust_args(r"C:\bin\codex.exe", &worktree, "s1", Path::new("s"), &[]).len(), 2);
    }

    #[test]
    fn agy_overlay_names_only_the_worktree_and_lives_in_the_agent_store() {
        let (temp, _repo, cwd) = managed();
        let store = temp.path().join("store");
        let args = trust_args("agy", &cwd, "s1", &store, &["--model", "m"]);
        assert_eq!(args[0], "--settings");
        assert_eq!(PathBuf::from(&args[1]), store.join(AGY_TRUST_SETTINGS_FILE));
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
        assert_eq!(written, serde_json::json!({ "trustedWorkspaces": [cwd.to_string_lossy()] }));
        assert!(trust_args("agy", &cwd, "s1", &store, &["--settings", "mine.json"]).is_empty());
    }

    #[test]
    fn other_clis_get_nothing() {
        let (_temp, _repo, worktree) = managed();
        for cli in ["claude", "wsl", "cursor"] {
            assert!(trust_args(cli, &worktree, "s1", Path::new("/s"), &[]).is_empty(), "{cli}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_backslash_is_a_filename_character_on_unix() {
        assert_ne!(normalize_path("/tmp/team/repo"), normalize_path("/tmp/team\\repo"));
        assert_eq!(normalize_path("/tmp/team/repo/"), normalize_path("/tmp/team/repo"));
    }

    #[cfg(windows)]
    #[test]
    fn printed_paths_compare_across_prefix_separators_and_trailing_slash() {
        assert_eq!(normalize_path(r"\\?\D:\Code Projects\hive\"), normalize_path("D:/Code Projects/hive"));
        assert_ne!(normalize_path(r"D:\a\worker-1"), normalize_path(r"D:\a\worker-2"));
    }

    #[cfg(windows)]
    #[test]
    fn printed_paths_compare_case_insensitively_on_windows() {
        assert_eq!(normalize_path(r"d:\hm\repo"), normalize_path(r"D:\HM\Repo"));
    }
}
