use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::evidence::{scrub, scrub_value, write_evidence};

const SCHEMA_VERSION: u8 = 1;

const DECISION_CORE_FIELDS: &[&str] = &[
    "v",
    "kind",
    "ts",
    "decision_id",
    "surface",
    "subject_ref",
    "state_hash",
    "state_ref",
    "question_id",
    "question_version",
    "judge",
    "model",
    "sampling",
    "mode",
    "answer",
    "probabilities",
    "confidence",
    "threshold_id",
    "routed",
    "latency_ms",
    "cost_usd",
    "error",
];

const OUTCOME_CORE_FIELDS: &[&str] = &["v", "kind", "ts", "decision_id", "label", "source", "note"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Judge {
    Jev,
    IncumbentLlm,
    Code,
    Human,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JudgmentMode {
    Shadow,
    Advisory,
    Gating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum OutcomeSource {
    UserAnswer,
    OperatorOverride,
    HumanLabel,
    ModelAck,
    Downstream,
}

#[derive(Debug, Clone)]
pub(crate) struct DecisionInput {
    pub decision_id: Option<String>,
    pub surface: String,
    pub subject_ref: Map<String, Value>,
    pub observations: Value,
    pub answer: Value,
    pub question_id: Option<String>,
    pub question_version: Option<String>,
    pub judge: Judge,
    pub model: Option<String>,
    pub sampling: Option<Map<String, Value>>,
    pub mode: JudgmentMode,
    pub probabilities: Option<Map<String, Value>>,
    pub confidence: Option<f64>,
    pub threshold_id: Option<String>,
    pub routed: String,
    pub latency_ms: Option<u64>,
    pub cost_usd: Option<f64>,
    pub error: Option<String>,
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OutcomeInput {
    pub decision_id: String,
    pub label: Value,
    pub source: OutcomeSource,
    pub note: Option<String>,
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DecisionRecord {
    pub v: u8,
    pub kind: String,
    pub ts: String,
    pub decision_id: String,
    pub surface: String,
    pub subject_ref: Map<String, Value>,
    pub state_hash: Option<String>,
    pub state_ref: Option<String>,
    pub question_id: Option<String>,
    pub question_version: Option<String>,
    pub judge: Judge,
    pub model: Option<String>,
    pub sampling: Option<Map<String, Value>>,
    pub mode: JudgmentMode,
    pub answer: Value,
    pub probabilities: Option<Map<String, Value>>,
    pub confidence: Option<f64>,
    pub threshold_id: Option<String>,
    pub routed: String,
    pub latency_ms: Option<u64>,
    pub cost_usd: Option<f64>,
    pub error: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct OutcomeRecord {
    pub v: u8,
    pub kind: String,
    pub ts: String,
    pub decision_id: String,
    pub label: Value,
    pub source: OutcomeSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct JudgmentLedger {
    ledger_path: PathBuf,
}

impl JudgmentLedger {
    pub(crate) fn new(ledger_path: PathBuf) -> Self {
        Self { ledger_path }
    }

    pub(crate) fn record_decision(&self, input: DecisionInput) -> Option<DecisionRecord> {
        match self.record_decision_inner(input) {
            Ok(record) => Some(record),
            Err(error) => {
                tracing::warn!(
                    ledger = %self.ledger_path.display(),
                    %error,
                    "judgment decision was not written"
                );
                None
            }
        }
    }

    pub(crate) fn record_decisions(&self, inputs: Vec<DecisionInput>) -> Vec<Option<DecisionRecord>> {
        let count = inputs.len();
        match with_ledger_lock(&self.ledger_path, || {
            let mut records = Vec::with_capacity(count);
            let mut created = Vec::new();
            for input in inputs {
                match self.prepare_decision(input) {
                    Ok((record, path)) => {
                        if let Some(path) = path { created.push(path); }
                        records.push(Some(record));
                    }
                    Err(error) => {
                        tracing::warn!(ledger = %self.ledger_path.display(), %error, "judgment decision was not written");
                        records.push(None);
                    }
                }
            }
            let ready: Vec<_> = records.iter().filter_map(Option::as_ref).collect();
            if !ready.is_empty() {
                if let Err(error) = append_records_unlocked(&self.ledger_path, &ready) {
                    for path in created { let _ = fs::remove_file(path); }
                    return Err(error);
                }
            }
            Ok(records)
        }) {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(ledger = %self.ledger_path.display(), %error, "judgment decisions were not written");
                vec![None; count]
            }
        }
    }

    pub(crate) fn record_outcome(&self, input: OutcomeInput) -> Option<OutcomeRecord> {
        match self.record_outcome_inner(input) {
            Ok(record) => Some(record),
            Err(error) => {
                tracing::warn!(
                    ledger = %self.ledger_path.display(),
                    %error,
                    "judgment outcome was not written"
                );
                None
            }
        }
    }

    pub(crate) fn record_outcomes(&self, inputs: Vec<OutcomeInput>) -> Vec<Option<OutcomeRecord>> {
        let mut records = Vec::with_capacity(inputs.len());
        for input in inputs {
            match self.prepare_outcome(input) {
                Ok(record) => records.push(Some(record)),
                Err(error) => {
                    tracing::warn!(ledger = %self.ledger_path.display(), %error, "judgment outcome was not written");
                    records.push(None);
                }
            }
        }
        let ready: Vec<_> = records.iter().filter_map(Option::as_ref).collect();
        if !ready.is_empty() {
            if let Err(error) = append_records(&self.ledger_path, &ready) {
                tracing::warn!(ledger = %self.ledger_path.display(), %error, "judgment outcomes were not written");
                return vec![None; records.len()];
            }
        }
        records
    }

    /// Retrieval IDs are deterministic. Evidence presence is a bounded lookup for
    /// an already written decision, and the lookup and append share the ledger lock.
    pub(crate) fn record_retrieval_decisions_once(&self, inputs: Vec<DecisionInput>) -> Vec<Option<DecisionRecord>> {
        let count = inputs.len();
        match with_ledger_lock(&self.ledger_path, || {
            let mut results = Vec::with_capacity(count);
            let mut pending = Vec::new();
            let mut created = Vec::new();
            for input in inputs {
                let existed = input.decision_id.as_deref().is_some_and(|id| {
                    self.ledger_path.parent().unwrap().join("evidence").join(format!("{id}.json")).exists()
                });
                match self.prepare_decision(input) {
                    Ok((record, path)) => {
                        if let Some(path) = path { created.push(path); }
                        if !existed { pending.push(record.clone()); }
                        results.push(Some(record));
                    }
                    Err(error) => {
                        tracing::warn!(ledger = %self.ledger_path.display(), %error, "retrieval decision was not written");
                        results.push(None);
                    }
                }
            }
            if !pending.is_empty() {
                if let Err(error) = append_records_unlocked(&self.ledger_path, &pending) {
                    for path in created { let _ = fs::remove_file(path); }
                    return Err(error);
                }
            }
            Ok(results)
        }) {
            Ok(results) => results,
            Err(error) => {
                tracing::warn!(ledger = %self.ledger_path.display(), %error, "retrieval decisions were not written");
                vec![None; count]
            }
        }
    }

    pub(crate) fn record_retrieval_outcomes_once(&self, inputs: Vec<OutcomeInput>) -> Vec<Option<OutcomeRecord>> {
        let count = inputs.len();
        match with_ledger_lock(&self.ledger_path, || {
            let index = retrieval_outcome_index(&self.ledger_path)?;
            let mut results = Vec::with_capacity(count);
            let mut pending = Vec::new();
            let mut markers = Vec::new();
            for input in inputs {
                let marker = outcome_marker(&index, &input.decision_id, &input.label, input.source)?;
                if marker.exists() {
                    results.push(None);
                    continue;
                }
                match self.prepare_outcome(input) {
                    Ok(record) => {
                        markers.push(marker);
                        pending.push(record.clone());
                        results.push(Some(record));
                    }
                    Err(error) => {
                        tracing::warn!(ledger = %self.ledger_path.display(), %error, "retrieval outcome was not written");
                        results.push(None);
                    }
                }
            }
            if !pending.is_empty() {
                append_records_unlocked(&self.ledger_path, &pending)?;
                for marker in markers {
                    if let Err(error) = fs::write(&marker, b"") {
                        tracing::warn!(path = %marker.display(), %error, "retrieval outcome index marker was not written");
                    }
                }
            }
            Ok(results)
        }) {
            Ok(results) => results,
            Err(error) => {
                tracing::warn!(ledger = %self.ledger_path.display(), %error, "retrieval outcomes were not written");
                vec![None; count]
            }
        }
    }

    fn record_decision_inner(&self, input: DecisionInput) -> io::Result<DecisionRecord> {
        with_ledger_lock(&self.ledger_path, || {
            let (record, created) = self.prepare_decision(input)?;
            if let Err(error) = append_records_unlocked(&self.ledger_path, &[&record]) {
                if let Some(path) = created { let _ = fs::remove_file(path); }
                return Err(error);
            }
            Ok(record)
        })
    }

    fn prepare_decision(&self, input: DecisionInput) -> io::Result<(DecisionRecord, Option<PathBuf>)> {
        validate_extra_fields(&input.extra, DECISION_CORE_FIELDS)?;
        if input.surface.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "decision surface must not be empty",
            ));
        }
        validate_optional_finite(input.confidence, "confidence")?;
        validate_optional_finite(input.cost_usd, "cost_usd")?;
        if let Some(probabilities) = &input.probabilities {
            if let Some((name, _)) = probabilities.iter().find(|(_, value)| !value.is_number()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("probability {name:?} must be a number"),
                ));
            }
        }
        if let Some(sampling) = &input.sampling {
            validate_sampling(sampling)?;
        }

        let decision_id = input
            .decision_id
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let evidence = write_evidence(&self.ledger_path, &decision_id, &input.observations)?;
        let record = DecisionRecord {
            v: SCHEMA_VERSION,
            kind: "decision".to_string(),
            ts: now_iso(),
            decision_id,
            surface: input.surface,
            subject_ref: input.subject_ref,
            state_hash: Some(evidence.state_hash),
            state_ref: Some(evidence.state_ref),
            question_id: input.question_id,
            question_version: input.question_version,
            judge: input.judge,
            model: input.model,
            sampling: input.sampling,
            mode: input.mode,
            answer: input.answer,
            probabilities: input.probabilities,
            confidence: input.confidence,
            threshold_id: input.threshold_id,
            routed: input.routed,
            latency_ms: input.latency_ms,
            cost_usd: input.cost_usd,
            error: input.error,
            extra: input.extra,
        };
        let clean = match scrub_record(record) {
            Ok(clean) => clean,
            Err(error) => {
                if let Some(path) = evidence.created_path { let _ = fs::remove_file(path); }
                return Err(error);
            }
        };
        Ok((clean, evidence.created_path))
    }

    fn record_outcome_inner(&self, input: OutcomeInput) -> io::Result<OutcomeRecord> {
        let record = self.prepare_outcome(input)?;
        append_record(&self.ledger_path, &record)?;
        Ok(record)
    }

    fn prepare_outcome(&self, input: OutcomeInput) -> io::Result<OutcomeRecord> {
        validate_extra_fields(&input.extra, OUTCOME_CORE_FIELDS)?;
        if input.decision_id.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "outcome decision id must not be empty",
            ));
        }
        let record = OutcomeRecord {
            v: SCHEMA_VERSION,
            kind: "outcome".to_string(),
            ts: now_iso(),
            decision_id: input.decision_id,
            label: input.label,
            source: input.source,
            note: input.note,
            extra: input.extra,
        };
        let clean = scrub_record(record)?;
        Ok(clean)
    }
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn validate_extra_fields(extra: &Map<String, Value>, core: &[&str]) -> io::Result<()> {
    let core: HashSet<_> = core.iter().copied().collect();
    if let Some(field) = extra.keys().find(|field| core.contains(field.as_str())) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("extra field collides with core field {field:?}"),
        ));
    }
    Ok(())
}

