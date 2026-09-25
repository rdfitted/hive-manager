//! Local, shadow-only rows for references actually considered at worker spawn.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::judgment::ledger::{
    DecisionInput, Judge, JudgmentLedger, JudgmentMode, OutcomeInput, OutcomeSource,
};

const SURFACE: &str = "hive.retrieval.spawn";

#[cfg(test)]
pub(crate) static RETRIEVAL_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn enabled() -> bool {
    !matches!(std::env::var("HIVE_RETRIEVAL_LEDGER"), Ok(value) if value.eq_ignore_ascii_case("off"))
}

fn references(sidecar: &Value) -> Option<Vec<(&Value, bool)>> {
    let kept = sidecar.get("kept")?.as_array()?;
    let dropped = sidecar.get("dropped")?.as_array()?;
    Some(
        kept.iter()
            .filter(|reference| reference.is_object())
            .map(|reference| (reference, true))
            .chain(
                dropped
                    .iter()
                    .filter(|reference| reference.is_object())
                    .map(|reference| (reference, false)),
            )
            .collect(),
    )
}

fn valid_sidecar(sidecar: &Value) -> Option<(&str, &str, &str)> {
    if sidecar.get("schema_version")?.as_str()? != "hive.spawn-context/v1" {
        return None;
    }
    if sidecar.get("delivery_path").and_then(Value::as_str) == Some("composition-free") {
        return None;
    }
    let session_id = sidecar.get("session_id")?.as_str()?.trim();
    let agent_id = sidecar.get("agent_id")?.as_str()?.trim();
    let plan_task_id = sidecar.get("plan_task_id")?.as_str()?.trim();
    if session_id.is_empty() || agent_id.is_empty() || plan_task_id.is_empty() {
        return None;
    }
    Some((session_id, agent_id, plan_task_id))
}

fn decision_id(repo: &str, session_id: &str, agent_id: &str, index: usize) -> String {
    let name = format!("hive-manager:retrieval:{repo}:{session_id}:{SURFACE}:{agent_id}:{index}");
    Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes()).to_string()
}

fn field(reference: &Value, name: &str) -> Value {
    reference.get(name).cloned().unwrap_or(Value::Null)
}

