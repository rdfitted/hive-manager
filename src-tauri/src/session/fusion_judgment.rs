use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::judgment::ledger::{
    DecisionInput, Judge, JudgmentLedger, JudgmentMode, OutcomeInput, OutcomeSource,
};
use crate::storage::SessionStorage;

const FUSION_RANK_NAMESPACE: Uuid = Uuid::from_u128(0x88548c6b6cfa5d2d8cfb6af3b8d7cc80);

pub(super) struct FusionCandidate<'a> {
    pub name: &'a str,
    pub slug: &'a str,
}

pub(super) fn decision_id(session_id: &str) -> String {
    Uuid::new_v5(
        &FUSION_RANK_NAMESPACE,
        format!("hive.fusion.rank:{session_id}").as_bytes(),
    )
    .to_string()
}

fn slugify_variant_name(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;

    for ch in name.trim().chars() {
        let lowered = ch.to_ascii_lowercase();
        if lowered.is_ascii_alphanumeric() {
            out.push(lowered);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }

    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "variant".to_string()
    } else {
        out
    }
}

pub(super) fn parse_winner<'a>(
    report: &str,
    variants: &'a [FusionCandidate<'a>],
) -> Result<&'a str, String> {
    let mut in_recommendation = false;
    for line in report.lines() {
        let line = line.trim();
        if line.starts_with("## ") {
            if in_recommendation {
                break;
            }
            in_recommendation = line == "## Recommendation";
            continue;
        }
        if !in_recommendation {
            continue;
        }
        let Some(raw) = line.strip_prefix("Winner:") else {
            continue;
        };
        let requested = raw.trim();
        if requested.is_empty() {
            return Err("Fusion recommendation has an empty Winner: value".to_string());
        }
        let requested_slug = slugify_variant_name(requested);
        return variants
            .iter()
            .find(|variant| variant.name == requested || variant.slug == requested_slug)
            .map(|variant| variant.name)
            .ok_or_else(|| format!("Fusion recommendation names unknown winner: {requested}"));
    }
    Err("Fusion recommendation is missing Winner: under ## Recommendation".to_string())
}

fn ledger(storage: &SessionStorage) -> JudgmentLedger {
    JudgmentLedger::new(storage.base_dir().join("judgments").join("ledger.jsonl"))
}

pub(super) fn decision_input(
    session_id: &str,
    task_description: &str,
    variants: &[FusionCandidate<'_>],
    report: &str,
    cli: &str,
    model: Option<&str>,
) -> DecisionInput {
    let parsed = parse_winner(report, variants);
    let (answer, error) = match parsed {
        Ok(name) => (json!(name), None),
        Err(error) => (Value::Null, Some(error)),
    };
    let mut subject_ref = Map::new();
    subject_ref.insert("session_id".to_string(), json!(session_id));
    let model = model
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("{cli}/{value}"))
        .or_else(|| Some(cli.to_string()));

    DecisionInput {
        decision_id: Some(decision_id(session_id)),
        surface: "hive.fusion.rank".to_string(),
        subject_ref,
        observations: json!({
            "task_description": task_description,
            "variants": variants.iter().map(|variant| json!({
                "name": variant.name,
                "slug": variant.slug,
            })).collect::<Vec<_>>(),
        }),
        answer,
        question_id: Some("hive.fusion.rank".to_string()),
        question_version: None,
        judge: Judge::IncumbentLlm,
        model,
        sampling: None,
        mode: JudgmentMode::Shadow,
        probabilities: None,
        confidence: None,
        threshold_id: None,
        routed: "none".to_string(),
        latency_ms: None,
        cost_usd: None,
        error,
        extra: Map::new(),
    }
}

