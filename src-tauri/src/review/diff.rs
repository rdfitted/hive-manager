//! Read-only review collection. A recorded spawn SHA avoids all base-discovery calls.
use std::{io, path::Path, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt};
use serde::Serialize;

const FILE_LINES: usize = 2_000;
const TOTAL_LINES: usize = 20_000;
const UNTRACKED_BYTES: usize = 1_048_576;

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
    let bytes = match git(root, &["merge-base", &reference, "HEAD"]).await {
        Ok(bytes) => bytes,
        Err(remote_error) => {
            let local = format!("refs/heads/{branch}");
            git(root, &["merge-base", &local, "HEAD"]).await.map_err(|local_error|
                format!("No review merge-base for {reference} or {local}: remote: {remote_error}; local: {local_error}"))?
        }
    };
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

/// Include committed, index and working-tree changes in one diff, then untracked additions.
pub async fn collect(root: &Path, recorded_base: Option<&str>, agent_id: String) -> Result<ReviewDiff, String> {
    collect_with_loader(root, recorded_base, agent_id, |path, full| async move {
        load_untracked(&path, &full).await
    }).await
}

async fn collect_with_loader<F, Fut>(root: &Path, recorded_base: Option<&str>, agent_id: String, load: F) -> Result<ReviewDiff, String>
where
    F: Fn(String, std::path::PathBuf) -> Fut,
    Fut: std::future::Future<Output = io::Result<ReviewFile>>,
{
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
        match load(path.to_string(), root.join(relative)).await {
            Ok(file) => files.push(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("Cannot read untracked {path}: {error}")),
        }
    }
    let truncated = cap_files(&mut files);
    Ok(ReviewDiff { base, agent_id, files, truncated })
}

async fn bounded_bytes(reader: impl AsyncRead + Unpin) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take((UNTRACKED_BYTES + 1) as u64).read_to_end(&mut bytes).await?;
    Ok(bytes)
}

async fn load_untracked(path: &str, full: &Path) -> io::Result<ReviewFile> {
    // Never follow an untracked symlink out of the selected worktree.
    let metadata = tokio::fs::symlink_metadata(full).await?;
    let bytes = if metadata.file_type().is_symlink() {
        tokio::fs::read_link(full).await?.to_string_lossy().as_bytes().to_vec()
    } else {
        bounded_bytes(tokio::fs::File::open(full).await?).await?
    };
    Ok(untracked_file(path, bytes))
}

