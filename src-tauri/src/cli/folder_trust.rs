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
/// The worktree must belong to exactly that project; an id without a session, an unknown
/// session or any other project gets no pre-trust.
pub fn spawn_trust_args(
    command: &str, agent_id: &str, cwd: Option<&str>, project_of: &dyn Fn(&str) -> Option<PathBuf>,
    store_dir: &Path, args: &[&str],
) -> Vec<String> {
    let session = agent_id.get(..36).filter(|id| uuid::Uuid::parse_str(id).is_ok() && agent_id[36..].starts_with('-'));
    let (Some(cwd), Some(session)) = (cwd.map(Path::new), session) else {
        return Vec::new();
    };
    let (Some(recorded), Some(project)) = (project_of(session), managed_worktree_project(cwd, session)) else {
        return Vec::new();
    };
    if normalize_path(&recorded.to_string_lossy()) != normalize_path(&project.to_string_lossy()) {
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

/// [`operator_sets_trust`], or a codex project the operator marked
/// `trust_level = "untrusted"` in their own `config.toml` for this worktree or its root.
pub fn operator_decides_trust(command: &str, args: &[&str], cwd: Option<&Path>) -> bool {
    if operator_sets_trust(command, args) {
        return true;
    }
    let (Some("codex"), Some(cwd)) = (cli_stem(command).as_deref(), cwd) else {
        return false;
    };
    let mut paths = vec![cwd.to_path_buf()];
    paths.extend(git_repo_root(cwd));
    codex_config_text().is_some_and(|config| codex_marks_untrusted(&config, &paths))
}

fn codex_config_text() -> Option<String> {
    let home = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| {
        std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|home| PathBuf::from(home).join(".codex"))
    })?;
    std::fs::read_to_string(home.join("config.toml")).ok()
}

/// Whether the operator's codex `config.toml` marks any of `paths` untrusted.
fn codex_marks_untrusted(config: &str, paths: &[PathBuf]) -> bool {
    let wanted: Vec<String> = paths.iter().map(|path| normalize_path(&path.to_string_lossy())).collect();
    codex_untrusted_projects(config).iter().any(|key| wanted.contains(&normalize_path(key)))
}

/// Project paths a codex `config.toml` marks `trust_level = "untrusted"`, in every form
/// TOML allows for that entry: a `[projects.'<path>']` table, dotted keys or inline
/// tables under `[projects]`, and a top-level `projects = { ... }` inline table, with
/// comments anywhere. Not a full TOML parser; anything it cannot read counts as absent.
fn codex_untrusted_projects(config: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut section: Option<String> = None;
    for raw in config.lines() {
        let line = compact_toml_line(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            section = Some(header.to_string());
            continue;
        }
        match section.as_deref() {
            Some(header) if header.starts_with("projects.") => {
                if let Some(key) = whole_quoted(&header["projects.".len()..]) {
                    if is_untrusted_value(line.strip_prefix("trust_level=")) {
                        found.push(key);
                    }
                }
            }
            Some("projects") => found.extend(untrusted_entries(&line)),
            None => found.extend(line.strip_prefix("projects=").map(untrusted_entries).unwrap_or_default()),
            _ => {}
        }
    }
    found
}

/// A TOML line without its comment and without the whitespace outside quoted strings.
fn compact_toml_line(raw: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in raw.chars() {
        match quote {
            Some(q) => {
                out.push(c);
                if q == '"' && c == '\\' && !escaped {
                    escaped = true;
                    continue;
                }
                if c == q && !escaped {
                    quote = None;
                }
                escaped = false;
            }
            None if c == '#' => break,
            None if c.is_whitespace() => {}
            None => {
                if c == '\'' || c == '"' {
                    quote = Some(c);
                }
                out.push(c);
            }
        }
    }
    out
}