pub(super) fn record_decision(
    storage: Option<&SessionStorage>,
    session_id: &str,
    task_description: &str,
    variants: &[FusionCandidate<'_>],
    report: &str,
    cli: &str,
    model: Option<&str>,
) {
    let Some(storage) = storage else {
        return;
    };
    let input = decision_input(session_id, task_description, variants, report, cli, model);
    let _ = ledger(storage).record_decision(input);
}

pub(super) fn record_outcome(
    storage: Option<&SessionStorage>,
    session_id: &str,
    winner_name: &str,
) {
    let Some(storage) = storage else {
        return;
    };
    let _ = ledger(storage).record_outcome(OutcomeInput {
        decision_id: decision_id(session_id),
        label: json!(winner_name),
        source: OutcomeSource::Downstream,
        note: None,
        extra: Map::new(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn candidates<'a>() -> [FusionCandidate<'a>; 2] {
        [
            FusionCandidate {
                name: "Fast Path",
                slug: "fast-path",
            },
            FusionCandidate {
                name: "Careful Pass",
                slug: "careful-pass",
            },
        ]
    }

    fn rows(storage: &SessionStorage) -> Vec<Value> {
        let path = storage.base_dir().join("judgments").join("ledger.jsonl");
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn parses_exact_name_slug_unknown_and_missing_winner() {
        let variants = candidates();
        assert_eq!(
            parse_winner("## Recommendation\nWinner: Fast Path\nRationale: pick", &variants),
            Ok("Fast Path")
        );
        assert_eq!(
            parse_winner("## Recommendation\nWinner: careful-pass", &variants),
            Ok("Careful Pass")
        );
        assert!(parse_winner("## Recommendation\nWinner: Other", &variants)
            .unwrap_err()
            .contains("unknown winner"));
        assert!(parse_winner("## Recommendation\nRationale: none", &variants)
            .unwrap_err()
            .contains("missing Winner:"));
        assert!(parse_winner("## Other\nWinner: Fast Path", &variants).is_err());
        assert!(parse_winner(
            "## Recommendation\nRationale: none\n## Recommendation\nWinner: Fast Path",
            &variants,
        )
        .is_err());
        assert!(parse_winner("## Recommendation\nWinner: \nWinner: Fast Path", &variants)
            .is_err());
    }

    #[test]
    fn writes_blind_decision_and_joined_downstream_outcome() {
        let tmp = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(tmp.path().to_path_buf()).unwrap();
        let report = "## Recommendation\nWinner: Fast Path\nRationale: secret judge reasoning";
        let variants = candidates();
        record_decision(
            Some(&storage), "fusion-1", "Implement feature", &variants,
            report, "codex", Some("gpt-6-sol"),
        );
        let before = rows(&storage);
        assert_eq!(before.len(), 1);
        assert_eq!(before[0]["answer"], "Fast Path");
        assert_eq!(before[0]["model"], "codex/gpt-6-sol");
        assert_eq!(before[0]["judge"], "incumbent-llm");
        assert_eq!(before[0]["mode"], "shadow");
        assert_eq!(before[0]["routed"], "none");
        let evidence = std::fs::read_to_string(
            storage.base_dir().join("judgments").join(
                before[0]["state_ref"].as_str().unwrap()
            )
        ).unwrap();
        assert!(evidence.contains("Implement feature"));
        assert!(!evidence.contains("secret judge reasoning"));
        assert!(!evidence.contains("Winner:"));

        record_outcome(Some(&storage), "fusion-1", "Careful Pass");
        let after = rows(&storage);
        assert_eq!(after.len(), 2);
        assert_eq!(after[1]["decision_id"], before[0]["decision_id"]);
        assert_eq!(after[1]["label"], "Careful Pass");
        assert_eq!(after[1]["source"], "downstream");
    }

    #[test]
    fn missing_winner_writes_null_answer_and_no_storage_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(tmp.path().to_path_buf()).unwrap();
        let variants = candidates();
        record_decision(None, "fusion-2", "Task", &variants, "## Recommendation", "codex", None);
        record_outcome(None, "fusion-2", "Fast Path");
        assert!(!storage.base_dir().join("judgments").exists());
        record_decision(Some(&storage), "fusion-2", "Task", &variants, "## Recommendation", "codex", None);
        let recorded = rows(&storage);
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0]["answer"].is_null());
        assert!(recorded[0]["error"].as_str().unwrap().contains("missing Winner:"));
    }

    #[test]
    fn unknown_winner_keeps_the_decision_without_guessing() {
        let tmp = TempDir::new().unwrap();
        let storage = SessionStorage::new_with_base(tmp.path().to_path_buf()).unwrap();
        let variants = candidates();
        record_decision(
            Some(&storage),
            "fusion-unknown",
            "Task",
            &variants,
            "## Recommendation\nWinner: Not a Variant\nRationale: secret",
            "codex",
            None,
        );
        let recorded = rows(&storage);
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0]["answer"].is_null());
        assert!(recorded[0]["error"].as_str().unwrap().contains("unknown winner"));
    }

    fn rust_files(root: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_files(&path, files);
            } else if path.extension().is_some_and(|extension| extension.to_string_lossy() == "rs") {
                files.push(path);
            }
        }
    }

    #[test]
    fn only_operator_http_and_tauri_callers_can_select_a_winner() {
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&source_root, &mut files);
        let needle = [".select_fusion", "_winner("].concat();
        let mut callers = files
            .iter()
            .flat_map(|path| {
                let source = std::fs::read_to_string(path).unwrap();
                let name = path
                    .strip_prefix(&source_root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                std::iter::repeat(name).take(source.matches(&needle).count())
            })
            .collect::<Vec<_>>();
        callers.sort();
        assert_eq!(
            callers,
            ["commands/fusion_commands.rs", "http/handlers/sessions.rs"]
        );

        let controller = include_str!("controller.rs");
        let judge_prompt = controller
            .split_once("fn build_fusion_judge_prompt")
            .unwrap()
            .1
            .split_once("fn ")
            .unwrap()
            .0;
        assert!(!judge_prompt.contains("select-winner"));
        assert!(!include_str!("../templates/mod.rs").contains("select-winner"));
    }

    #[test]
    fn controller_hooks_follow_transition_and_successful_commit() {
        let controller = include_str!("controller.rs");
        let evaluation = controller
            .split_once("pub fn get_fusion_evaluation")
            .unwrap()
            .1
            .split_once("pub async fn on_debate_round_completed")
            .unwrap()
            .0;
        let guard = evaluation.find("s.state == SessionState::Judging").unwrap();
        let hook = evaluation.find("fusion_judgment::record_decision(").unwrap();
        assert!(guard < hook);
        assert_eq!(evaluation.matches("fusion_judgment::record_decision(").count(), 1);

        let merge = controller
            .split_once("pub fn select_fusion_winner")
            .unwrap()
            .1
            .split_once("fn terminate_worker")
            .unwrap()
            .0;
        let commit = merge.find("\"commit\",").unwrap();
        let outcome = merge.find("fusion_judgment::record_outcome(").unwrap();
        let cleanup = merge.find("cleanup_session_worktrees(&session)").unwrap();
        assert!(commit < outcome && outcome < cleanup);
        assert!(merge[commit..outcome].contains(")?;"));
    }
}
