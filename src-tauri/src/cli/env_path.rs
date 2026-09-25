//! Finder launches inherit a small PATH. Extend it once before any application threads start.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// Pure PATH merge, also compiled on Windows for mutation tests.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn merge_paths(
    inherited: &OsStr,
    login_shell: Option<&OsStr>,
    fallback_dirs: &[PathBuf],
) -> OsString {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();

    let inherited_dirs = std::env::split_paths(inherited);
    let shell_dirs = login_shell.into_iter().flat_map(std::env::split_paths);
    for path in inherited_dirs.chain(shell_dirs).chain(fallback_dirs.iter().cloned()) {
        if !path.as_os_str().is_empty() && seen.insert(path.as_os_str().to_os_string()) {
            paths.push(path);
        }
    }

    std::env::join_paths(paths).unwrap_or_else(|_| inherited.to_os_string())
}

#[cfg(target_os = "macos")]
fn known_cli_dirs(home: Option<&std::path::Path>) -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = home {
        dirs.extend([
            home.join(".local/bin"),
            home.join(".npm-global/bin"),
            home.join(".volta/bin"),
        ]);
    }
    if let Some(nvm_bin) = std::env::var_os("NVM_BIN") {
        dirs.push(PathBuf::from(nvm_bin));
    }
    dirs.retain(|dir| dir.is_dir());
    dirs
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn probe_login_shell(shell: &std::path::Path, timeout: std::time::Duration) -> Option<OsString> {
    use std::process::{Command, Stdio};
    use std::time::Instant;
    use std::io::{Read, Seek};
    use std::os::unix::ffi::OsStringExt;

    // A file avoids a pipe reader waiting on a grandchild left by shell startup files.
    let mut output_file = tempfile::tempfile().ok()?;
    let mut child = Command::new(shell)
        .args(["-ilc", "printf %s \"$PATH\""])
        .stdout(Stdio::from(output_file.try_clone().ok()?))
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                output_file.rewind().ok()?;
                let mut output = Vec::new();
                output_file.read_to_end(&mut output).ok()?;
                return (!output.is_empty()).then(|| OsString::from_vec(output));
            }
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(std::time::Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Only call at the very start of `run`, before storage or any worker/runtime is created.
#[cfg(target_os = "macos")]
pub(crate) fn initialize_macos_cli_environment() {
    use std::time::Duration;

    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let shell = std::env::var_os("SHELL")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
    let shell_path = probe_login_shell(&shell, Duration::from_secs(3));
    let home = std::env::var_os("HOME").filter(|value| !value.is_empty()).map(PathBuf::from);
    let fallback_dirs = known_cli_dirs(home.as_deref());
    let merged = merge_paths(&inherited, shell_path.as_deref(), &fallback_dirs);

    let inherited_set: HashSet<PathBuf> = std::env::split_paths(&inherited).collect();
    let added: Vec<String> = std::env::split_paths(&merged)
        .filter(|path| !inherited_set.contains(path))
        .map(|path| path.display().to_string())
        .collect();

    std::env::set_var("PATH", merged);
    for (name, value) in [
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("LANG", "en_US.UTF-8"),
    ] {
        if std::env::var_os(name).is_none() {
            std::env::set_var(name, value);
        }
    }
    tracing::info!(?added, "macOS CLI PATH initialized");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_inherited_order_and_adds_fallback_clis_once() {
        let temp = tempfile::tempdir().unwrap();
        let inherited = temp.path().join("system");
        let home_cli = temp.path().join("home/.local/bin");
        let brew_cli = temp.path().join("homebrew/bin");
        std::fs::create_dir_all(&inherited).unwrap();
        std::fs::create_dir_all(&home_cli).unwrap();
        std::fs::create_dir_all(&brew_cli).unwrap();
        std::fs::write(home_cli.join("claude"), "fixture").unwrap();
        std::fs::write(brew_cli.join("codex"), "fixture").unwrap();

        let original = std::env::join_paths([&inherited]).unwrap();
        let shell = std::env::join_paths([&inherited, &brew_cli]).unwrap();
        let merged = merge_paths(&original, Some(&shell), &[home_cli.clone(), brew_cli.clone()]);
        let dirs: Vec<_> = std::env::split_paths(&merged).collect();
        assert_eq!(dirs, vec![inherited, brew_cli.clone(), home_cli.clone()]);
        assert!(dirs.iter().any(|dir| dir.join("claude").is_file()));
        assert!(dirs.iter().any(|dir| dir.join("codex").is_file()));
    }

    #[cfg(unix)]
    #[test]
    fn timed_out_shell_probe_fails_open_to_inherited_and_fallback() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let shell = temp.path().join("slow-shell");
        std::fs::write(&shell, "#!/bin/sh\nexec sleep 2\n").unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let probe = probe_login_shell(&shell, std::time::Duration::from_millis(50));
        assert!(probe.is_none());
        let original = std::env::join_paths([temp.path().join("system")]).unwrap();
        let fallback = temp.path().join("fallback");
        let merged = merge_paths(&original, probe.as_deref(), &[fallback.clone()]);
        assert!(std::env::split_paths(&merged).any(|dir| dir == fallback));
    }
}
