use std::path::Path;
use std::process::Command;

use serde_json::json;

use crate::{
    domain::ArtifactBundle,
    storage::{SessionStorage, StorageError},
};

pub struct ArtifactCollector {
    storage: SessionStorage,
}

impl ArtifactCollector {
    pub fn new(storage: SessionStorage) -> Self {
        Self { storage }
    }

    pub fn collect_from_worktree(worktree_path: &Path) -> Result<ArtifactBundle, StorageError> {
        if !worktree_path.exists() {
            return Err(StorageError::InvalidPath(format!(
                "Worktree path does not exist: {}",
                worktree_path.display()
            )));
        }

        let branch = run_git(worktree_path, &["branch", "--show-current"]).unwrap_or_default();
        let commits = run_git_lines(worktree_path, &["log", "--oneline", "-10"]);
        let changed_files = run_git_lines(worktree_path, &["diff", "--name-only", "--", "."]);
        let diff_summary = run_git(worktree_path, &["diff", "--stat", "--", "."]);
        let test_results = detect_test_results(worktree_path)?;
        let summary = Some(build_summary(&branch, &changed_files, &commits));
        let confidence = Some(estimate_confidence(&changed_files, &test_results));

        Ok(ArtifactBundle {
            summary,
            changed_files,
            commits,
            branch,
            test_results,
            diff_summary,
            unresolved_issues: vec![],
            confidence,
            recommended_next_step: Some("Review candidate output in Resolver comparison".to_string()),
        })
    }

    pub fn persist_artifact(
        &self,
        session_id: &str,
        cell_id: &str,
        bundle: &ArtifactBundle,
    ) -> Result<(), StorageError> {
        self.storage.save_artifact(session_id, cell_id, bundle)
    }

    pub fn load_artifact(
        &self,
        session_id: &str,
        cell_id: &str,
    ) -> Result<Option<ArtifactBundle>, StorageError> {
        self.storage.load_artifact(session_id, cell_id)
    }
}

impl Default for ArtifactCollector {
    fn default() -> Self {
        Self::new(SessionStorage::new().expect("artifact collector storage initialization failed"))
    }
}

fn run_git(worktree_path: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        None
    } else {
        Some(stdout)
    }
}

fn run_git_lines(worktree_path: &Path, args: &[&str]) -> Vec<String> {
    run_git(worktree_path, args)
        .map(|output| {
            output
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn detect_test_results(worktree_path: &Path) -> Result<Option<serde_json::Value>, StorageError> {
    let candidates = [
        "test-results.json",
        "test-results.txt",
        "test-output.txt",
        "junit.xml",
        "pytest-report.txt",
    ];

    for relative_path in candidates {
        let path = worktree_path.join(relative_path);
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let snippet = content.lines().take(20).collect::<Vec<_>>().join("\n");
            return Ok(Some(json!({
                "source": relative_path,
                "snippet": snippet,
            })));
        }
    }

    Ok(None)
}

fn build_summary(branch: &str, changed_files: &[String], commits: &[String]) -> String {
    let branch_label = if branch.trim().is_empty() {
        "detached HEAD".to_string()
    } else {
        branch.to_string()
    };
    format!(
        "{} changed file(s) on {} with {} recent commit(s)",
        changed_files.len(),
        branch_label,
        commits.len()
    )
}

fn estimate_confidence(
    changed_files: &[String],
    test_results: &Option<serde_json::Value>,
) -> f32 {
    let mut confidence: f32 = if changed_files.is_empty() { 0.35 } else { 0.6 };
    if test_results.is_some() {
        confidence += 0.2;
    }
    if changed_files.len() <= 5 {
        confidence += 0.1;
    }
    confidence.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use tempfile::TempDir;

    use super::ArtifactCollector;
    use crate::storage::SessionStorage;

    fn init_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        let path = dir.path();

        Command::new("git").args(["init"]).current_dir(path).output().unwrap();
        Command::new("git")
            .args(["config", "user.email", "worker1@example.com"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Worker One"])
            .current_dir(path)
            .output()
            .unwrap();

        fs::write(path.join("README.md"), "hello\n").unwrap();
        Command::new("git")
            .args(["add", "README.md"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .current_dir(path)
            .output()
            .unwrap();

        fs::write(path.join("README.md"), "hello\nworld\n").unwrap();
        fs::write(path.join("test-results.txt"), "ok: 3 passed\n").unwrap();

        dir
    }

    #[test]
    fn collects_expected_git_and_test_artifacts() {
        let repo = init_repo();
        let bundle = ArtifactCollector::collect_from_worktree(repo.path()).unwrap();

        assert!(bundle.summary.is_some());
        assert!(!bundle.branch.is_empty());
        assert!(!bundle.commits.is_empty());
        assert!(bundle.changed_files.iter().any(|file| file == "README.md"));
        assert!(bundle.test_results.is_some());
        assert!(bundle.confidence.unwrap() > 0.0);
    }

    #[test]
    fn persists_and_loads_artifacts() {
        let repo = init_repo();
        let bundle = ArtifactCollector::collect_from_worktree(repo.path()).unwrap();

        let storage_root = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(storage_root.path().to_path_buf()).unwrap();
        let collector = ArtifactCollector::new(storage);

        collector
            .persist_artifact("session-a", "cell-a", &bundle)
            .unwrap();

        let loaded = collector.load_artifact("session-a", "cell-a").unwrap();
        assert_eq!(loaded, Some(bundle));
    }
}
