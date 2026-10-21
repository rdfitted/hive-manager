//! Read-only review collection. A recorded spawn SHA avoids all base-discovery calls.
use std::{path::Path, time::Duration};
use serde::Serialize;

const FILE_LINES: usize = 2_000;
const TOTAL_LINES: usize = 20_000;

#[derive(Debug, Serialize)]
pub struct ReviewFile {
    pub path: String,
    pub unified: String,
    pub binary: bool,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct ReviewDiff {
    pub base: String,
    pub agent_id: String,
    pub files: Vec<ReviewFile>,
    pub truncated: bool,
}

async fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = tokio::process::Command::new("git");
    crate::process::hide_tokio_console_window(&mut command);
    command.current_dir(root).env("GIT_OPTIONAL_LOCKS", "0")
        .args(["-c", "core.quotePath=false"]).args(args).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(60), command.output()).await
        .map_err(|_| "Review git command timed out after 60 seconds".to_string())?
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(output.stdout)
}

async fn fallback_base(root: &Path) -> Result<String, String> {
    // Read-only equivalent of detect_main_branch: remote default, local main/master.
    // These discovery calls occur only for legacy agents without a spawn SHA.
    let branch = match git(root, &["symbolic-ref", "refs/remotes/origin/HEAD"]).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).trim()
            .strip_prefix("refs/remotes/origin/").unwrap_or("main").to_string(),
        Err(_) => {
            if git(root, &["show-ref", "--verify", "--quiet", "refs/heads/main"]).await.is_ok() {
                "main".to_string()
            } else if git(root, &["show-ref", "--verify", "--quiet", "refs/heads/master"]).await.is_ok() {
                "master".to_string()
            } else { "main".to_string() }
        }
    };
    let reference = format!("refs/remotes/origin/{branch}");
    let bytes = git(root, &["merge-base", &reference, "HEAD"]).await?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

/// Include committed, index and working-tree changes in one diff, then untracked additions.
pub async fn collect(root: &Path, recorded_base: Option<&str>, agent_id: String) -> Result<ReviewDiff, String> {
    let base = match recorded_base {
        Some(base) if (40..=64).contains(&base.len()) && base.bytes().all(|b| b.is_ascii_hexdigit()) => base.to_string(),
        Some(_) => return Err("Invalid recorded review base SHA".to_string()),
        None => fallback_base(root).await?,
    };
    let patch = git(root, &["diff", "--no-ext-diff", "--no-textconv", "--no-color", "--src-prefix=a/", "--dst-prefix=b/", "--no-renames", "--unified=3", &base, "--", "."]).await?;
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard", "-z"]).await?;
    let mut files = parse_patch(&String::from_utf8_lossy(&patch));
    for name in untracked.split(|b| *b == 0).filter(|name| !name.is_empty()) {
        let path = std::str::from_utf8(name).map_err(|_| "Untracked path is not UTF-8")?;
        let relative = Path::new(path);
        if relative.is_absolute() || relative.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err("Unsafe untracked path".to_string());
        }
        let full = root.join(relative);
        // Never follow an untracked symlink out of the selected worktree.
        let metadata = tokio::fs::symlink_metadata(&full).await.map_err(|e| e.to_string())?;
        let bytes = if metadata.file_type().is_symlink() {
            tokio::fs::read_link(&full).await.map_err(|e| e.to_string())?.to_string_lossy().as_bytes().to_vec()
        } else { tokio::fs::read(&full).await.map_err(|e| e.to_string())? };
        let binary = bytes.contains(&0) || std::str::from_utf8(&bytes).is_err();
        let mut unified = String::new();
        if !binary {
            let content = std::str::from_utf8(&bytes).unwrap();
            let count = content.split_inclusive('\n').count();
            unified = format!("--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{count} @@\n");
            for line in content.split_inclusive('\n') {
                unified.push('+'); unified.push_str(line);
            }
            if !content.is_empty() && !content.ends_with('\n') {
                unified.push_str("\n\\ No newline at end of file\n");
            }
        }
        files.push(ReviewFile { path: path.to_string(), unified, binary, truncated: false });
    }
    let truncated = cap_files(&mut files);
    Ok(ReviewDiff { base, agent_id, files, truncated })
}

