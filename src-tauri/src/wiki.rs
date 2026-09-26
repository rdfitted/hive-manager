use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_WIKI_ROOT: &str = "~/.ai-docs/wiki";

/// Resolve the one wiki root shared by Atlas, prompt rendering, and CLI health.
pub(crate) fn resolve_wiki_root(configured: Option<&str>) -> PathBuf {
    let env_root = std::env::var_os("HIVE_WIKI_ROOT").filter(|value| !value.is_empty());
    let preferred = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let fallback = if cfg!(windows) { "HOME" } else { "USERPROFILE" };
    let home = std::env::var_os(preferred)
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os(fallback).filter(|value| !value.is_empty()))
        .map(PathBuf::from);
    resolve_wiki_root_from(env_root.as_deref(), configured, home.as_deref())
}

pub(crate) fn resolve_wiki_root_from(
    env_root: Option<&OsStr>,
    configured: Option<&str>,
    home: Option<&Path>,
) -> PathBuf {
    let selected = env_root
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            configured
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from(DEFAULT_WIKI_ROOT));
    expand_tilde_path(&selected, home)
}

fn expand_tilde_path(path: &Path, home: Option<&Path>) -> PathBuf {
    let Some(raw) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some(rest) = raw.strip_prefix('~') else {
        return path.to_path_buf();
    };
    if !rest.is_empty() && !rest.starts_with('/') && !rest.starts_with('\\') {
        return path.to_path_buf();
    }
    let Some(home) = home else {
        return path.to_path_buf();
    };
    let rest = rest.trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        home.to_path_buf()
    } else {
        home.join(rest)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WikiState {
    Absent,
    Local,
    Remote,
}

/// A wiki probe shared by every prompt in one launch, round, or judge batch.
pub(crate) struct WikiBatch {
    pub root: PathBuf,
    pub state: WikiState,
}

impl WikiBatch {
    pub(crate) fn probe(root: PathBuf) -> Self {
        // Synchronous launch entry points can be called by async handlers. Like the
        // controller's spawn_blocking git checks, keep these bounded CLI probes off
        // a Tokio worker whenever a multithreaded runtime is active.
        if matches!(
            tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()),
            Ok(tokio::runtime::RuntimeFlavor::MultiThread)
        ) {
            tokio::task::block_in_place(|| Self::probe_blocking(root))
        } else {
            Self::probe_blocking(root)
        }
    }

    pub(crate) fn probe_blocking(root: PathBuf) -> Self {
        let state = wiki_state(&root);
        Self { root, state }
    }

    #[cfg(test)]
    pub(crate) fn probe_from(
        root: PathBuf,
        probe: impl Fn(&str, &[&str], &Path) -> bool,
    ) -> Self {
        let state = wiki_state_from(&root, probe);
        Self { root, state }
    }
}

pub(crate) fn wiki_state(root: &Path) -> WikiState {
    wiki_state_from(root, bounded_probe)
}

pub(crate) fn wiki_state_from(
    root: &Path,
    probe: impl Fn(&str, &[&str], &Path) -> bool,
) -> WikiState {
    if !root.is_dir() || !root.join("index.md").is_file() {
        return WikiState::Absent;
    }
    if !probe("git", &["rev-parse", "--show-toplevel"], root)
        || !probe("git", &["remote", "get-url", "origin"], root)
        || !probe("gh", &["auth", "status"], root)
    {
        return WikiState::Local;
    }
    WikiState::Remote
}

pub(crate) fn bounded_probe(executable: &str, args: &[&str], root: &Path) -> bool {
    // A remote check uses up to three sequential probes. Each gets five seconds,
    // so one batch has at most 15 seconds of probe deadlines, off the executor.
    bounded_probe_with_timeout(executable, args, root, Duration::from_secs(5))
}

pub(crate) fn bounded_probe_with_timeout(
    executable: &str,
    args: &[&str],
    root: &Path,
    timeout: Duration,
) -> bool {
    let mut child = match Command::new(executable)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_over_config_over_default_with_tilde_expansion() {
        let home = Path::new("/tmp/wiki-home");
        assert_eq!(
            resolve_wiki_root_from(Some(OsStr::new("~/override")), Some("~/configured"), Some(home)),
            home.join("override")
        );
        assert_eq!(
            resolve_wiki_root_from(None, Some("  ~/configured  "), Some(home)),
            home.join("configured")
        );
        assert_eq!(
            resolve_wiki_root_from(None, Some("  "), Some(home)),
            home.join(".ai-docs/wiki")
        );
        assert_eq!(
            resolve_wiki_root_from(None, Some("~someone/wiki"), Some(home)),
            PathBuf::from("~someone/wiki")
        );
    }

    #[test]
    fn timed_out_probe_keeps_indexed_wiki_local() {
        let wiki = tempfile::tempdir().unwrap();
        std::fs::write(wiki.path().join("index.md"), "# Test wiki\n").unwrap();
        #[cfg(windows)]
        let (program, args): (&str, &[&str]) = (
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 1"],
        );
        #[cfg(not(windows))]
        let (program, args): (&str, &[&str]) = ("/bin/sh", &["-c", "sleep 1"]);

        assert!(bounded_probe_with_timeout(
            program,
            args,
            wiki.path(),
            Duration::from_secs(30),
        ));
        assert!(!bounded_probe_with_timeout(
            program,
            args,
            wiki.path(),
            Duration::from_millis(1),
        ));
        assert_eq!(
            wiki_state_from(wiki.path(), |_, _, root| {
                bounded_probe_with_timeout(program, args, root, Duration::from_millis(1))
            }),
            WikiState::Local,
        );
    }

    #[test]
    fn embedded_role_pointers_have_starter_pages_or_optional_project_context() {
        use crate::orchestrator::org_graph::definitions::{
            resolve_role_definition, RoleDefinitionSource, RoleResolutionIssueKind,
        };
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let starter = repo.join("docs/wiki-starter");
        let project = tempfile::tempdir().unwrap();
        let missing_root = project.path().join("missing-wiki");
        let empty_root = project.path().join("empty-wiki");
        std::fs::create_dir(&empty_root).unwrap();

        let mut checked = 0;
        for entry in std::fs::read_dir(repo.join("roles")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let role = path.file_stem().unwrap().to_str().unwrap();
            let source = std::fs::read_to_string(&path).unwrap();
            let frontmatter: serde_json::Value = serde_json::from_str(source.lines().nth(1).unwrap()).unwrap();
            for pointer in frontmatter["knowledge_scope"].as_array().unwrap() {
                let relative = pointer["pointer"].as_str().unwrap();
                match pointer["source"].as_str().unwrap() {
                    "institutional" => assert!(starter.join(relative).is_file(), "{role}: {relative}"),
                    "project" => {
                        assert_eq!(relative, "project-dna.md", "{role}: project context is optional");
                        assert!(pointer["summary"].as_str().unwrap().starts_with("Optional "));
                    }
                    other => panic!("{role}: unexpected source {other}"),
                }
            }
            for root in [&missing_root, &empty_root] {
                let resolved = resolve_role_definition(project.path(), Some(root), role);
                assert!(resolved.definition.is_some(), "{role}: embedded fallback missing");
                assert_eq!(resolved.base_source, Some(RoleDefinitionSource::EmbeddedDefault));
                assert!(resolved.issues.iter().any(|issue| issue.kind == RoleResolutionIssueKind::InstitutionalUnavailable));
            }
            checked += 1;
        }
        assert_eq!(checked, 15);
    }
}
