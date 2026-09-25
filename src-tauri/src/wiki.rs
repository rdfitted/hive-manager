use std::ffi::OsStr;
use std::path::{Path, PathBuf};

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
}
