//! Role-contract eval: hand-labelled stress cases graded deterministically.
//!
//! Two case sets live in `tools/role-contracts/`: `cases.json` (the tuning
//! set the detectors were developed against) and `holdout.json` (written
//! before the tuned detectors ran, never tuned against). One command runs
//! both and fails on any regression against the committed scorecard
//! `tools/role-contracts/scorecards/latest.json`:
//!
//! ```text
//! cd src-tauri && cargo test --lib role_contract::eval
//! ```
//!
//! Regenerate the dated scorecards after an intentional change (the
//! regression test then holds the new numbers):
//!
//! ```text
//! cd src-tauri && cargo test --lib role_contract::eval::write_scorecards -- --ignored --exact
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{check_message, RULESET_VERSION};

const SETS: [(&str, &str); 2] = [("tuning", "cases.json"), ("holdout", "holdout.json")];

#[derive(Debug, Deserialize)]
struct CaseFile {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    category: String,
    role: String,
    channel: String,
    #[serde(default)]
    status: Option<String>,
    text: String,
    expect_flag: bool,
    #[serde(default)]
    expect_rules: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Checker {
    /// Behaviour of the code before this change: no message-level contract
    /// check exists, so every message is admitted unflagged.
    Baseline,
    /// The contract rule check.
    Contracts,
}

impl Checker {
    fn name(self) -> &'static str {
        match self {
            Checker::Baseline => "baseline-no-message-check",
            Checker::Contracts => "contracts",
        }
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn load_cases(file: &str) -> Vec<Case> {
    let path = repo_root().join("tools/role-contracts").join(file);
    let source = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str::<CaseFile>(&source).expect("valid case file").cases
}

fn ratio(numerator: usize, denominator: usize) -> Value {
    if denominator == 0 {
        Value::Null
    } else {
        json!(((numerator as f64 / denominator as f64) * 10_000.0).round() / 10_000.0)
    }
}

#[derive(Default)]
struct Tally {
    cases: usize,
    violating: usize,
    caught: usize,
    flagged_violating: usize,
    clean: usize,
    false_positives: usize,
}

impl Tally {
    fn add(&mut self, expect_flag: bool, flagged: bool, pass: bool) {
        self.cases += 1;
        if expect_flag {
            self.violating += 1;
            self.caught += usize::from(pass);
            self.flagged_violating += usize::from(flagged);
        } else {
            self.clean += 1;
            self.false_positives += usize::from(flagged);
        }
    }