fn validate_optional_finite(value: Option<f64>, field: &str) -> io::Result<()> {
    if value.is_some_and(|value| !value.is_finite()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{field} must be finite"),
        ));
    }
    Ok(())
}

fn validate_sampling(sampling: &Map<String, Value>) -> io::Result<()> {
    for field in ["temperature", "top_p"] {
        if sampling
            .get(field)
            .is_some_and(|value| !value.is_null() && !value.is_number())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("sampling.{field} must be a number or null"),
            ));
        }
    }
    if sampling
        .get("seed")
        .is_some_and(|value| !value.is_null() && !value.is_i64() && !value.is_u64())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sampling.seed must be an integer or null",
        ));
    }
    Ok(())
}

fn scrub_record<T>(record: T) -> io::Result<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    let value = serde_json::to_value(record).map_err(io::Error::other)?;
    serde_json::from_value(scrub_value(&value)?).map_err(io::Error::other)
}

fn lock_path(ledger_path: &Path) -> PathBuf {
    let mut path = ledger_path.as_os_str().to_os_string();
    path.push(".lock");
    PathBuf::from(path)
}

fn append_record<T: Serialize>(ledger_path: &Path, record: &T) -> io::Result<()> {
    append_records(ledger_path, &[record])
}

fn outcome_marker(index: &Path, id: &str, label: &Value, source: OutcomeSource) -> io::Result<PathBuf> {
    let result = label.get("result").and_then(Value::as_str).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "retrieval outcome has no result"))?;
    let key = serde_json::to_vec(&(id, result, source)).map_err(io::Error::other)?;
    Ok(index.join(format!("{:x}", Sha256::digest(key))))
}

