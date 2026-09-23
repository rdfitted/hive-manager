use serde::Serialize;
#[cfg(not(test))]
use tauri::State;

use crate::coordination::Verdict;
use crate::http::handlers::evaluator::read_qa_verdict_record;
use crate::storage::SessionStorage;

#[cfg(not(test))]
use super::StorageState;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QaCriterion {
    pub id: String,
    pub label: String,
    pub passed: bool,
    pub evidence: Option<String>,
    pub advisory_status: Verdict,
    pub advisory_threshold_disagreement: bool,
    pub unchanged_evidence_flip: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QaVerdict {
    pub session_id: String,
    pub milestone_id: String,
    pub iteration: u8,
    pub passed: bool,
    pub criteria: Vec<QaCriterion>,
    pub summary: String,
    pub timestamp: String,
    pub advisory_verdict: Verdict,
    pub advisory_disagrees: bool,
    pub advisory_threshold_disagreement: bool,
    pub advisory_flip: bool,
}

fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || session_id.contains("..")
        || session_id.contains('/')
        || session_id.contains('\\')
    {
        return Err("Invalid session ID format".to_string());
    }
    Ok(())
}

pub(crate) fn load_qa_verdict(
    storage: &SessionStorage,
    session_id: &str,
) -> Result<Option<QaVerdict>, String> {
    validate_session_id(session_id)?;
    let Some(record) = read_qa_verdict_record(&storage.session_dir(session_id))
        .map_err(|error| format!("Failed to read QA verdict record: {error}"))?
    else {
        return Ok(None);
    };

    let advisory_criteria = record.advisory_criteria;
    Ok(Some(QaVerdict {
        session_id: record.session_id,
        milestone_id: record.milestone_id,
        iteration: record.iteration,
        passed: record.passed,
        criteria: record
            .criteria
            .into_iter()
            .map(|criterion| {
                let advisory = advisory_criteria
                    .iter()
                    .find(|item| item.number == criterion.number);
                QaCriterion {
                    id: criterion.number.to_string(),
                    label: criterion.label,
                    passed: criterion.passed,
                    evidence: (!criterion.evidence.trim().is_empty()).then_some(criterion.evidence),
                    advisory_status: advisory.map_or(Verdict::Undetermined, |item| item.status),
                    advisory_threshold_disagreement: advisory
                        .is_some_and(|item| item.threshold_disagreement),
                    unchanged_evidence_flip: advisory
                        .is_some_and(|item| item.unchanged_evidence_flip),
                }
            })
            .collect(),
        summary: record.summary,
        timestamp: record.timestamp,
        advisory_verdict: record.advisory_verdict,
        advisory_disagrees: record.advisory_disagrees,
        advisory_threshold_disagreement: record.advisory_threshold_disagreement,
        advisory_flip: record.advisory_flip,
    }))
}

#[cfg(not(test))]
#[tauri::command]
pub fn get_qa_verdict(
    state: State<'_, StorageState>,
    session_id: String,
) -> Result<Option<QaVerdict>, String> {
    load_qa_verdict(&state.0, &session_id)
}

#[cfg(test)]
mod tests {
    use super::load_qa_verdict;
    use crate::coordination::CriterionKind;
    use crate::http::handlers::evaluator::{
        write_qa_verdict_record, CriterionSubmissionResult, QaCriterionVerdictRecord,
        QaVerdictRecord,
    };
    use crate::storage::SessionStorage;
    use tempfile::TempDir;

    #[test]
    fn returns_null_when_no_verdict_record_exists() {
        let temp = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();

        assert_eq!(load_qa_verdict(&storage, "missing-session").unwrap(), None);
    }

    #[test]
    fn projects_the_stored_record_to_the_panel_shape() {
        let temp = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(temp.path().to_path_buf()).unwrap();
        let session_id = "qa-panel-session";
        let record = QaVerdictRecord {
            schema_version: 1,
            session_id: session_id.to_string(),
            milestone_id: "Typed QA".to_string(),
            iteration: 2,
            verdict: "FAIL".to_string(),
            passed: false,
            summary: "One criterion needs work".to_string(),
            timestamp: "2026-09-22T22:00:00.000Z".to_string(),
            contract_path: Some("contracts/milestone-1.md".to_string()),
            contract_typed: true,
            omission: None,
            criteria: vec![
                QaCriterionVerdictRecord {
                    number: 1,
                    label: "Primary flow completes".to_string(),
                    kind: CriterionKind::PassFail,
                    result: CriterionSubmissionResult::Pass,
                    passed: true,
                    evidence: "API worker observed completion".to_string(),
                    evidence_refs: vec!["qa-worker-api#1".to_string()],
                    decision_id: Some("decision-1".to_string()),
                },
                QaCriterionVerdictRecord {
                    number: 2,
                    label: "Failure is visible".to_string(),
                    kind: CriterionKind::PassFail,
                    result: CriterionSubmissionResult::Fail,
                    passed: false,
                    evidence: String::new(),
                    evidence_refs: Vec::new(),
                    decision_id: Some("decision-2".to_string()),
                },
            ],
            advisory_verdict: crate::coordination::Verdict::Fail,
            advisory_disagrees: false,
            advisory_threshold_disagreement: false,
            advisory_flip: false,
            advisory_criteria: Vec::new(),
        };
        write_qa_verdict_record(&storage.session_dir(session_id), &record).unwrap();

        let verdict = load_qa_verdict(&storage, session_id).unwrap().unwrap();

        assert_eq!(verdict.session_id, session_id);
        assert_eq!(verdict.milestone_id, "Typed QA");
        assert_eq!(verdict.iteration, 2);
        assert!(!verdict.passed);
        assert_eq!(verdict.summary, "One criterion needs work");
        assert_eq!(verdict.advisory_verdict, crate::coordination::Verdict::Fail);
        assert!(!verdict.advisory_disagrees);
        assert_eq!(verdict.criteria.len(), 2);
        assert_eq!(verdict.criteria[0].id, "1");
        assert_eq!(
            verdict.criteria[0].evidence.as_deref(),
            Some("API worker observed completion")
        );
        assert_eq!(verdict.criteria[1].id, "2");
        assert_eq!(verdict.criteria[1].evidence, None);
    }

    #[test]
    fn legacy_record_without_advisory_fields_deserializes() {
        let value = serde_json::json!({
            "schema_version": 1,
            "session_id": "legacy",
            "milestone_id": "Legacy",
            "iteration": 1,
            "verdict": "PASS",
            "passed": true,
            "summary": "Legacy result",
            "timestamp": "2026-09-22T22:00:00.000Z",
            "contract_path": null,
            "contract_typed": false,
            "omission": null,
            "criteria": []
        });
        let record: QaVerdictRecord = serde_json::from_value(value).unwrap();
        assert_eq!(record.advisory_verdict, crate::coordination::Verdict::Undetermined);
        assert!(!record.advisory_disagrees);
        assert!(record.advisory_criteria.is_empty());
    }
}
