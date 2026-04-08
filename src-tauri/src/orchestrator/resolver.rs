use std::time::{Duration, Instant};

use thiserror::Error;

use crate::{
    artifacts::{collector::ArtifactCollector, resolver_input::{assemble_resolver_input, ResolverInput}},
    domain::ResolverOutput,
    storage::{SessionStorage, StorageError},
    templates::{PromptContext, TemplateEngine},
};

#[derive(Debug, Error)]
pub enum ResolverError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Template(#[from] crate::templates::TemplateError),
    #[error("No candidate artifacts available for resolver launch")]
    NoCandidates,
    #[error("Timed out waiting for candidate artifacts")]
    Timeout,
}

pub struct Resolver {
    storage: SessionStorage,
    #[allow(dead_code)]
    artifacts_collector: ArtifactCollector,
    template_engine: TemplateEngine,
}

impl Resolver {
    pub fn new(storage: SessionStorage) -> Self {
        let templates_dir = storage.templates_dir();
        let artifacts_collector = ArtifactCollector::new(SessionStorage::new_with_base(
            storage.base_dir().clone(),
        )
        .expect("resolver artifact storage initialization failed"));

        Self {
            storage,
            artifacts_collector,
            template_engine: TemplateEngine::new(templates_dir),
        }
    }

    pub fn wait_for_candidates(
        &self,
        session_id: &str,
        candidate_ids: &[String],
        timeout: Duration,
    ) -> Result<Vec<String>, ResolverError> {
        let started = Instant::now();

        loop {
            let available = candidate_ids
                .iter()
                .filter_map(|cell_id| {
                    self.storage
                        .load_artifact(session_id, cell_id)
                        .ok()
                        .flatten()
                        .map(|_| cell_id.clone())
                })
                .collect::<Vec<_>>();

            if !available.is_empty() {
                return Ok(available);
            }

            if started.elapsed() >= timeout {
                return Err(ResolverError::Timeout);
            }

            std::thread::sleep(Duration::from_millis(250));
        }
    }

    pub fn launch(
        &self,
        session_id: &str,
        candidates: Vec<String>,
    ) -> Result<ResolverOutput, ResolverError> {
        let resolver_input = assemble_resolver_input(&self.storage, session_id, candidates)?;
        if resolver_input.candidates.is_empty() {
            return Err(ResolverError::NoCandidates);
        }

        let _prompt = self.render_prompt(session_id, &resolver_input)?;
        Ok(select_best_candidate(resolver_input))
    }

    pub fn persist_output(
        &self,
        session_id: &str,
        output: &ResolverOutput,
    ) -> Result<(), ResolverError> {
        self.storage.save_resolver_output(session_id, output)?;
        Ok(())
    }

    fn render_prompt(
        &self,
        session_id: &str,
        resolver_input: &ResolverInput,
    ) -> Result<String, ResolverError> {
        let mut context = PromptContext {
            session_id: session_id.to_string(),
            project_path: String::new(),
            task: Some("Resolve candidate implementations".to_string()),
            variables: std::collections::HashMap::new(),
        };
        context.variables.insert(
            "queen_summary".to_string(),
            resolver_input
                .queen_summary
                .clone()
                .unwrap_or_else(|| "No queen summary available.".to_string()),
        );
        context.variables.insert(
            "candidates_json".to_string(),
            serde_json::to_string_pretty(&resolver_input.candidates)
                .unwrap_or_else(|_| "[]".to_string()),
        );

        Ok(self.template_engine.render_resolver_prompt(&context)?)
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new(SessionStorage::new().expect("resolver storage initialization failed"))
    }
}

fn select_best_candidate(input: ResolverInput) -> ResolverOutput {
    let best = input
        .candidates
        .iter()
        .max_by(|left, right| {
            score_candidate(left)
                .partial_cmp(&score_candidate(right))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("resolver candidates should be non-empty");

    ResolverOutput {
        selected_candidate: best.cell_id.clone(),
        rationale: format!(
            "Selected {} based on artifact confidence, commit history, and unresolved issue count.",
            best.cell_id
        ),
        tradeoffs: vec![
            format!("{} changed {} file(s)", best.cell_id, best.changed_files.len()),
            format!("{} reported {} unresolved issue(s)", best.cell_id, best.unresolved_issues.len()),
        ],
        hybrid_integration_plan: None,
        final_recommendation: best.summary.clone(),
    }
}

fn score_candidate(candidate: &crate::artifacts::resolver_input::CandidateInput) -> f32 {
    candidate.confidence.unwrap_or(0.0)
        + (candidate.commits.len() as f32 * 0.05)
        + (candidate.changed_files.len() as f32 * 0.02)
        - (candidate.unresolved_issues.len() as f32 * 0.1)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::Resolver;
    use crate::{domain::ArtifactBundle, storage::SessionStorage};

    #[test]
    fn launch_selects_best_candidate_from_artifacts() {
        let temp = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();
        storage.create_session_dir("resolver-session").unwrap();
        storage
            .save_artifact(
                "resolver-session",
                "variant-a",
                &ArtifactBundle {
                    summary: Some("Variant A".to_string()),
                    changed_files: vec!["src/a.rs".to_string()],
                    commits: vec!["a1".to_string()],
                    branch: "fusion/a".to_string(),
                    test_results: None,
                    diff_summary: None,
                    unresolved_issues: vec!["todo".to_string()],
                    confidence: Some(0.5),
                    recommended_next_step: None,
                },
            )
            .unwrap();
        storage
            .save_artifact(
                "resolver-session",
                "variant-b",
                &ArtifactBundle {
                    summary: Some("Variant B".to_string()),
                    changed_files: vec!["src/b.rs".to_string()],
                    commits: vec!["b1".to_string(), "b2".to_string()],
                    branch: "fusion/b".to_string(),
                    test_results: None,
                    diff_summary: None,
                    unresolved_issues: vec![],
                    confidence: Some(0.9),
                    recommended_next_step: None,
                },
            )
            .unwrap();

        let resolver = Resolver::new(storage);
        let output = resolver
            .launch(
                "resolver-session",
                vec!["variant-a".to_string(), "variant-b".to_string()],
            )
            .unwrap();

        assert_eq!(output.selected_candidate, "variant-b");
        assert!(output.rationale.contains("variant-b"));
        assert_eq!(output.tradeoffs.len(), 2);
        assert!(
            output
                .tradeoffs
                .iter()
                .any(|tradeoff| tradeoff.contains("1 file(s)"))
        );
        assert!(
            output
                .tradeoffs
                .iter()
                .any(|tradeoff| tradeoff.contains("0 unresolved issue(s)"))
        );
        assert_eq!(output.hybrid_integration_plan, None);
        assert_eq!(output.final_recommendation, Some("Variant B".to_string()));
    }
}
