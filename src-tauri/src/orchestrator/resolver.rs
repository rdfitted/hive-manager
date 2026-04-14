use std::time::{Duration, Instant};
use std::sync::Arc;

use thiserror::Error;

use crate::{
    artifacts::{collector::ArtifactCollector, resolver_input::{assemble_resolver_input, ResolverInput}},
    domain::ResolverOutput,
    events::{EventBus, EventEmitter},
    session::cell_status::RESOLVER_CELL_ID,
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
    #[error("Resolver launch omitted candidate artifacts: requested {requested}, assembled {assembled}")]
    IncompleteCandidates { requested: usize, assembled: usize },
    #[error("Timed out waiting for candidate artifacts")]
    Timeout,
}

pub struct Resolver {
    storage: SessionStorage,
    #[allow(dead_code)]
    artifacts_collector: ArtifactCollector,
    template_engine: TemplateEngine,
    event_emitter: Option<EventEmitter>,
}

impl Resolver {
    pub fn new(storage: SessionStorage) -> Self {
        Self::new_with_optional_emitter(storage, None)
    }

    pub fn new_with_event_bus(storage: SessionStorage, event_bus: Arc<EventBus>) -> Self {
        Self::new_with_optional_emitter(storage, Some(EventEmitter::new(event_bus)))
    }

    fn new_with_optional_emitter(
        storage: SessionStorage,
        event_emitter: Option<EventEmitter>,
    ) -> Self {
        let templates_dir = storage.templates_dir();
        let artifacts_collector = ArtifactCollector::new(SessionStorage::new_with_base(
            storage.base_dir().clone(),
        )
        .expect("resolver artifact storage initialization failed"));

        Self {
            storage,
            artifacts_collector,
            template_engine: TemplateEngine::new(templates_dir),
            event_emitter,
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
            let mut available = Vec::new();
            let mut errors = Vec::new();

            for cell_id in candidate_ids {
                match self.storage.load_artifact(session_id, cell_id) {
                    Ok(Some(_)) => available.push(cell_id.clone()),
                    Ok(None) => {}
                    Err(error) => errors.push(format!("{cell_id}: {error}")),
                }
            }

            if !errors.is_empty() {
                return Err(ResolverError::Storage(StorageError::InvalidPath(format!(
                    "Failed to load candidate artifacts: {}",
                    errors.join("; ")
                ))));
            }

            if available.len() == candidate_ids.len() {
                return Ok(available);
            }

            if started.elapsed() >= timeout {
                return Err(ResolverError::Timeout);
            }

            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(ResolverError::Timeout);
            }

            std::thread::sleep(Duration::from_millis(250).min(remaining));
        }
    }

    pub fn launch(
        &self,
        session_id: &str,
        candidates: Vec<String>,
    ) -> Result<ResolverOutput, ResolverError> {
        let requested_candidates = candidates.len();
        let resolver_input = assemble_resolver_input(&self.storage, session_id, candidates)?;

        if resolver_input.candidates.len() != requested_candidates {
            return Err(ResolverError::IncompleteCandidates {
                requested: requested_candidates,
                assembled: resolver_input.candidates.len(),
            });
        }

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
        if let Some(emitter) = self.event_emitter.clone() {
            let session_id = session_id.to_string();
            let selected_candidate = output.selected_candidate.clone();
            let rationale = output.rationale.clone();
            tokio::spawn(async move {
                if let Err(error) = emitter
                    .emit_resolver_selected_candidate(
                        &session_id,
                        RESOLVER_CELL_ID,
                        &selected_candidate,
                        &rationale,
                    )
                    .await
                {
                    tracing::debug!("Failed to emit resolver selected candidate event: {}", error);
                }
            });
        }
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
    use std::time::Duration;

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

    #[test]
    fn wait_for_candidates_requires_all_candidates() {
        let temp = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();
        storage.create_session_dir("resolver-session").unwrap();
        storage
            .save_artifact(
                "resolver-session",
                "variant-a",
                &ArtifactBundle {
                    summary: Some("Variant A".to_string()),
                    changed_files: vec![],
                    commits: vec![],
                    branch: "fusion/a".to_string(),
                    test_results: None,
                    diff_summary: None,
                    unresolved_issues: vec![],
                    confidence: Some(0.5),
                    recommended_next_step: None,
                },
            )
            .unwrap();

        let resolver = Resolver::new(storage);
        let err = resolver
            .wait_for_candidates(
                "resolver-session",
                &["variant-a".to_string(), "variant-b".to_string()],
                Duration::from_millis(10),
            )
            .unwrap_err();

        assert!(matches!(err, super::ResolverError::Timeout));
    }

    #[test]
    fn wait_for_candidates_with_zero_timeout_returns_immediately() {
        let temp = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();
        storage.create_session_dir("resolver-session").unwrap();

        let resolver = Resolver::new(storage);
        let err = resolver
            .wait_for_candidates(
                "resolver-session",
                &["variant-a".to_string()],
                Duration::ZERO,
            )
            .unwrap_err();

        assert!(matches!(err, super::ResolverError::Timeout));
    }

    #[test]
    fn launch_rejects_partial_candidate_set() {
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
                    unresolved_issues: vec![],
                    confidence: Some(0.5),
                    recommended_next_step: None,
                },
            )
            .unwrap();

        let resolver = Resolver::new(storage);
        let err = resolver
            .launch(
                "resolver-session",
                vec!["variant-a".to_string(), "variant-b".to_string()],
            )
            .unwrap_err();

        assert!(matches!(
            err,
            super::ResolverError::IncompleteCandidates {
                requested: 2,
                assembled: 1,
            }
        ));
    }

    #[test]
    fn wait_for_candidates_returns_storage_errors() {
        let temp = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();
        storage.create_session_dir("resolver-session").unwrap();
        let artifacts_dir = temp
            .path()
            .join("sessions")
            .join("resolver-session")
            .join("artifacts");
        std::fs::create_dir_all(&artifacts_dir).unwrap();
        std::fs::write(artifacts_dir.join("variant-a.json"), "{not-json}").unwrap();

        let resolver = Resolver::new(storage);
        let err = resolver
            .wait_for_candidates(
                "resolver-session",
                &["variant-a".to_string()],
                Duration::from_millis(10),
            )
            .unwrap_err();

        assert!(matches!(err, super::ResolverError::Storage(_)));
    }
}
