//! Launch environment shared by all agents that execute through WSL.

use std::ffi::OsString;
use std::path::Path;

use portable_pty::CommandBuilder;

/// Apply fresh agent identity and WSL forwarding to the actual PTY command.
/// Inspect `executable` before Windows wraps it in a batch-file launcher.
pub fn apply_launch_env(
    command: &mut CommandBuilder,
    executable: &str,
    cwd: Option<&Path>,
    injected: &[(String, String)],
    inherited: &[(OsString, OsString)],
) -> Result<(), String> {
    let environment = launch_env(executable, cwd, injected, inherited)?;
    for (key, _) in inherited {
        if key.to_string_lossy().to_ascii_uppercase().starts_with("HIVE_") {
            command.env_remove(key);
        }
    }
    for (key, value) in environment {
        command.env(key, value);
    }
    Ok(())
}

/// Compute forwarding without reading or modifying process-global environment.
pub fn launch_env(
    executable: &str,
    cwd: Option<&Path>,
    injected: &[(String, String)],
    inherited: &[(OsString, OsString)],
) -> Result<Vec<(String, String)>, String> {
    let mut environment = injected.to_vec();
    let binary = executable.rsplit(['/', '\\']).next().unwrap_or(executable)
        .to_ascii_lowercase();
    let binary = binary.strip_suffix(".exe").unwrap_or(&binary);
    // The classifier also accepts the CLI name `cursor`; here we have the
    // executable, so only a real wsl boundary needs environment forwarding.
    if binary != "wsl" || !crate::session::SessionController::cli_runs_under_wsl(binary) {
        return Ok(environment);
    }

    if let Some(cwd) = cwd {
        let dot_git = cwd.join(".git");
        if dot_git.is_file() {
            // These inherited keys redirect Git even when our seven overrides are
            // correct. Stop before touching the command; never clear operator state.
            for key in ["GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_PREFIX"] {
                if inherited.iter().any(|(name, value)| {
                    name.to_string_lossy().eq_ignore_ascii_case(key) && !value.is_empty()
                }) {
                    return Err(format!("WSL launch would overwrite operator environment entry {key}"));
                }
                if inherited.iter().any(|(name, value)| {
                    name.to_string_lossy().eq_ignore_ascii_case("WSLENV")
                        && value.to_string_lossy().split(':').any(|entry| {
                            entry.split('/').next().is_some_and(|name| name.eq_ignore_ascii_case(key))
                        })
                }) {
                    return Err(format!("WSL launch would overwrite operator WSLENV entry {key}"));
                }
            }
            let contents = std::fs::read_to_string(&dot_git)
                .map_err(|error| format!("Cannot read worktree .git file: {error}"))?;
            let gitdir = contents.trim().strip_prefix("gitdir:")
                .map(str::trim).filter(|value| !value.is_empty())
                .ok_or_else(|| "Invalid worktree .git file".to_string())?;
            let windows_absolute = gitdir.as_bytes().get(1) == Some(&b':')
                || gitdir.starts_with("\\\\");
            let admin_dir = if windows_absolute || Path::new(gitdir).is_absolute() {
                gitdir.to_string()
            } else {
                cwd.join(gitdir).to_string_lossy().into_owned()
            };
            let git_env = [
                ("GIT_DIR", admin_dir.replace('/', "\\")),
                ("GIT_WORK_TREE", cwd.to_string_lossy().replace('/', "\\")),
                ("GIT_CONFIG_COUNT", "2".to_string()),
                ("GIT_CONFIG_KEY_0", "core.autocrlf".to_string()),
                ("GIT_CONFIG_VALUE_0", "true".to_string()),
                ("GIT_CONFIG_KEY_1", "core.fileMode".to_string()),
                ("GIT_CONFIG_VALUE_1", "false".to_string()),
            ];
            for (key, value) in git_env {
                if inherited.iter().any(|(old_key, old_value)| {
                    old_key.to_string_lossy().eq_ignore_ascii_case(key)
                        && old_value != &OsString::from(&value)
                }) {
                    return Err(format!("WSL launch would overwrite operator environment entry {key}"));
                }
                environment.push((key.to_string(), value));
            }
        }
    }

    let mut entries: Vec<String> = inherited.iter()
        .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("WSLENV"))
        .map(|(_, value)| value.to_string_lossy().split(':')
            .filter(|entry| !entry.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    for (key, _) in &environment {
        let expected = if matches!(key.as_str(), "GIT_DIR" | "GIT_WORK_TREE") {
            format!("{key}/p")
        } else {
            key.clone()
        };
        let existing: Vec<_> = entries.iter().filter(|entry| {
            entry.split('/').next().is_some_and(|name| name.eq_ignore_ascii_case(key))
        }).collect();
        if existing.iter().any(|entry| **entry != expected) {
            return Err(format!("WSL launch would overwrite operator WSLENV entry {key}"));
        }
        if existing.is_empty() {
            entries.push(expected);
        }
    }
    environment.push(("WSLENV".to_string(), entries.join(":")));
    Ok(environment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn identity() -> Vec<(String, String)> {
        ["HIVE_SESSION_ID", "HIVE_AGENT_ID", "HIVE_ROLE", "HIVE_FUTURE_KEY"]
            .into_iter().map(|key| (key.to_string(), "identity:unchanged".to_string())).collect()
    }

    #[test]
    fn wsl_worktree_forwards_every_injected_key_and_git_settings() {
        let root = tempfile::tempdir().unwrap();
        for gitdir in ["D:/Code Projects/repo/.git/worktrees/worker", r"D:\Code Projects\repo\.git\worktrees\worker"] {
            std::fs::write(root.path().join(".git"), format!("gitdir: {gitdir}\n")).unwrap();
            let env: HashMap<_, _> = launch_env("wsl", Some(root.path()), &identity(), &[]).unwrap().into_iter().collect();
            let forwarded: Vec<_> = env["WSLENV"].split(':').collect();
            for (key, value) in identity() {
                assert!(forwarded.contains(&key.as_str()), "missing {key}");
                assert_eq!(env[&key], value);
            }
            for (key, value) in &env {
                if key == "WSLENV" { continue; }
                let entry = if matches!(key.as_str(), "GIT_DIR" | "GIT_WORK_TREE") {
                    format!("{key}/p")
                } else { key.clone() };
                assert!(forwarded.contains(&entry.as_str()), "missing {entry}");
                if key == "GIT_DIR" {
                    assert_eq!(value, r"D:\Code Projects\repo\.git\worktrees\worker");
                }
            }
            assert_eq!(env["GIT_CONFIG_COUNT"], "2");
            assert_eq!(env["GIT_CONFIG_KEY_0"], "core.autocrlf");
            assert_eq!(env["GIT_CONFIG_VALUE_0"], "true");
            assert_eq!(env["GIT_CONFIG_KEY_1"], "core.fileMode");
            assert_eq!(env["GIT_CONFIG_VALUE_1"], "false");
        }
    }

    #[test]
    fn main_checkout_and_non_wsl_skip_git_overrides() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        let env = launch_env("C:\\Windows\\System32\\wsl.exe", Some(root.path()), &identity(), &[]).unwrap();
        assert!(env.iter().any(|(key, _)| key == "WSLENV"));
        assert!(!env.iter().any(|(key, _)| key.starts_with("GIT_")));
        std::fs::remove_dir(root.path().join(".git")).unwrap();
        std::fs::write(root.path().join(".git"), "gitdir: D:/repo/.git/worktrees/worker").unwrap();
        for executable in ["codex", "cursor", "cmd.exe"] {
            assert_eq!(launch_env(executable, Some(root.path()), &identity(), &[]).unwrap(), identity());
        }
    }

    #[test]
    fn wslenv_preserves_operator_entries_and_rejects_conflicts() {
        let inherited = [("WSLENV".into(), "OPERATOR_PATH/p:HIVE_AGENT_ID".into())];
        let env: HashMap<_, _> = launch_env("wsl", None, &identity(), &inherited).unwrap().into_iter().collect();
        assert!(env["WSLENV"].starts_with("OPERATOR_PATH/p:HIVE_AGENT_ID:"));
        assert_eq!(env["WSLENV"].matches("HIVE_AGENT_ID").count(), 1);
        let conflict = [("WSLENV".into(), "HIVE_AGENT_ID/p".into())];
        assert!(launch_env("wsl", None, &identity(), &conflict).unwrap_err().contains("HIVE_AGENT_ID"));
    }

    #[test]
    fn worktree_rejects_operator_git_overrides_without_exposing_values() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".git"), "gitdir: D:/repo/.git/worktrees/worker").unwrap();
        let inherited = [("GIT_CONFIG_COUNT".into(), "1".into())];
        assert_eq!(
            launch_env("wsl", Some(root.path()), &identity(), &inherited).unwrap_err(),
            "WSL launch would overwrite operator environment entry GIT_CONFIG_COUNT"
        );
        let inherited = [("WSLENV".into(), "GIT_DIR".into())];
        assert_eq!(
            launch_env("wsl", Some(root.path()), &identity(), &inherited).unwrap_err(),
            "WSL launch would overwrite operator WSLENV entry GIT_DIR"
        );
    }

    #[test]
    fn worktree_rejects_inherited_git_context_before_command_mutation() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".git"), "gitdir: D:/repo/.git/worktrees/worker").unwrap();
        let main = tempfile::tempdir().unwrap();
        std::fs::create_dir(main.path().join(".git")).unwrap();
        let mut missed = Vec::new();
        for key in ["GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_PREFIX"] {
            for name in [key.to_string(), key.to_ascii_lowercase(), key.replacen("GIT_", "gIt_", 1)] {
                let inherited = [(name.clone().into(), "private-operator-context".into())];
                let mut command = CommandBuilder::new("wsl");
                command.env(&name, "private-operator-context");
                command.env("HIVE_FUTURE_INHERITED", "preserve-before-validation");
                let result = apply_launch_env(&mut command, "wsl", Some(root.path()), &identity(), &inherited);
                if result != Err(format!("WSL launch would overwrite operator environment entry {key}")) {
                    missed.push(name.clone());
                }
                assert_eq!(command.get_env(&name).unwrap(), "private-operator-context");
                assert_eq!(command.get_env("HIVE_FUTURE_INHERITED").unwrap(), "preserve-before-validation");
                // Empty values cannot redirect Git's context.
                assert!(launch_env("wsl", Some(root.path()), &identity(), &[(key.into(), OsString::new())]).is_ok());
                let main_env = launch_env("wsl", Some(main.path()), &identity(), &inherited).unwrap();
                assert!(!main_env.iter().any(|(name, _)| name.starts_with("GIT_")));
                assert_eq!(launch_env("codex", Some(root.path()), &identity(), &inherited).unwrap(), identity());
            }
        }
        assert!(missed.is_empty(), "unrejected Git context keys: {}", missed.join(", "));
    }

    #[test]
    fn worktree_rejects_operator_git_context_forwarding_without_mutation() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".git"), "gitdir: D:/repo/.git/worktrees/worker").unwrap();
        let main = tempfile::tempdir().unwrap();
        std::fs::create_dir(main.path().join(".git")).unwrap();
        let mut missed = Vec::new();
        for key in ["GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_PREFIX"] {
            for entry in [key.to_string(), format!("{}/p", key.to_ascii_lowercase()), format!("{}/p", key.replacen("GIT_", "gIt_", 1))] {
                let forwarding = format!("OPERATOR_PATH/p:{entry}");
                let inherited = [("WSLENV".into(), OsString::from(&forwarding))];
                let mut command = CommandBuilder::new("wsl");
                command.env("WSLENV", &forwarding);
                let result = apply_launch_env(&mut command, "wsl", Some(root.path()), &identity(), &inherited);
                if result != Err(format!("WSL launch would overwrite operator WSLENV entry {key}")) {
                    missed.push(entry);
                }
                if result.is_err() {
                    assert_eq!(command.get_env("WSLENV").unwrap(), forwarding.as_str());
                }
                let env: HashMap<_, _> = launch_env("wsl", Some(main.path()), &identity(), &inherited).unwrap().into_iter().collect();
                assert!(env["WSLENV"].starts_with(&format!("{forwarding}:")));
                assert_eq!(launch_env("codex", Some(root.path()), &identity(), &inherited).unwrap(), identity());
            }
        }
        assert!(missed.is_empty(), "unrejected Git forwarding keys: {}", missed.join(", "));
    }

    #[test]
    fn non_wsl_pty_also_removes_inherited_identity_without_git_or_forwarding() {
        let inherited = [("HIVE_AGENT_TOKEN".into(), OsString::new())];
        let mut command = CommandBuilder::new("codex");
        command.env("HIVE_AGENT_TOKEN", "");
        command.env_remove("WSLENV");
        command.env_remove("GIT_DIR");
        apply_launch_env(&mut command, "codex", None, &identity(), &inherited).unwrap();
        assert!(command.get_env("HIVE_AGENT_TOKEN").is_none());
        assert!(command.get_env("WSLENV").is_none());
        assert!(command.get_env("GIT_DIR").is_none());
        assert_eq!(command.get_env("HIVE_AGENT_ID").unwrap(), "identity:unchanged");
    }

    #[test]
    fn actual_pty_command_removes_all_inherited_hive_keys_and_forwards_new_keys() {
        let inherited = [("HIVE_AGENT_TOKEN".into(), OsString::new()), ("HIVE_FUTURE_INHERITED".into(), "old".into())];
        let mut command = CommandBuilder::new("cmd.exe");
        for (key, value) in &inherited { command.env(key, value); }
        apply_launch_env(&mut command, "wsl", None, &identity(), &inherited).unwrap();
        assert!(command.get_env("HIVE_AGENT_TOKEN").is_none());
        assert!(command.get_env("HIVE_FUTURE_INHERITED").is_none());
        assert_eq!(command.get_env("HIVE_AGENT_ID").unwrap(), "identity:unchanged");
        assert!(command.get_env("WSLENV").unwrap().to_string_lossy().split(':').any(|entry| entry == "HIVE_AGENT_ID"));
        let injected = vec![("HIVE_AGENT_TOKEN".to_string(), String::new())];
        apply_launch_env(&mut command, "wsl", None, &injected, &inherited).unwrap();
        assert_eq!(command.get_env("WSLENV").unwrap(), "HIVE_AGENT_TOKEN");
    }
}