/// The quoted string `text` consists of, unquoted: `'literal'` as written, `"basic"`
/// with `\\` and `\"` unescaped. Returns the string and the length it took.
fn leading_quoted(text: &str) -> Option<(String, usize)> {
    let quote = text.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let mut value = String::new();
    let mut escaped = false;
    for (i, c) in text.char_indices().skip(1) {
        if quote == '"' && c == '\\' && !escaped {
            escaped = true;
            continue;
        }
        if c == quote && !escaped {
            return Some((value, i + c.len_utf8()));
        }
        value.push(c);
        escaped = false;
    }
    None
}

fn whole_quoted(text: &str) -> Option<String> {
    leading_quoted(text).filter(|(_, len)| *len == text.len()).map(|(value, _)| value)
}

fn is_untrusted_value(value: Option<&str>) -> bool {
    value.and_then(whole_quoted).is_some_and(|value| value == "untrusted")
}

/// `'<path>'={trust_level="untrusted",...}` and `'<path>'.trust_level="untrusted"`
/// entries in a compacted `[projects]` line or `projects={...}` inline table.
fn untrusted_entries(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let Some((key, len)) = leading_quoted(&text[at..]) else {
            at += text[at..].chars().next().map_or(1, char::len_utf8);
            continue;
        };
        let rest = &text[at + len..];
        if let Some(value) = rest.strip_prefix(".trust_level=") {
            let end = value.find([',', '}']).unwrap_or(value.len());
            if is_untrusted_value(Some(&value[..end])) {
                found.push(key);
            }
        } else if let Some(body) = rest.strip_prefix("={") {
            let end = body.find('}').unwrap_or(body.len());
            if body[..end].split(',').any(|pair| is_untrusted_value(pair.strip_prefix("trust_level="))) {
                found.push(key);
            }
        }
        at += len;
    }
    found
}

fn codex_trust_args(cwd: &Path, existing_args: &[&str]) -> Vec<String> {
    if operator_decides_trust("codex", existing_args, Some(cwd)) {
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
    (normalize_path(&root.to_string_lossy()) == normalize_path(&project.to_string_lossy())).then_some(project)
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
    fn an_operators_untrusted_codex_project_is_left_alone() {
        let (_temp, repo, worktree) = managed();
        let config = format!(
            "model = \"x\"\n\n[projects.'{}']\ntrust_level = \"untrusted\"\n\n[projects.'D:\\other']\ntrust_level = \"trusted\"\n",
            repo.display()
        );
        assert!(codex_marks_untrusted(&config, &[worktree.clone(), repo.clone()]));
        assert!(!codex_marks_untrusted(&config, &[worktree.clone()]), "only the root was marked");
        let trusted = config.replace("\"untrusted\"", "\"trusted\"");
        assert!(!codex_marks_untrusted(&trusted, &[repo]));
    }

    #[test]
    fn every_toml_spelling_of_an_untrusted_project_is_read() {
        let marked = |config: &str| codex_untrusted_projects(config);
        let path = r"D:\Code Projects\repo";
        for config in [
            format!("[projects.'{path}'] # mine\ntrust_level = \"untrusted\" # leave it\n"),
            format!("[projects.\"D:\\\\Code Projects\\\\repo\"]\ntrust_level='untrusted'\n"),
            format!("[projects]\n'{path}' = {{ trust_level = \"untrusted\" }}\n"),
            format!("[projects]\n'{path}'.trust_level = \"untrusted\"\n"),
            format!("projects = {{ 'D:\\x' = {{ trust_level = \"trusted\" }}, '{path}' = {{ trust_level = \"untrusted\" }} }}\n"),
        ] {
            assert_eq!(marked(&config), [path.to_string()], "{config}");
        }
        for config in [
            format!("[projects.'{path}']\ntrust_level = \"trusted\" # untrusted\n"),
            format!("# [projects.'{path}']\n# trust_level = \"untrusted\"\n"),
            format!("[projects.'{path}'.extra]\ntrust_level = \"untrusted\"\n"),
            format!("[other]\n'{path}' = {{ trust_level = \"untrusted\" }}\n"),
        ] {
            assert!(marked(&config).is_empty(), "{config}");
        }
    }

    #[cfg(windows)]
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