/// Returns one durable decision ID slot per valid kept then dropped reference.
pub(crate) fn record_spawn_rows(
    ledger_path: &Path,
    original_project_path: &Path,
    sidecar: &Value,
) -> Vec<Option<String>> {
    let Some(references) = references(sidecar) else {
        return Vec::new();
    };
    let mut ids = vec![None; references.len()];
    if !enabled() {
        return ids;
    }
    let Some((session_id, agent_id, plan_task_id)) = valid_sidecar(sidecar) else {
        return ids;
    };
    let Some(repo) = fs::canonicalize(original_project_path).ok()
        .and_then(|path| path.file_name().and_then(|name| name.to_str()).map(str::to_string))
    else {
        tracing::warn!(path = %original_project_path.display(), "retrieval ledger skipped: original project path unavailable");
        return ids;
    };
    let misses: HashSet<&str> = sidecar.get("miss_sample_references")
        .and_then(Value::as_array)
        .into_iter().flatten().filter_map(Value::as_str).collect();
    let question_version = sidecar.get("question_version").and_then(|value| {
        if value.is_null() { None } else if let Some(text) = value.as_str() {
            (!text.is_empty()).then(|| text.to_string())
        } else { Some(value.to_string()) }
    });
    let mut inputs = Vec::new();
    let mut positions = Vec::new();
    for (index, (reference, kept)) in references.into_iter().enumerate() {
        let id = decision_id(&repo, session_id, agent_id, index);
        let mut subject_ref = Map::new();
        subject_ref.insert("repo".into(), json!(repo));
        subject_ref.insert("session_id".into(), json!(session_id));
        subject_ref.insert("agent_id".into(), json!(agent_id));
        subject_ref.insert("plan_task_id".into(), json!(plan_task_id));
        subject_ref.insert("reference_index".into(), json!(index));
        subject_ref.insert("tag".into(), field(reference, "tag"));
        subject_ref.insert("pointer".into(), field(reference, "pointer"));
        let features = json!({
            "disposition": if kept { "kept" } else { "dropped" },
            "reason": if kept { Value::Null } else { field(reference, "reason") },
            "position": if kept { field(reference, "position") } else { Value::Null },
            "origin": field(reference, "origin"),
            "provenance": field(reference, "source"),
            "priority": field(reference, "priority"),
            "cost": field(reference, "chars"),
            "miss_sample": kept && reference.get("tag").and_then(Value::as_str).is_some_and(|tag| misses.contains(tag)),
        });
        let mut extra = Map::new();
        extra.insert("retrieval_features".into(), features);
        inputs.push(DecisionInput {
            decision_id: Some(id.clone()),
            surface: SURFACE.into(),
            subject_ref,
            observations: json!({
                "reference": {
                    "pointer": field(reference, "pointer"),
                    "source": field(reference, "source"),
                    "origin": field(reference, "origin"),
                    "priority": field(reference, "priority"),
                    "chars": field(reference, "chars"),
                },
                "budget": sidecar.get("budget").cloned().unwrap_or(Value::Null),
            }),
            answer: json!({"result": if kept { "used" } else { "unused" }}),
            question_id: Some("knowledge_ack".into()),
            question_version: question_version.clone(),
            judge: Judge::Code,
            model: None,
            sampling: None,
            mode: JudgmentMode::Shadow,
            probabilities: None,
            confidence: None,
            threshold_id: None,
            routed: "none".into(),
            latency_ms: None,
            cost_usd: None,
            error: None,
            extra,
        });
        positions.push((index, id));
    }
    for ((index, id), result) in positions.into_iter().zip(JudgmentLedger::new(ledger_path.to_path_buf()).record_retrieval_decisions_once(inputs)) {
        if result.is_some() {
            ids[index] = Some(id);
        }
    }
    ids
}

