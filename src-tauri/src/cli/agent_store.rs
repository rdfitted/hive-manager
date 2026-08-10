//! Per-agent CLI state isolation (#207).
//!
//! Codex keeps its SQLite state — `state_5.sqlite`, `logs_2.sqlite`, `goals_1.sqlite`,
//! `memories_1.sqlite`, `queue_1.sqlite` — in one directory shared by every process that
//! inherits the operator's environment. Workers spawned together therefore race the same
//! lock, and once that store is large enough the losers die during startup with
//! `failed to initialize state runtime ... (code: 5) database is locked`.
//!
//! `sqlite_home` relocates only that store. Credentials (`auth.json`) and configuration
//! (`config.toml`) deliberately stay in the single shared `CODEX_HOME`. Giving each worker
//! a whole private home instead would copy OAuth tokens into per-session directories and
//! let N copies refresh — and rotate — the same refresh token against each other. Verified
//! against codex-cli 0.147.0: with `sqlite_home` set, every `*.sqlite` lands in the
//! override directory and the shared home stays DB-free.

use std::path::{Path, PathBuf};

/// Root directory holding every per-agent CLI state store.
///
/// Follows the same temp-dir convention the Windows batch writer already uses. These
/// stores are regenerable caches, and keeping worker traffic out of the operator's real
/// `~/.codex` is what stops that store from growing without bound.
pub fn store_root() -> PathBuf {
    std::env::temp_dir().join("hive-manager").join("cli-state")
}

/// Restrict the store root to the current user on platforms with a shared, world-writable
/// temp directory. On Windows, `%TEMP%` is already per-user; on Unix, `/tmp` is shared and
/// the per-agent paths are predictable, so without this another local user could pre-own
/// them. Best-effort: isolation still works if the chmod fails, it is just less private.
#[cfg(unix)]
fn restrict_to_current_user(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_to_current_user(_dir: &Path) {}

/// Create the per-agent store directory, tightening permissions on the shared root.
pub fn ensure_store_dir(store_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(store_dir)?;
    if let Some(root) = store_dir.parent() {
        restrict_to_current_user(root);
    }
    restrict_to_current_user(store_dir);
    Ok(())
}

/// State-store directory for a single agent.
pub fn store_dir_for(agent_id: &str) -> PathBuf {
    store_root().join(sanitize_component(agent_id))
}

/// Flags that point `command` at its own state store.
///
/// Returns empty for every CLI that has no shared-store problem, so non-codex agents keep
/// their argv byte-for-byte identical.
pub fn isolation_args(command: &str, store_dir: &Path) -> Vec<String> {
    if !is_codex(command) {
        return Vec::new();
    }

    // Two argv entries rather than one pre-joined string: the Windows batch writer quotes
    // any single arg containing a space, which is what lets a store path under
    // `D:\Code Projects\...` survive. Passing `-c sqlite_home=<path>` unquoted splits at
    // the space and silently consumes the positional prompt slot.
    vec![
        "-c".to_string(),
        format!("sqlite_home={}", store_dir.display()),
    ]
}

/// Whether `command` is a CLI with a shared-store contention problem — i.e. codex,
/// tolerating a full path or an `.exe` suffix. Callers use this both to inject the
/// isolation flags and to stagger spawns.
pub fn has_contended_store(command: &str) -> bool {
    is_codex(command)
}

/// Whether `command` names the codex CLI, tolerating a full path or an `.exe` suffix.
fn is_codex(command: &str) -> bool {
    Path::new(command)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("codex"))
}

/// How long an untouched per-agent store survives before the startup janitor removes it.
const STALE_STORE_RETENTION: std::time::Duration =
    std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// Best-effort removal of per-agent stores that have not been touched within the
/// retention window (#207 fix 6, for the stores this app creates).
///
/// These are regenerable caches keyed by agent id; a finished session never touches its
/// stores again, so anything old is garbage. Without this the fix for unbounded growth of
/// `~/.codex` would just relocate the growth here. Every failure is skipped silently — a
/// janitor must never take the app down.
pub fn cleanup_stale_stores() {
    let root = store_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };

    let now = std::time::SystemTime::now();
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        // Age by the newest timestamp of the directory AND everything inside it: a
        // directory's own mtime does not move when SQLite writes into an existing file,
        // so judging the directory alone could reap a store that is still in active use
        // (e.g. by a long-lived process from a prior session).
        let newest = std::fs::read_dir(entry.path())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|child| child.metadata().ok()?.modified().ok())
            .chain(metadata.modified().ok())
            .max();
        let expired = newest
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > STALE_STORE_RETENTION);
        if expired && std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }

    if removed > 0 {
        tracing::info!(
            removed,
            root = %root.display(),
            "removed stale per-agent CLI state stores"
        );
    }
}

/// Agent ids are `{session-uuid}-worker-{n}` today, but they reach the filesystem here, so
/// anything outside a conservative allowlist collapses to `_`.
fn sanitize_component(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    if cleaned.is_empty() {
        "agent".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_gets_a_private_sqlite_home() {
        let args = isolation_args("codex", Path::new("/tmp/store"));
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        assert!(args[1].starts_with("sqlite_home="));
        assert!(args[1].contains("store"));
    }

    #[test]
    fn the_store_path_stays_one_argv_entry_so_spaces_survive_quoting() {
        // Regression guard for the failure that makes codex exit 2 with
        // "unexpected argument": if the path were pre-joined onto `-c`, or split here,
        // a store under `D:\Code Projects\...` would break the positional prompt.
        let args = isolation_args("codex", Path::new(r"D:\Code Projects\hive\store"));
        assert_eq!(args.len(), 2);
        assert!(args[1].contains("Code Projects"));
    }

    #[test]
    fn non_codex_clis_are_left_untouched() {
        for cli in ["claude", "droid", "cursor", "opencode", "qwen", "agy"] {
            assert!(
                isolation_args(cli, Path::new("/tmp/store")).is_empty(),
                "{cli} should not be given codex flags"
            );
        }
    }

    #[test]
    fn codex_is_detected_through_a_path_or_exe_suffix() {
        let store = Path::new("/tmp/store");
        assert!(!isolation_args("codex.exe", store).is_empty());
        assert!(!isolation_args(r"C:\npm\bin\codex.exe", store).is_empty());
        assert!(!isolation_args("CODEX", store).is_empty());
    }

    #[test]
    fn each_agent_id_maps_to_its_own_directory() {
        let a = store_dir_for("sess-worker-1");
        let b = store_dir_for("sess-worker-2");
        assert_ne!(a, b);
        assert!(a.starts_with(store_root()));
    }

    #[test]
    fn path_separators_in_an_agent_id_cannot_escape_the_store_root() {
        let escaped = store_dir_for("../../etc/passwd");
        assert!(escaped.starts_with(store_root()));
        assert_eq!(escaped.components().count(), store_root().components().count() + 1);
    }
}