fn unquote(path: &str) -> String {
    if !path.starts_with('"') { return path.to_string(); }
    let bytes = path.strip_prefix('"').and_then(|p| p.strip_suffix('"')).unwrap_or(path).as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 1;
            match bytes[i] {
                b'n' => out.push(b'\n'), b'r' => out.push(b'\r'), b't' => out.push(b'\t'),
                b'0'..=b'7' => {
                    let mut value = bytes[i] - b'0';
                    for _ in 0..2 {
                        if i + 1 < bytes.len() && (b'0'..=b'7').contains(&bytes[i + 1]) {
                            i += 1; value = value.wrapping_mul(8).wrapping_add(bytes[i] - b'0');
                        }
                    }
                    out.push(value);
                }
                other => out.push(other),
            }
        } else { out.push(bytes[i]); }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_patch(patch: &str) -> Vec<ReviewFile> {
    let mut files: Vec<ReviewFile> = Vec::new();
    let mut in_hunk = false;
    for line in patch.split_inclusive('\n') {
        if let Some(header) = line.strip_prefix("diff --git ") {
            in_hunk = false;
            let header = header.trim_end_matches('\n');
            let path = if let Some((offset, _)) = header.match_indices(" b/").find(|(offset, _)| {
                header.strip_prefix("a/").map(|left| &left[..offset - 2] == &header[offset + 3..]).unwrap_or(false)
            }) {
                header[offset + 3..].to_string()
            } else if let Some((_, path)) = header.rsplit_once(" \"b/") {
                unquote(&format!("\"{path}"))
            } else { header.to_string() };
            files.push(ReviewFile { path, unified: String::new(), binary: false, truncated: false });
        }
        if line.starts_with("@@") { in_hunk = true; }
        if let Some(file) = files.last_mut() {
            // Extended headers are unambiguous even when a filename contains " b/".
            if let Some(path) = line.strip_prefix("+++ ").filter(|_| !in_hunk) {
                let path = unquote(path.trim_end_matches('\n').trim_end_matches('\t'));
                if path != "/dev/null" { file.path = path.strip_prefix("b/").unwrap_or(&path).to_string(); }
            } else if let Some(path) = line.strip_prefix("--- ").filter(|_| !in_hunk) {
                let path = unquote(path.trim_end_matches('\n').trim_end_matches('\t'));
                if path != "/dev/null" { file.path = path.strip_prefix("a/").unwrap_or(&path).to_string(); }
            }
            if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") { file.binary = true; }
            file.unified.push_str(line);
        }
    }
    for file in &mut files { if file.binary { file.unified.clear(); } }
    files
}