    fn summary(&self) -> Value {
        json!({
            "cases": self.cases,
            "violating_cases": self.violating,
            "clean_cases": self.clean,
            "caught": self.caught,
            "catch_rate": ratio(self.caught, self.violating),
            "flag_rate_any_rule": ratio(self.flagged_violating, self.violating),
            "false_positives": self.false_positives,
            "false_positive_rate": ratio(self.false_positives, self.clean),
        })
    }
}

fn run_case(checker: Checker, case: &Case) -> (bool, Vec<String>) {
    let declares_done = case.channel == "heartbeat" && case.status.as_deref() == Some("completed");
    match checker {
        Checker::Baseline => (false, Vec::new()),
        Checker::Contracts => {
            let result = check_message(&case.role, &case.text, declares_done)
                .unwrap_or_else(|| panic!("{}: role {} has no contract", case.id, case.role));
            let mut rules: Vec<String> = result.rule_ids().into_iter().map(str::to_string).collect();
            rules.sort();
            rules.dedup();
            (result.flagged(), rules)
        }
    }
}

fn scorecard(checker: Checker, date: &str) -> Value {
    let mut rows = Vec::new();
    let mut overall = Tally::default();
    let mut sets = Map::new();
    let mut by_category: BTreeMap<String, Tally> = BTreeMap::new();
    for (set, file) in SETS {
        let mut tally = Tally::default();
        for case in load_cases(file) {
            let (flagged, rules) = run_case(checker, &case);
            let rules_ok = case.expect_rules.iter().all(|rule| rules.contains(rule));
            let pass = flagged == case.expect_flag && (!case.expect_flag || rules_ok);
            tally.add(case.expect_flag, flagged, pass);
            overall.add(case.expect_flag, flagged, pass);
            by_category
                .entry(case.category.clone())
                .or_default()
                .add(case.expect_flag, flagged, pass);
            rows.push(json!({
                "set": set,
                "id": case.id,
                "category": case.category,
                "role": case.role,
                "expect_flag": case.expect_flag,
                "flagged": flagged,
                "rules": rules,
                "pass": pass,
            }));
        }
        sets.insert(set.to_string(), tally.summary());
    }
    json!({
        "schema": "hive.role-contract-scorecard/v1",
        "date": date,
        "checker": checker.name(),
        "ruleset_version": RULESET_VERSION,
        "overall": overall.summary(),
        "sets": sets,
        "by_category": by_category
            .into_iter()
            .map(|(category, tally)| (category, tally.summary()))
            .collect::<Map<String, Value>>(),
        "results": rows,
    })
}

fn scorecard_dir() -> PathBuf {
    repo_root().join("tools/role-contracts/scorecards")
}

fn number(value: &Value, path: &[&str]) -> f64 {
    path.iter().fold(value, |node, key| &node[*key]).as_f64().unwrap_or(0.0)
}

#[test]
fn role_contract_eval_has_no_regression() {
    let current = scorecard(Checker::Contracts, "now");
    let path = scorecard_dir().join("latest.json");
    let committed: Value = serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display())),
    )
    .expect("valid latest.json");

    let key = |row: &Value| format!("{}/{}", row["set"].as_str().unwrap(), row["id"].as_str().unwrap());
    let passed_before: Vec<String> = committed["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["pass"] == true)
        .map(key)
        .collect();
    let regressed: Vec<String> = current["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["pass"] == false && passed_before.contains(&key(row)))
        .map(|row| format!("{} (flagged={}, rules={})", key(row), row["flagged"], row["rules"]))
        .collect();
    assert!(regressed.is_empty(), "cases that passed in latest.json now fail: {regressed:#?}");

    for (set, _) in SETS {
        assert_eq!(
            current["sets"][set]["cases"], committed["sets"][set]["cases"],
            "{set} cases changed: regenerate the scorecards with write_scorecards"
        );
        let catch = ["sets", set, "catch_rate"];
        let fp = ["sets", set, "false_positive_rate"];
        assert!(
            number(&current, &catch) >= number(&committed, &catch),
            "{set} catch rate fell: {} < {}",
            number(&current, &catch),
            number(&committed, &catch)
        );
        assert!(
            number(&current, &fp) <= number(&committed, &fp),
            "{set} false-positive rate rose: {} > {}",
            number(&current, &fp),
            number(&committed, &fp)
        );
    }
}

#[test]
fn baseline_flags_nothing() {
    let baseline = scorecard(Checker::Baseline, "now");
    assert_eq!(baseline["overall"]["catch_rate"], json!(0.0));
    assert_eq!(baseline["overall"]["false_positives"], json!(0));
}

#[test]
#[ignore = "writes tools/role-contracts/scorecards; run explicitly after an intentional change"]
fn write_scorecards() {
    let date = std::env::var("HIVE_ROLE_EVAL_DATE")
        .unwrap_or_else(|_| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let dir = scorecard_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: String, value: &Value| {
        let mut text = serde_json::to_string_pretty(value).unwrap();
        text.push('\n');
        std::fs::write(dir.join(name), text).unwrap();
    };
    write(format!("{date}-baseline.json"), &scorecard(Checker::Baseline, &date));
    let contracts = scorecard(Checker::Contracts, &date);
    write(format!("{date}-contracts.json"), &contracts);
    write("latest.json".to_string(), &contracts);
    println!("{}", serde_json::to_string_pretty(&contracts["sets"]).unwrap());
}

/// Print every failing case without writing anything.
#[test]
#[ignore = "diagnostic"]
fn print_failures() {
    let current = scorecard(Checker::Contracts, "now");
    for row in current["results"].as_array().unwrap() {
        if row["pass"] == false {
            println!("{row}");
        }
    }
    println!("{}", serde_json::to_string_pretty(&current["sets"]).unwrap());
}