fn retrieval_outcome_index(ledger_path: &Path) -> io::Result<PathBuf> {
    let parent = ledger_path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ledger path has no parent"))?;
    let name = ledger_path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ledger path has no filename"))?;
    let index = parent.join(format!("{}.retrieval-outcome-index", name.to_string_lossy()));
    fs::create_dir_all(&index)?;
    let cursor_path = index.join(".indexed");
    let cursor = fs::read_to_string(&cursor_path).ok().and_then(|text| text.parse::<u64>().ok()).unwrap_or(0);
    match fs::File::open(ledger_path) {
        Ok(mut file) => {
            let end = file.metadata()?.len();
            if cursor == end && cursor_path.exists() { return Ok(index); }
            file.seek(SeekFrom::Start(if cursor <= end { cursor } else { 0 }))?;
            for line in BufReader::new(file).lines() {
                let line = line?;
                let Ok(row) = serde_json::from_str::<Value>(&line) else { continue; };
                if row.get("kind").and_then(Value::as_str) != Some("outcome") { continue; }
                let (Some(id), Some(label), Some(source)) = (
                    row.get("decision_id").and_then(Value::as_str),
                    row.get("label"),
                    row.get("source").cloned().and_then(|value| serde_json::from_value::<OutcomeSource>(value).ok()),
                ) else { continue; };
                if let Ok(marker) = outcome_marker(&index, id, label, source) {
                    fs::write(marker, b"")?;
                }
            }
            fs::write(cursor_path, end.to_string())?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::write(cursor_path, b"0")?;
        }
        Err(error) => return Err(error),
    }
    Ok(index)
}