fn untracked_file(path: &str, bytes: Vec<u8>) -> ReviewFile {
    if bytes.len() > UNTRACKED_BYTES {
        // Incomplete bytes cannot establish binary status; false means unclassified here.
        return ReviewFile { path: path.into(), unified: String::new(), binary: false, truncated: true };
    }
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
    ReviewFile { path: path.into(), unified, binary, truncated: false }
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
    let mut truncated = files.iter().any(|file| file.truncated);
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
    #[tokio::test]
    async fn local_only_main_and_master_are_read_only() {
        for branch in ["main", "master"] {
            let (dir, base) = repository();
            if branch == "master" { run(dir.path(), &["branch", "-m", "master"]); }
            run(dir.path(), &["checkout", "-b", "worker"]);
            std::fs::write(dir.path().join("tracked.txt"), "committed\n").unwrap();
            run(dir.path(), &["commit", "-am", "worker"]);
            let refs = run(dir.path(), &["for-each-ref"]);
            let status = run(dir.path(), &["status", "--porcelain"]);
            let index = std::fs::read(dir.path().join(".git/index")).unwrap();
            let result = collect(dir.path(), None, "principal".into()).await.unwrap();
            assert_eq!(result.base, base);
            assert_eq!(refs, run(dir.path(), &["for-each-ref"]));
            assert_eq!(status, run(dir.path(), &["status", "--porcelain"]));
            assert_eq!(index, std::fs::read(dir.path().join(".git/index")).unwrap());
        }
    }

    #[tokio::test]
    async fn untracked_byte_cap_keeps_paths_and_aggregate_flag() {
        let (dir, base) = repository();
        std::fs::write(dir.path().join("huge.txt"), vec![b'x'; 1_048_577]).unwrap();
        std::fs::write(dir.path().join("huge.bin"), vec![0; 1_048_577]).unwrap();
        std::fs::write(dir.path().join("exact.txt"), vec![b'x'; 1_048_576]).unwrap();
        let result = collect(dir.path(), Some(&base), "principal".into()).await.unwrap();
        for path in ["huge.txt", "huge.bin"] {
            let file = result.files.iter().find(|f| f.path == path).unwrap();
            assert!(file.truncated, "{path} must be byte-truncated");
            assert!(file.unified.is_empty());
        }
        assert!(result.truncated);
        let exact = result.files.iter().find(|f| f.path == "exact.txt").unwrap();
        assert!(!exact.truncated && !exact.binary);
        assert!(exact.unified.contains(&"x".repeat(1_048_576)));
    }

    #[tokio::test]
    async fn disappeared_untracked_entries_preserve_tracked_changes_and_other_errors() {
        let (dir, base) = repository();
        std::fs::write(dir.path().join("tracked.txt"), "survives\n").unwrap();
        std::fs::write(dir.path().join("gone.txt"), "enumerated\n").unwrap();
        // Inject the entry boundary: NotFound may originate at metadata, read_link, open or read.
        for stage in ["metadata", "read_link", "open", "read"] {
            let result = collect_with_loader(dir.path(), Some(&base), "principal".into(), |_, _| async move {
                Err(io::Error::new(io::ErrorKind::NotFound, stage))
            }).await.unwrap();
            assert_eq!(result.files.len(), 1);
            assert!(result.files[0].unified.contains("+survives"));
        }
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::Other] {
            let error = collect_with_loader(dir.path(), Some(&base), "principal".into(), |_, _| async move {
                Err(io::Error::new(kind, "injected entry failure"))
            }).await.unwrap_err();
            assert!(error.contains("injected entry failure"));
        }
        assert_eq!(load_untracked("missing", &dir.path().join("missing")).await.unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn bounded_reader_limits_consumption_even_when_metadata_is_outdated() {
        // Cursor is larger than any earlier metadata size; the read must enforce the cap itself.
        let mut reader = std::io::Cursor::new(vec![b'x'; UNTRACKED_BYTES * 3]);
        let bytes = bounded_bytes(&mut reader).await.unwrap();
        assert_eq!(bytes.len(), UNTRACKED_BYTES + 1);
        assert_eq!(reader.position(), (UNTRACKED_BYTES + 1) as u64);
        assert!(untracked_file("growing", bytes).truncated);
        assert_eq!(bounded_bytes(tokio::io::empty()).await.unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn byte_truncation_survives_line_caps_and_regular_content_is_preserved() {
        let mut files = vec![untracked_file("huge", vec![b'x'; UNTRACKED_BYTES + 1])];
        assert!(cap_files(&mut files));
        for bytes in [b"plain\n".to_vec(), b"windows\r\nlast".to_vec()] {
            let file = untracked_file("text", bytes.clone());
            assert!(!file.binary && !file.truncated);
            assert!(file.unified.contains(std::str::from_utf8(&bytes).unwrap().split('\n').next().unwrap()));
        }
        for bytes in [vec![0, 1], vec![255]] {
            let file = untracked_file("binary", bytes);
            assert!(file.binary && file.unified.is_empty() && !file.truncated);
        }
    }

    #[tokio::test]
    async fn remote_merge_base_wins_and_missing_bases_report_context() {
        let (dir, base) = repository();
        run(dir.path(), &["update-ref", "refs/remotes/origin/main", &base]);
        run(dir.path(), &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]);
        std::fs::write(dir.path().join("tracked.txt"), "local ahead\n").unwrap();
        run(dir.path(), &["commit", "-am", "local"]);
        run(dir.path(), &["checkout", "-b", "worker"]);
        let refs = run(dir.path(), &["for-each-ref"]);
        let status = run(dir.path(), &["status", "--porcelain"]);
        let index = std::fs::read(dir.path().join(".git/index")).unwrap();
        assert_eq!(fallback_base(dir.path()).await.unwrap(), base);
        assert_eq!(refs, run(dir.path(), &["for-each-ref"]));
        assert_eq!(status, run(dir.path(), &["status", "--porcelain"]));
        assert_eq!(index, std::fs::read(dir.path().join(".git/index")).unwrap());
        run(dir.path(), &["update-ref", "-d", "refs/remotes/origin/main"]);
        run(dir.path(), &["branch", "-D", "main"]);
        let refs = run(dir.path(), &["for-each-ref"]);
        let error = fallback_base(dir.path()).await.unwrap_err();
        assert!(error.contains("refs/remotes/origin/main") && error.contains("refs/heads/main"));
        assert_eq!(refs, run(dir.path(), &["for-each-ref"]));
        assert_eq!(status, run(dir.path(), &["status", "--porcelain"]));
        assert_eq!(index, std::fs::read(dir.path().join(".git/index")).unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_representation_is_target_not_target_content() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("target"), "secret content").unwrap();
        std::os::unix::fs::symlink("target", dir.path().join("link")).unwrap();
        let file = load_untracked("link", &dir.path().join("link")).await.unwrap();
        assert!(file.unified.contains("+target"));
        assert!(!file.unified.contains("secret content"));
    }

}