fn cap_files(files: &mut [ReviewFile]) -> bool {
    let mut remaining = TOTAL_LINES;
    let mut truncated = false;
    for file in files {
        let limit = remaining.min(FILE_LINES);
        let mut end = 0;
        let mut count = 0;
        for line in file.unified.split_inclusive('\n').take(limit) { end += line.len(); count += 1; }
        if end < file.unified.len() { file.unified.truncate(end); file.truncated = true; truncated = true; }
        remaining -= count;
    }
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn run(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git").current_dir(root).args(args).output().unwrap();
        assert!(output.status.success(), "{:?}: {}", args, String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    }


    #[test]
    fn hunk_content_that_looks_like_headers_cannot_change_file_path() {
        let files = parse_patch("diff --git a/real.txt b/real.txt\n--- a/real.txt\n+++ b/real.txt\n@@ -1 +1 @@\n--- old content\n+++ new content\n");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "real.txt");
        assert!(files[0].unified.contains("+++ new content"));
    }


    #[test]
    fn quoted_paths_preserve_trailing_quotes_and_real_b_prefixes() {
        assert_eq!(unquote("\"b/name\\\"\""), "b/name\"");
        let files = parse_patch("diff --git \"a/b/a\\t\" \"b/b/a\\t\"\nBinary files differ\n");
        assert_eq!(files[0].path, "b/a\t");
        assert!(files[0].binary);
    }

    fn repository() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-b", "main"]);
        run(dir.path(), &["config", "user.email", "fixture@example.test"]);
        run(dir.path(), &["config", "user.name", "Fixture"]);
        run(dir.path(), &["config", "core.autocrlf", "false"]);
        std::fs::write(dir.path().join("tracked.txt"), "base\n").unwrap();
        run(dir.path(), &["add", "."]);
        run(dir.path(), &["commit", "-m", "base"]);
        let base = run(dir.path(), &["rev-parse", "HEAD"]).trim().to_string();
        (dir, base)
    }

    #[tokio::test]
    async fn committed_only_review_includes_clean_committed_change() {
        let (dir, base) = repository();
        std::fs::write(dir.path().join("tracked.txt"), "committed-only\n").unwrap();
        run(dir.path(), &["commit", "-am", "change"]);
        assert!(run(dir.path(), &["status", "--porcelain"]).is_empty());
        let result = collect(dir.path(), Some(&base), "principal".into()).await.unwrap();
        assert_eq!(result.files.len(), 1);
        assert!(result.files[0].unified.contains("+committed-only"));
    }

    #[tokio::test]
    async fn review_includes_index_worktree_untracked_binary_and_preserves_state() {
        let (dir, base) = repository();
        std::fs::write(dir.path().join("staged.txt"), "staged\n").unwrap();
        run(dir.path(), &["add", "staged.txt"]);
        std::fs::write(dir.path().join("tracked-binary.dat"), [0, 3, 4]).unwrap();
        run(dir.path(), &["add", "tracked-binary.dat"]);
        std::fs::write(dir.path().join("large.txt"), "line\r\n".repeat(2_001)).unwrap();
        std::fs::write(dir.path().join("tracked.txt"), "working\r\n").unwrap();
        std::fs::write(dir.path().join("new file.txt"), "new\r\nlast").unwrap();
        std::fs::write(dir.path().join("binary.dat"), [0, 1, 2]).unwrap();
        let status = run(dir.path(), &["status", "--porcelain"]);
        let refs = run(dir.path(), &["for-each-ref"]);
        let result = collect(dir.path(), Some(&base), "principal".into()).await.unwrap();
        assert!(result.files.iter().any(|f| f.path == "staged.txt" && f.unified.contains("+staged")));
        assert!(result.files.iter().any(|f| f.path == "tracked.txt" && f.unified.contains("+working\r\n")));
        assert!(result.files.iter().any(|f| f.path == "new file.txt" && f.unified.contains("+new\r\n")));
        assert!(result.files.iter().any(|f| f.path == "binary.dat" && f.binary && f.unified.is_empty()));
        assert!(result.files.iter().any(|f| f.path == "tracked-binary.dat" && f.binary && f.unified.is_empty()));
        let large = result.files.iter().find(|f| f.path == "large.txt").unwrap();
        assert!(large.truncated && result.truncated);
        assert_eq!(large.unified.split_inclusive('\n').count(), FILE_LINES);
        assert_eq!(status, run(dir.path(), &["status", "--porcelain"]));
        assert_eq!(refs, run(dir.path(), &["for-each-ref"]));
    }

    #[tokio::test]
    async fn fallback_merge_base_is_read_only() {
        let (dir, base) = repository();
        run(dir.path(), &["update-ref", "refs/remotes/origin/main", &base]);
        run(dir.path(), &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]);
        std::fs::write(dir.path().join("tracked.txt"), "committed\n").unwrap();
        run(dir.path(), &["commit", "-am", "change"]);
        let refs = run(dir.path(), &["for-each-ref"]);
        let status = run(dir.path(), &["status", "--porcelain"]);
        let result = collect(dir.path(), None, "principal".into()).await.unwrap();
        assert_eq!(result.base, base);
        assert_eq!(result.files.len(), 1);
        assert_eq!(refs, run(dir.path(), &["for-each-ref"]));
        assert_eq!(status, run(dir.path(), &["status", "--porcelain"]));
    }

    #[test]
    fn caps_per_file_and_total_keep_explicit_flags() {
        let mut files: Vec<_> = (0..12).map(|i| ReviewFile {
            path: format!("{i}.txt"), unified: "+line\r\n".repeat(2_001), binary: false, truncated: false,
        }).collect();
        assert!(cap_files(&mut files));
        assert_eq!(files[0].unified.lines().count(), FILE_LINES);
        assert_eq!(files.iter().map(|f| f.unified.lines().count()).sum::<usize>(), TOTAL_LINES);
        assert!(files.iter().all(|f| f.truncated));
        assert!(files[0].unified.contains("\r\n"));
    }

    #[test]
    fn quoted_and_ambiguous_paths_use_patch_headers() {
        let patch = "diff --git a/name b/nested b/name b/nested\n--- a/name b/nested\n+++ b/name b/nested\n@@ -1 +1 @@\n-old\n+new\n";
        assert_eq!(parse_patch(patch)[0].path, "name b/nested");
        let patch = "diff --git \"a/a\\tb\\nc\" \"b/a\\tb\\nc\"\n--- \"a/a\\tb\\nc\"\n+++ \"b/a\\tb\\nc\"\n";
        assert_eq!(parse_patch(patch)[0].path, "a\tb\nc");
    }
}