fn append_records<T: Serialize>(ledger_path: &Path, records: &[T]) -> io::Result<()> {
    with_ledger_lock(ledger_path, || append_records_unlocked(ledger_path, records))
}

fn with_ledger_lock<T>(ledger_path: &Path, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let parent = ledger_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ledger path has no parent"))?;
    fs::create_dir_all(parent)?;

    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path(ledger_path))?;
    FileExt::lock_exclusive(&lock_file)?;
    let result = operation();
    let _ = FileExt::unlock(&lock_file);
    result
}

fn append_records_unlocked<T: Serialize>(ledger_path: &Path, records: &[T]) -> io::Result<()> {
    let mut lines = Vec::new();
    for record in records {
        let serialized = serde_json::to_string(record).map_err(io::Error::other)?;
        lines.extend_from_slice(scrub(&serialized).as_bytes());
        lines.push(b'\n');
    }
    let mut ledger = OpenOptions::new()
        .create(true)
        .truncate(false)
        .append(true)
        .open(ledger_path)?;
    ledger.write_all(&lines)?;
    ledger.flush()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::process::Command;
    use std::sync::Arc;
    use std::thread;

    use super::*;
    use crate::judgment::evidence::{canonical_json, sha256_of};
    use tempfile::TempDir;

    const PYTHON_FIXTURE: &str = include_str!("fixtures/python-canonical-state.json");
    const PYTHON_FIXTURE_HASH: &str = include_str!("fixtures/python-canonical-state.sha256");

    fn decision(id: impl Into<String>) -> DecisionInput {
        DecisionInput {
            decision_id: Some(id.into()),
            surface: "hive.qa.criterion.pass_fail".to_string(),
            subject_ref: serde_json::from_value(serde_json::json!({
                "session_id": "synthetic-session",
                "milestone": 1,
                "criterion": 1
            }))
            .unwrap(),
            observations: serde_json::json!({
                "criterion": {"number": 1, "kind": "pass_fail", "text": "Synthetic criterion"},
                "evidence_refs": ["qa-worker-synthetic#1"],
                "excerpts": ["Synthetic observation"]
            }),
            answer: serde_json::json!({"result": "pass", "rationale": "Synthetic rationale"}),
            question_id: Some("hive.qa.criterion.v1".to_string()),
            question_version: Some("sha256:synthetic".to_string()),
            judge: Judge::IncumbentLlm,
            model: Some("synthetic-cli/synthetic-model".to_string()),
            sampling: None,
            mode: JudgmentMode::Shadow,
            probabilities: None,
            confidence: None,
            threshold_id: None,
            routed: "none".to_string(),
            latency_ms: None,
            cost_usd: None,
            error: None,
            extra: Map::new(),
        }
    }

    fn read_rows(path: &Path) -> Vec<Value> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn production_ledger_snapshot() -> Option<(PathBuf, bool, u64)> {
        let app_data = std::env::var_os("APPDATA")?;
        let path = PathBuf::from(app_data)
            .join("hive-manager")
            .join("judgments")
            .join("ledger.jsonl");
        match fs::metadata(&path) {
            Ok(metadata) => Some((path, true, metadata.len())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Some((path, false, 0)),
            Err(_) => None,
        }
    }

    fn assert_production_ledger_unchanged(before: Option<(PathBuf, bool, u64)>) {
        let Some((path, existed, len)) = before else {
            return;
        };
        match fs::metadata(path) {
            Ok(metadata) => {
                assert!(existed, "test created the production judgment ledger");
                assert_eq!(
                    metadata.len(),
                    len,
                    "test changed the production judgment ledger"
                );
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                assert!(!existed, "test removed the production judgment ledger");
            }
            Err(error) => panic!("could not re-stat production judgment ledger: {error}"),
        }
    }

    #[test]
    fn python_hashed_fixture_matches_canonical_rust_bytes() {
        let fixture: Value = serde_json::from_str(PYTHON_FIXTURE).unwrap();
        assert_eq!(sha256_of(&fixture).unwrap(), PYTHON_FIXTURE_HASH.trim());
        let canonical = String::from_utf8(canonical_json(&fixture).unwrap()).unwrap();
        assert!(canonical.contains("1e+20"));
        assert!(canonical.contains("1e-05"));
        assert!(canonical.contains("1e-06"));
        assert!(canonical.contains("雪 / café / 😀"));
    }

    #[test]
    fn evidence_excludes_incumbent_conclusion_and_rehashes() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("ledger.jsonl");
        let ledger = JudgmentLedger::new(path.clone());
        let record = ledger
            .record_decision(decision("observations-only"))
            .unwrap();
        let evidence_path = path.parent().unwrap().join(record.state_ref.unwrap());
        let evidence: Value = serde_json::from_slice(&fs::read(evidence_path).unwrap()).unwrap();

        assert!(evidence.get("result").is_none());
        assert!(evidence.get("rationale").is_none());
        assert!(!evidence.to_string().contains("Synthetic rationale"));
        assert_eq!(sha256_of(&evidence).unwrap(), record.state_hash.unwrap());
    }

    #[test]
    fn rows_and_evidence_scrub_key_shaped_strings_and_keep_unknown_fields() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("ledger.jsonl");
        let ledger = JudgmentLedger::new(path.clone());
        let mut input = decision("scrubbed");
        input.observations["token"] = serde_json::json!("sk-ABCDEFGHIJKLMNOPQRSTUVWX");
        input.answer = serde_json::json!({
            "result": "pass",
            "rationale": "Bearer abcdefghijklmnop"
        });
        input.extra.insert(
            "future_field".to_string(),
            serde_json::json!("sk-ZYXWVUTSRQPONMLKJIHGFE"),
        );
        let record = ledger.record_decision(input).unwrap();
        let row = &read_rows(&path)[0];
        assert_eq!(row["future_field"], "[REDACTED]");
        assert_eq!(row["answer"]["rationale"], "[REDACTED]");
        let evidence: Value = serde_json::from_slice(
            &fs::read(path.parent().unwrap().join(record.state_ref.unwrap())).unwrap(),
        )
        .unwrap();
        assert_eq!(evidence["token"], "[REDACTED]");
    }

    #[test]
    fn sampling_null_and_empty_object_remain_distinct() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("ledger.jsonl");
        let ledger = JudgmentLedger::new(path.clone());
        ledger
            .record_decision(decision("unknown-sampling"))
            .unwrap();
        let mut no_parameters = decision("no-sampling-parameters");
        no_parameters.sampling = Some(Map::new());
        ledger.record_decision(no_parameters).unwrap();

        let rows = read_rows(&path);
        assert!(rows[0]["sampling"].is_null());
        assert_eq!(rows[1]["sampling"], serde_json::json!({}));
    }

    #[test]
    fn repeated_decision_id_never_replaces_existing_evidence() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("ledger.jsonl");
        let ledger = JudgmentLedger::new(path.clone());
        let first = ledger.record_decision(decision("same-id")).unwrap();
        let evidence_path = temp.path().join(first.state_ref.unwrap());
        let original = fs::read(&evidence_path).unwrap();
        let mut changed = decision("same-id");
        changed.observations["excerpts"] = serde_json::json!(["Changed observation"]);
        assert!(ledger.record_decision(changed).is_none());
        assert_eq!(fs::read(evidence_path).unwrap(), original);
        assert_eq!(read_rows(&path).len(), 1);
        assert!(ledger.record_decision(decision("same-id")).is_some());
    }

    #[test]
    fn failed_batch_append_removes_only_new_evidence() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("ledger.jsonl");
        let ledger = JudgmentLedger::new(path.clone());
        let first = ledger.record_decision(decision("existing-id")).unwrap();
        let prior_path = temp.path().join(first.state_ref.unwrap());
        let prior_bytes = fs::read(&prior_path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let result = ledger.record_decisions(vec![decision("existing-id"), decision("new-id")]);
        assert!(result.iter().all(Option::is_none));
        assert_eq!(fs::read(prior_path).unwrap(), prior_bytes);
        assert!(!temp.path().join("evidence/new-id.json").exists());
    }

    #[test]
    fn batched_rows_match_single_row_api_and_fail_open() {
        let single_dir = TempDir::new().unwrap();
        let batch_dir = TempDir::new().unwrap();
        let single_path = single_dir.path().join("ledger.jsonl");
        let batch_path = batch_dir.path().join("ledger.jsonl");
        let single = JudgmentLedger::new(single_path.clone());
        let batch = JudgmentLedger::new(batch_path.clone());
        for id in ["synthetic-1", "synthetic-2"] {
            assert!(single.record_decision(decision(id)).is_some());
        }
        let decisions = batch.record_decisions(vec![decision("synthetic-1"), decision("synthetic-2")]);
        assert!(decisions.iter().all(Option::is_some));
        let outcome = |id: &str| OutcomeInput {
            decision_id: id.to_string(),
            label: serde_json::json!({ "result": "pass" }),
            source: OutcomeSource::Downstream,
            note: None,
            extra: Map::new(),
        };
        for id in ["synthetic-1", "synthetic-2"] {
            assert!(single.record_outcome(outcome(id)).is_some());
        }
        let outcomes = batch.record_outcomes(vec![outcome("synthetic-1"), outcome("synthetic-2")]);
        assert!(outcomes.iter().all(Option::is_some));
        let mut single_rows = read_rows(&single_path);
        let mut batch_rows = read_rows(&batch_path);
        for row in single_rows.iter_mut().chain(batch_rows.iter_mut()) {
            row.as_object_mut().unwrap().remove("ts");
        }
        assert_eq!(batch_rows, single_rows);

        let mut invalid = decision("bad");
        invalid.surface.clear();
        let mixed = batch.record_decisions(vec![invalid, decision("synthetic-3")]);
        assert!(mixed[0].is_none());
        assert!(mixed[1].is_some());
        assert_eq!(read_rows(&batch_path).len(), 5);

        let blocked_path = batch_dir.path().join("blocked-ledger.jsonl");
        fs::create_dir(&blocked_path).unwrap();
        let blocked = JudgmentLedger::new(blocked_path);
        assert!(blocked.record_decisions(vec![decision("synthetic-4")])[0].is_none());
        assert!(blocked.record_outcomes(vec![outcome("synthetic-4")])[0].is_none());
    }

    #[test]
    fn decision_validation_rejects_schema_type_errors_and_core_collisions() {
        let temp = TempDir::new().unwrap();
        let ledger = JudgmentLedger::new(temp.path().join("ledger.jsonl"));

        let mut bad_probability = decision("bad-probability");
        bad_probability
            .probabilities
            .get_or_insert_default()
            .insert("pass".to_string(), Value::String("certain".to_string()));
        assert!(ledger.record_decision(bad_probability).is_none());

        let mut bad_sampling = decision("bad-sampling");
        bad_sampling
            .sampling
            .get_or_insert_default()
            .insert("seed".to_string(), serde_json::json!(1.5));
        assert!(ledger.record_decision(bad_sampling).is_none());

        let mut non_finite = decision("non-finite");
        non_finite.confidence = Some(f64::NAN);
        assert!(ledger.record_decision(non_finite).is_none());

        let mut collision = decision("collision");
        collision.extra.insert(
            "surface".to_string(),
            Value::String("replacement".to_string()),
        );
        assert!(ledger.record_decision(collision).is_none());

        let mut empty_surface = decision("empty-surface");
        empty_surface.surface.clear();
        assert!(ledger.record_decision(empty_surface).is_none());
        assert!(!temp.path().join("ledger.jsonl").exists());
    }

    #[test]
    fn decision_row_keeps_null_answer_and_uses_schema_spellings() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("ledger.jsonl");
        let ledger = JudgmentLedger::new(path.clone());
        let mut input = decision("null-answer");
        input.answer = Value::Null;
        let record = ledger.record_decision(input).unwrap();
        let row = &read_rows(&path)[0];

        assert!(row.get("answer").is_some_and(Value::is_null));
        assert_eq!(row["judge"], "incumbent-llm");
        assert_eq!(row["mode"], "shadow");
        assert_eq!(row["v"], 1);
        assert!(record.ts.ends_with('Z'));
        assert_eq!(record.ts.len(), "2026-09-22T21:24:00.000Z".len());
        assert!(chrono::DateTime::parse_from_rfc3339(&record.ts).is_ok());
    }

    #[test]
    fn write_failure_is_fail_open() {
        let temp = TempDir::new().unwrap();
        let not_a_directory = temp.path().join("file");
        fs::write(&not_a_directory, b"occupied").unwrap();
        let ledger = JudgmentLedger::new(not_a_directory.join("ledger.jsonl"));
        assert!(ledger.record_decision(decision("cannot-write")).is_none());
    }

    #[test]
    fn two_rust_writers_append_one_thousand_intact_rows() {
        let before = production_ledger_snapshot();
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("ledger.jsonl");
        let ledger = Arc::new(JudgmentLedger::new(path.clone()));
        let handles: Vec<_> = ["rust-a", "rust-b"]
            .into_iter()
            .map(|prefix| {
                let ledger = Arc::clone(&ledger);
                thread::spawn(move || {
                    for index in 0..500 {
                        assert!(ledger
                            .record_decision(decision(format!("{prefix}-{index}")))
                            .is_some());
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        assert_one_thousand_unique_rows(&path);
        assert_production_ledger_unchanged(before);
    }

    #[test]
    fn rust_and_python_writers_share_one_lock_and_lose_no_rows() {
        let before = production_ledger_snapshot();
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("ledger.jsonl");
        let vendored = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tools")
            .join("judgment")
            .join("ledger.py");
        let python = r#"
import argparse
import importlib.util

parser = argparse.ArgumentParser()
parser.add_argument("--ledger", required=True)
parser.add_argument("--module", required=True)
args = parser.parse_args()
spec = importlib.util.spec_from_file_location("vendored_judgment_ledger", args.module)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
for index in range(500):
    row = module.record_decision(
        "hive.qa.criterion.pass_fail",
        {"session_id": "synthetic-session", "criterion": 1},
        "code",
        {"result": "pass"},
        decision_id=f"python-{index}",
        sampling={},
        ledger=args.ledger,
    )
    if row is None:
        raise RuntimeError(f"python row {index} was not written")
"#;

        let mut python_commands = std::env::var_os("PYTHON")
            .into_iter()
            .chain(["python".into(), "python3".into()]);
        let mut attempted = Vec::new();
        let mut child = python_commands
            .find_map(|program| {
                attempted.push(program.to_string_lossy().into_owned());
                match Command::new(&program)
                    .arg("-c")
                    .arg(python)
                    .arg("--ledger")
                    .arg(&path)
                    .arg("--module")
                    .arg(&vendored)
                    .spawn()
                {
                    Ok(child) => Some(child),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => panic!("failed to launch {program:?}: {error}"),
                }
            })
            .unwrap_or_else(|| {
                panic!(
                    "python is required for judgment writer interoperability tests; tried {}",
                    attempted.join(", ")
                )
            });
        let rust_path = path.clone();
        let rust_writer = thread::spawn(move || {
            let ledger = JudgmentLedger::new(rust_path);
            for index in 0..500 {
                assert!(ledger
                    .record_decision(decision(format!("rust-{index}")))
                    .is_some());
            }
        });

        let status = child.wait().unwrap();
        rust_writer.join().unwrap();
        assert!(status.success(), "vendored Python writer failed: {status}");
        assert_one_thousand_unique_rows(&path);
        assert_production_ledger_unchanged(before);
    }

    fn assert_one_thousand_unique_rows(path: &Path) {
        let rows = read_rows(path);
        assert_eq!(rows.len(), 1_000);
        let ids: HashSet<_> = rows
            .iter()
            .map(|row| row["decision_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 1_000);
        for row in rows {
            assert_eq!(row["v"], 1);
            assert_eq!(row["kind"], "decision");
            assert!(row["ts"].is_string());
            assert!(row["surface"].is_string());
            assert!(row["subject_ref"].is_object());
            assert!(matches!(
                row["judge"].as_str(),
                Some("incumbent-llm" | "code")
            ));
            assert_eq!(row["mode"], "shadow");
            assert!(row.get("answer").is_some());
        }
    }

    #[test]
    fn outcome_rows_scrub_and_preserve_unknown_fields() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("ledger.jsonl");
        let ledger = JudgmentLedger::new(path.clone());
        let mut extra = Map::new();
        extra.insert(
            "future_field".to_string(),
            serde_json::json!({"kept": true}),
        );
        let record = ledger
            .record_outcome(OutcomeInput {
                decision_id: "decision-1".to_string(),
                label: serde_json::json!({"verdict": "fail"}),
                source: OutcomeSource::OperatorOverride,
                note: Some("Bearer abcdefghijklmnop".to_string()),
                extra,
            })
            .unwrap();
        assert_eq!(record.note.as_deref(), Some("[REDACTED]"));
        assert_eq!(read_rows(&path)[0]["future_field"]["kept"], true);
    }
}
