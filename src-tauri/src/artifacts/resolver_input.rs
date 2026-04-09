use serde::{Deserialize, Serialize};

use crate::{
    domain::ArtifactBundle,
    storage::{SessionStorage, StorageError},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolverInput {
    pub queen_summary: Option<String>,
    pub candidates: Vec<CandidateInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateInput {
    pub cell_id: String,
    pub summary: Option<String>,
    pub changed_files: Vec<String>,
    pub commits: Vec<String>,
    pub branch: String,
    pub diff_summary: Option<String>,
    pub test_results: Option<serde_json::Value>,
    pub unresolved_issues: Vec<String>,
    pub confidence: Option<f32>,
}

pub fn assemble_resolver_input(
    storage: &SessionStorage,
    session_id: &str,
    cell_ids: Vec<String>,
) -> Result<ResolverInput, StorageError> {
    let mut candidates = Vec::new();

    for cell_id in cell_ids {
        if let Some(bundle) = storage.load_artifact(session_id, &cell_id)? {
            candidates.push(map_candidate(cell_id, bundle));
        }
    }

    Ok(ResolverInput {
        queen_summary: storage.read_latest_conversation_message(session_id, "queen")?,
        candidates,
    })
}

fn map_candidate(cell_id: String, bundle: ArtifactBundle) -> CandidateInput {
    CandidateInput {
        cell_id,
        summary: bundle.summary,
        changed_files: bundle.changed_files,
        commits: bundle.commits,
        branch: bundle.branch,
        diff_summary: bundle.diff_summary,
        test_results: bundle.test_results,
        unresolved_issues: bundle.unresolved_issues,
        confidence: bundle.confidence,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::assemble_resolver_input;
    use crate::{
        domain::ArtifactBundle,
        storage::SessionStorage,
    };

    #[test]
    fn assembles_candidates_and_queen_summary() {
        let storage_root = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(storage_root.path().to_path_buf()).unwrap();
        storage.create_session_dir("session-b").unwrap();
        std::fs::write(
            storage
                .session_dir("session-b")
                .join("conversations")
                .join("queen.md"),
            "---\n[2026-04-08T23:30:00Z] from @queen\nPick the safer variant\n\n",
        )
        .unwrap();

        storage
            .save_artifact(
                "session-b",
                "variant-a",
                &ArtifactBundle {
                    summary: Some("Variant A summary".to_string()),
                    changed_files: vec!["src/a.rs".to_string()],
                    commits: vec!["abc123 initial".to_string()],
                    branch: "fusion/a".to_string(),
                    test_results: None,
                    diff_summary: Some("1 file changed".to_string()),
                    unresolved_issues: vec!["needs more tests".to_string()],
                    confidence: Some(0.7),
                    recommended_next_step: None,
                },
            )
            .unwrap();

        let input = assemble_resolver_input(
            &storage,
            "session-b",
            vec!["variant-a".to_string(), "missing".to_string()],
        )
        .unwrap();

        assert_eq!(input.queen_summary.as_deref(), Some("Pick the safer variant"));
        assert_eq!(input.candidates.len(), 1);
        assert_eq!(input.candidates[0].cell_id, "variant-a");
    }
}