/// Called only after an explicit completed acknowledgement was saved.
pub(crate) fn record_ack_outcomes(ledger_path: &Path, sidecar: &Value, ack: &[String]) -> usize {
    if !enabled() || valid_sidecar(sidecar).is_none() || sidecar.get("sampled").and_then(Value::as_bool) != Some(true) {
        return 0;
    }
    let Some(references) = references(sidecar) else { return 0; };
    let kept_tags: HashSet<&str> = references.iter().filter(|(_, kept)| *kept)
        .filter_map(|(reference, _)| reference.get("tag").and_then(Value::as_str)).collect();
    if let Some(tag) = ack.iter().find(|tag| !kept_tags.contains(tag.as_str())) {
        tracing::warn!(tag, "retrieval acknowledgement has out-of-context tag; no outcomes written");
        return 0;
    }
    let ids = sidecar.get("decision_ids").and_then(Value::as_array);
    let acknowledged: HashSet<&str> = ack.iter().map(String::as_str).collect();
    let mut inputs = Vec::new();
    for (index, (reference, kept)) in references.iter().enumerate() {
        if !kept { continue; }
        let Some(tag) = reference.get("tag").and_then(Value::as_str) else { continue; };
        let Some(id) = ids.and_then(|ids| ids.get(index)).and_then(Value::as_str) else { continue; };
        let result = if acknowledged.contains(tag) { "used" } else { "unused" };
        let mut extra = Map::new();
        extra.insert("session_id".into(), field(sidecar, "session_id"));
        extra.insert("agent_id".into(), field(sidecar, "agent_id"));
        extra.insert("tag".into(), json!(tag));
        inputs.push(OutcomeInput {
            decision_id: id.to_string(),
            label: json!({"result": result}),
            source: OutcomeSource::ModelAck,
            note: Some("worker completion knowledge acknowledgement".into()),
            extra,
        });
    }
    if inputs.is_empty() { return 0; }
    JudgmentLedger::new(ledger_path.to_path_buf()).record_retrieval_outcomes_once(inputs).into_iter().filter(Option::is_some).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::Instant;

    fn fixture(name: &str) -> Value {
        let text = match name {
            "sampled" => include_str!("../../../../tools/judgment/tests/fixtures/retrieval-parity/sampled-context.json"),
            "unsampled" => include_str!("../../../../tools/judgment/tests/fixtures/retrieval-parity/unsampled-context.json"),
            _ => unreachable!(),
        };
        serde_json::from_str(text).unwrap()
    }

    fn rows(path: &Path) -> Vec<Value> {
        fs::read_to_string(path).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    #[test]
    fn retrieval_rows_match_replay_fixture_and_join_ack_once() {
        let _guard = RETRIEVAL_ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("synthetic-repo");
        fs::create_dir(&repo).unwrap();
        let ledger = temp.path().join("ledger.jsonl");
        let mut sampled = fixture("sampled");
        let mut unsampled = fixture("unsampled");
        let sampled_ids = record_spawn_rows(&ledger, &repo, &sampled);
        let unsampled_ids = record_spawn_rows(&ledger, &repo, &unsampled);
        assert_eq!(sampled_ids.len(), 3);
        assert_eq!(unsampled_ids.len(), 2);
        assert!(sampled_ids.iter().chain(unsampled_ids.iter()).all(Option::is_some));
        sampled["decision_ids"] = json!(sampled_ids);
        unsampled["decision_ids"] = json!(unsampled_ids);

        let projected: Vec<_> = rows(&ledger).into_iter().map(|row| json!({
            "decision_id": row["decision_id"],
            "surface": row["surface"],
            "subject_ref": row["subject_ref"],
            "answer": row["answer"],
        })).collect();
        let expected: Value = serde_json::from_str(include_str!("../../../../tools/judgment/tests/fixtures/retrieval-parity/expected-rows.json")).unwrap();
        assert_eq!(json!(projected), expected);
        assert_eq!(record_spawn_rows(&ledger, &repo, &sampled), sampled["decision_ids"].as_array().unwrap().iter().map(|id| id.as_str().map(str::to_string)).collect::<Vec<_>>());
        assert_eq!(rows(&ledger).len(), 5);

        assert_eq!(record_ack_outcomes(&ledger, &unsampled, &["k1".into()]), 0);
        assert_eq!(record_ack_outcomes(&ledger, &sampled, &["outside".into()]), 0);
        assert_eq!(record_ack_outcomes(&ledger, &sampled, &["k1".into()]), 2);
        assert_eq!(record_ack_outcomes(&ledger, &sampled, &["k1".into()]), 0);
        let outcomes: Vec<_> = rows(&ledger).into_iter().filter(|row| row["kind"] == "outcome").collect();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0]["label"], json!({"result": "used"}));
        assert_eq!(outcomes[1]["label"], json!({"result": "unused"}));
        assert_eq!(outcomes[0]["source"], "model-ack");
        let tables = temp.path().join("tables.json");
        fs::write(&tables, r#"{"surfaces":{"hive.retrieval.spawn":{"mode":"shadow","question_type":"multiclass"}}}"#).unwrap();
        let audit = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
            .join("tools/judgment/audit.py");
        let python = std::env::var_os("PYTHON").unwrap_or_else(|| "python".into());
        let output = Command::new(python).arg(audit).arg("--ledger").arg(&ledger)
            .arg("--tables").arg(&tables).arg("--no-retrievals")
            .arg("--no-history").arg("--json").output().unwrap();
        assert!(output.status.success(), "audit failed: {}", String::from_utf8_lossy(&output.stderr));
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        let group = report["judges"].as_array().unwrap().iter()
            .find(|group| group["surface"] == SURFACE).unwrap();
        assert_eq!(group["joined"], 2);
        let correct = ["heldout", "tune"].iter().map(|split| {
            let agreement = group[format!("agreement_{split}")].as_f64().unwrap_or(0.0);
            let count = group[format!("n_{split}")].as_u64().unwrap_or(0);
            agreement * count as f64
        }).sum::<f64>();
        assert_eq!(correct, 1.0);
        assert_eq!(correct / group["joined"].as_f64().unwrap(), 0.5);
        // An explicit empty acknowledgement is a decision, and only its new
        // (decision_id, label, source) triple is appended.
        assert_eq!(record_ack_outcomes(&ledger, &sampled, &[]), 1);
        assert_eq!(record_ack_outcomes(&ledger, &sampled, &[]), 0);
        assert_eq!(rows(&ledger).len(), 8);
    }

    #[test]
    fn retrieval_rows_are_plan_bound_and_fail_open() {
        let _guard = RETRIEVAL_ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("synthetic-repo");
        fs::create_dir(&repo).unwrap();
        let ledger = temp.path().join("ledger.jsonl");
        let mut sidecar = fixture("sampled");
        sidecar["plan_task_id"] = Value::Null;
        assert_eq!(record_spawn_rows(&ledger, &repo, &sidecar), vec![None; 3]);
        assert!(!ledger.exists());
        sidecar["plan_task_id"] = json!("T1");
        sidecar["delivery_path"] = json!("composition-free");
        assert_eq!(record_spawn_rows(&ledger, &repo, &sidecar), vec![None; 3]);
        assert!(!ledger.exists());
        sidecar["delivery_path"] = json!("task-bound");
        assert_eq!(record_spawn_rows(&ledger, &temp.path().join("missing"), &sidecar), vec![None; 3]);
        assert!(!ledger.exists());
        let blocked = temp.path().join("blocked");
        fs::write(&blocked, "file").unwrap();
        assert_eq!(record_spawn_rows(&blocked.join("ledger.jsonl"), &repo, &sidecar), vec![None; 3]);
    }

    #[test]
    fn retrieval_kill_switch_writes_no_rows() {
        let _guard = RETRIEVAL_ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os("HIVE_RETRIEVAL_LEDGER");
        std::env::set_var("HIVE_RETRIEVAL_LEDGER", "off");
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("synthetic-repo");
        fs::create_dir(&repo).unwrap();
        let ledger = temp.path().join("ledger.jsonl");
        let sidecar = fixture("sampled");
        assert_eq!(record_spawn_rows(&ledger, &repo, &sidecar), vec![None; 3]);
        assert_eq!(record_ack_outcomes(&ledger, &sidecar, &["k1".into()]), 0);
        assert!(!ledger.exists());
        if let Some(value) = previous { std::env::set_var("HIVE_RETRIEVAL_LEDGER", value); }
        else { std::env::remove_var("HIVE_RETRIEVAL_LEDGER"); }
    }

    #[test]
    fn large_existing_ledger_keeps_retrieval_spawn_under_fifty_ms() {
        let _guard = RETRIEVAL_ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("synthetic-repo");
        fs::create_dir(&repo).unwrap();
        let ledger = temp.path().join("ledger.jsonl");
        let row = b"{\"kind\":\"decision\",\"decision_id\":\"unrelated\",\"padding\":\"synthetic synthetic synthetic synthetic synthetic\"}\n";
        let mut file = fs::File::create(&ledger).unwrap();
        use std::io::Write;
        for _ in 0..100_000 { file.write_all(row).unwrap(); }
        drop(file);
        assert!(fs::metadata(&ledger).unwrap().len() >= 10_000_000);
        let mut sidecar = fixture("sampled");
        let mut best_ms = u128::MAX;
        for index in 0..3 {
            sidecar["agent_id"] = json!(format!("synthetic-agent-{index}"));
            let start = Instant::now();
            let ids = record_spawn_rows(&ledger, &repo, &sidecar);
            best_ms = best_ms.min(start.elapsed().as_millis());
            assert!(ids.iter().all(Option::is_some));
        }
        assert!(best_ms < 50, "best spawn took {best_ms} ms against large ledger");
    }

    #[test]
    fn concurrent_retrieval_writers_append_each_id_once() {
        let _guard = RETRIEVAL_ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("synthetic-repo");
        fs::create_dir(&repo).unwrap();
        let ledger = temp.path().join("ledger.jsonl");
        let sidecar = fixture("sampled");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            for _ in 0..2 {
                let barrier = barrier.clone();
                let sidecar = sidecar.clone();
                let ledger = ledger.clone();
                let repo = repo.clone();
                scope.spawn(move || {
                    barrier.wait();
                    assert!(record_spawn_rows(&ledger, &repo, &sidecar).iter().all(Option::is_some));
                });
            }
        });
        assert_eq!(rows(&ledger).len(), 3);
        let mut sidecar = sidecar;
        let ids = record_spawn_rows(&ledger, &repo, &sidecar);
        sidecar["decision_ids"] = json!(ids);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                let barrier = barrier.clone();
                let sidecar = sidecar.clone();
                let ledger = ledger.clone();
                scope.spawn(move || {
                    barrier.wait();
                    record_ack_outcomes(&ledger, &sidecar, &["k1".into()]);
                });
            }
        });
        assert_eq!(rows(&ledger).len(), 5);
    }

    #[test]
    #[ignore = "timing benchmark: run isolated via --ignored --test-threads=1 (CI step 'Overhead benchmarks')"]
    fn benchmark_retrieval_spawn_overhead() {
        let _guard = RETRIEVAL_ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("synthetic-repo");
        fs::create_dir(&repo).unwrap();
        let mut sidecar = fixture("sampled");
        let one = sidecar["kept"][0].clone();
        sidecar["kept"] = Value::Array((0..20).map(|i| {
            let mut reference = one.clone();
            reference["pointer"] = json!(format!("synthetic/{i}.md"));
            reference["tag"] = json!(format!("k{}", i + 1));
            reference
        }).collect());
        sidecar["dropped"] = json!([]);
        let mut best_median_ms = f64::INFINITY;
        // A real regression is slow on every attempt. Scheduler contention only
        // adds time, so the best of three medians discounts transient noise.
        for attempt in 1..=3 {
            let warmup_ledger = temp.path().join(format!("warmup-{attempt}.jsonl"));
            assert_eq!(record_spawn_rows(&warmup_ledger, &repo, &sidecar).len(), 20);
            let mut elapsed = Vec::with_capacity(21);
            let mut bytes = 0;
            for i in 0..21 {
                sidecar["session_id"] = json!(format!("55555555-5555-4555-8555-{attempt:02}{i:010}"));
                let ledger = temp.path().join(format!("ledger-{attempt}-{i}.jsonl"));
                let start = Instant::now();
                let ids = record_spawn_rows(&ledger, &repo, &sidecar);
                sidecar["decision_ids"] = json!(ids);
                elapsed.push(start.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(sidecar["decision_ids"].as_array().unwrap().len(), 20);
                assert!(sidecar["decision_ids"].as_array().unwrap().iter().all(Value::is_string));
                bytes += fs::metadata(ledger).unwrap().len();
            }
            elapsed.sort_by(f64::total_cmp);
            let median_ms = elapsed[10];
            eprintln!("retrieval spawn platform={} attempt={attempt} N=21 median={median_ms:.2}ms range={:.2}..={:.2}ms ledger_bytes={bytes}", std::env::consts::OS, elapsed[0], elapsed[20]);
            best_median_ms = best_median_ms.min(median_ms);
            if median_ms < 50.0 {
                break;
            }
        }
        assert!(best_median_ms < 50.0, "best median {best_median_ms:.2} ms exceeds 50 ms");
    }
}
