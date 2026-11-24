//! Role-contract eval: hand-labelled stress cases graded deterministically.
//!
//! Two case sets. The **tuning** set the detectors were developed against is
//! in the repo at `tools/role-contracts/cases.json`. The **held-out** set is
//! not in the repo: evaluated agents run in hive worktrees and must not be
//! able to read the answer key. It loads from `HIVE_ROLE_CONTRACT_HOLDOUT`
//! (a file path), else `<app data>/role-contracts/holdout.json`
//! (`%APPDATA%\hive-manager\role-contracts\holdout.json` on Windows). See
//! `tools/role-contracts/README.md` for how to restore it.
//!
//! The tuning set runs in every `cargo test` and fails on any regression
//! against the committed scorecard `tools/role-contracts/scorecards/latest.json`:
//!
//! ```text
//! cd src-tauri && cargo test --lib role_contract::eval
//! ```
//!
//! The held-out run needs the external file and fails, naming the path, when
//! it is absent. It compares per case with `holdout-latest.json` beside the
//! held-out file, and in aggregate with the committed scorecard:
//!
//! ```text
//! cd src-tauri && cargo test --lib role_contract::eval::holdout -- --ignored
//! ```
//!
//! Regenerate the scorecards after an intentional change (needs the held-out
//! file; the regression tests then hold the new numbers):
//!
//! ```text
//! cd src-tauri && cargo test --lib role_contract::eval::write_scorecards -- --ignored --exact
//! ```

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{check_message, RULESET_VERSION};

const TUNING_FILE: &str = "cases.json";
pub(crate) const HOLDOUT_ENV: &str = "HIVE_ROLE_CONTRACT_HOLDOUT";
const HOLDOUT_FILE: &str = "holdout.json";
/// Per-case held-out baseline, kept beside the held-out file, never in the repo.
const HOLDOUT_SCORECARD: &str = "holdout-latest.json";

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

/// Cited commits and files in the cases resolve against this repository.
fn evidence_roots() -> Vec<PathBuf> {
    vec![repo_root()]
}

fn parse_cases(path: &Path) -> Result<Vec<Case>, String> {
    let source = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str::<CaseFile>(&source)
        .map(|file| file.cases)
        .map_err(|error| format!("{}: invalid case file: {error}", path.display()))
}

fn tuning_cases() -> Vec<Case> {
    parse_cases(&repo_root().join("tools/role-contracts").join(TUNING_FILE)).unwrap_or_else(|error| panic!("{error}"))
}

/// Where the held-out set lives: the override, else the app data directory.
fn holdout_path_from(override_path: Option<OsString>, app_data: Option<PathBuf>) -> Option<PathBuf> {
    override_path
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| app_data.map(|dir| dir.join("role-contracts").join(HOLDOUT_FILE)))
}

fn holdout_path() -> Option<PathBuf> {
    holdout_path_from(
        std::env::var_os(HOLDOUT_ENV),
        crate::storage::SessionStorage::get_app_data_dir().ok(),
    )
}

fn load_holdout_from(path: Option<&Path>) -> Result<Vec<Case>, String> {
    let path = path.ok_or_else(|| {
        format!("held-out set location unknown: set {HOLDOUT_ENV} or APPDATA (see tools/role-contracts/README.md)")
    })?;
    if !path.is_file() {
        return Err(format!(
            "held-out set not found at {}: it is kept outside the repo; restore it there or set {HOLDOUT_ENV} \
             (see tools/role-contracts/README.md)",
            path.display()
        ));
    }
    parse_cases(path)
}

fn load_holdout() -> Vec<Case> {
    load_holdout_from(holdout_path().as_deref()).unwrap_or_else(|error| panic!("{error}"))
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
            let result = check_message(&case.role, &case.text, declares_done, &evidence_roots())
                .unwrap_or_else(|| panic!("{}: role {} has no contract", case.id, case.role));
            let mut rules: Vec<String> = result.rule_ids().into_iter().map(str::to_string).collect();
            rules.sort();
            rules.dedup();
            (result.flagged(), rules)
        }
    }
}

/// Grade one set: its summary, per-category tallies, and per-case rows.
fn grade(checker: Checker, set: &str, cases: &[Case]) -> (Value, BTreeMap<String, Tally>, Vec<Value>) {
    let mut tally = Tally::default();
    let mut by_category: BTreeMap<String, Tally> = BTreeMap::new();
    let mut rows = Vec::new();
    for case in cases {
        let (flagged, rules) = run_case(checker, case);
        let rules_ok = case.expect_rules.iter().all(|rule| rules.contains(rule));
        let pass = flagged == case.expect_flag && (!case.expect_flag || rules_ok);
        tally.add(case.expect_flag, flagged, pass);
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
    (tally.summary(), by_category, rows)
}

fn card(checker: Checker, date: &str, sets: Map<String, Value>, by_category: BTreeMap<String, Tally>, overall: Value, rows: Vec<Value>) -> Value {
    json!({
        "schema": "hive.role-contract-scorecard/v1",
        "date": date,
        "checker": checker.name(),
        "ruleset_version": RULESET_VERSION,
        "overall": overall,
        "sets": sets,
        "by_category": by_category
            .into_iter()
            .map(|(category, tally)| (category, tally.summary()))
            .collect::<Map<String, Value>>(),
        "results": rows,
    })
}

/// The committed scorecard: tuning rows only. When the held-out set is given,
/// only its aggregate summary is added under `sets.holdout`, never its rows.
fn scorecard(checker: Checker, date: &str, holdout: Option<&[Case]>) -> Value {
    let (summary, by_category, rows) = grade(checker, "tuning", &tuning_cases());
    let mut sets = Map::new();
    sets.insert("tuning".to_string(), summary.clone());
    if let Some(cases) = holdout {
        sets.insert("holdout".to_string(), grade(checker, "holdout", cases).0);
    }
    card(checker, date, sets, by_category, summary, rows)
}

/// The held-out scorecard with per-case rows. Written only beside the
/// held-out file, outside the repo.
fn holdout_scorecard(checker: Checker, date: &str, cases: &[Case]) -> Value {
    let (summary, by_category, rows) = grade(checker, "holdout", cases);
    let mut sets = Map::new();
    sets.insert("holdout".to_string(), summary.clone());
    card(checker, date, sets, by_category, summary, rows)
}

fn scorecard_dir() -> PathBuf {
    repo_root().join("tools/role-contracts/scorecards")
}

fn number(value: &Value, path: &[&str]) -> f64 {
    path.iter().fold(value, |node, key| &node[*key]).as_f64().unwrap_or(0.0)
}

fn read_card(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display())))
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn key(row: &Value) -> String {
    format!("{}/{}", row["set"].as_str().unwrap(), row["id"].as_str().unwrap())
}

/// Cases that passed in `committed` and fail in `current`.
fn regressions(current: &Value, committed: &Value) -> Vec<String> {
    let passed_before: Vec<String> = committed["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["pass"] == true)
        .map(key)
        .collect();
    current["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["pass"] == false && passed_before.contains(&key(row)))
        .map(|row| format!("{} (flagged={}, rules={})", key(row), row["flagged"], row["rules"]))
        .collect()
}

fn assert_rates_hold(set: &str, current: &Value, committed: &Value) {
    assert_eq!(
        current["sets"][set]["cases"], committed["sets"][set]["cases"],
        "{set} cases changed: regenerate the scorecards with write_scorecards"
    );
    let catch = ["sets", set, "catch_rate"];
    let fp = ["sets", set, "false_positive_rate"];
    assert!(
        number(current, &catch) >= number(committed, &catch),
        "{set} catch rate fell: {} < {}",
        number(current, &catch),
        number(committed, &catch)
    );
    assert!(
        number(current, &fp) <= number(committed, &fp),
        "{set} false-positive rate rose: {} > {}",
        number(current, &fp),
        number(committed, &fp)
    );
}

#[test]
fn role_contract_eval_has_no_regression() {
    let current = scorecard(Checker::Contracts, "now", None);
    let committed = read_card(&scorecard_dir().join("latest.json"));
    let regressed = regressions(&current, &committed);
    assert!(regressed.is_empty(), "cases that passed in latest.json now fail: {regressed:#?}");
    assert_rates_hold("tuning", &current, &committed);
}

#[test]
#[ignore = "needs the held-out set, which lives outside the repo; see tools/role-contracts/README.md"]
fn holdout_has_no_regression() {
    let path = holdout_path();
    let cases = load_holdout();
    let current = holdout_scorecard(Checker::Contracts, "now", &cases);
    let committed = read_card(&scorecard_dir().join("latest.json"));
    assert_rates_hold("holdout", &current, &committed);
    let beside = path.unwrap().with_file_name(HOLDOUT_SCORECARD);
    if beside.is_file() {
        let regressed = regressions(&current, &read_card(&beside));
        assert!(regressed.is_empty(), "held-out cases that passed before now fail: {regressed:#?}");
    }
}

#[test]
fn holdout_loads_from_the_configured_location() {
    let temp = tempfile::tempdir().unwrap();
    let app_data = temp.path().join("hive-manager");
    let default_path = holdout_path_from(None, Some(app_data.clone())).unwrap();
    assert_eq!(default_path, app_data.join("role-contracts").join(HOLDOUT_FILE));
    let error = load_holdout_from(Some(&default_path)).unwrap_err();
    assert!(error.contains("not found") && error.contains(HOLDOUT_ENV), "{error}");
    assert!(load_holdout_from(None).unwrap_err().contains(HOLDOUT_ENV));

    std::fs::create_dir_all(default_path.parent().unwrap()).unwrap();
    std::fs::write(
        &default_path,
        r#"{"cases":[{"id":"X-1","category":"clean","role":"queen","channel":"conversation","text":"Reading the plan.","expect_flag":false}]}"#,
    )
    .unwrap();
    let cases = load_holdout_from(Some(&default_path)).unwrap();
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].id, "X-1");

    let override_path = temp.path().join("elsewhere.json");
    assert_eq!(
        holdout_path_from(Some(override_path.clone().into_os_string()), Some(app_data)),
        Some(override_path)
    );
}

#[test]
fn repo_tree_holds_no_heldout_labels() {
    let dir = repo_root().join("tools/role-contracts");
    assert!(!dir.join(HOLDOUT_FILE).exists(), "the held-out set must not be in the repo");
    let mut files = vec![dir.join("README.md"), dir.join(TUNING_FILE)];
    files.extend(std::fs::read_dir(scorecard_dir()).unwrap().map(|entry| entry.unwrap().path()));
    let heldout_id = regex::Regex::new(r"\bH-[A-Z]\d{2}\b").unwrap();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!heldout_id.is_match(&text), "{} names a held-out case", file.display());
        if file.extension().is_some_and(|ext| ext == "json") && file.starts_with(scorecard_dir()) {
            let value: Value = serde_json::from_str(&text).unwrap();
            let rows = value["results"].as_array().cloned().unwrap_or_default();
            assert!(
                rows.iter().all(|row| row["set"] != "holdout"),
                "{} carries held-out rows",
                file.display()
            );
        }
    }
}

#[test]
fn baseline_flags_nothing() {
    let baseline = scorecard(Checker::Baseline, "now", None);
    assert_eq!(baseline["overall"]["catch_rate"], json!(0.0));
    assert_eq!(baseline["overall"]["false_positives"], json!(0));
}

#[test]
#[ignore = "writes tools/role-contracts/scorecards and the external held-out scorecard; run explicitly after an intentional change"]
fn write_scorecards() {
    let date = std::env::var("HIVE_ROLE_EVAL_DATE")
        .unwrap_or_else(|_| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let holdout_file = holdout_path();
    let holdout = load_holdout();
    let dir = scorecard_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let write = |path: PathBuf, value: &Value| {
        let mut text = serde_json::to_string_pretty(value).unwrap();
        text.push('\n');
        std::fs::write(path, text).unwrap();
    };
    write(dir.join(format!("{date}-baseline.json")), &scorecard(Checker::Baseline, &date, Some(&holdout)));
    let contracts = scorecard(Checker::Contracts, &date, Some(&holdout));
    write(dir.join(format!("{date}-contracts.json")), &contracts);
    write(dir.join("latest.json"), &contracts);
    write(
        holdout_file.unwrap().with_file_name(HOLDOUT_SCORECARD),
        &holdout_scorecard(Checker::Contracts, &date, &holdout),
    );
    println!("{}", serde_json::to_string_pretty(&contracts["sets"]).unwrap());
}

/// Print every failing case without writing anything.
#[test]
#[ignore = "diagnostic"]
fn print_failures() {
    let mut rows = scorecard(Checker::Contracts, "now", None)["results"].as_array().cloned().unwrap();
    if let Ok(cases) = load_holdout_from(holdout_path().as_deref()) {
        rows.extend(holdout_scorecard(Checker::Contracts, "now", &cases)["results"].as_array().cloned().unwrap());
    }
    for row in rows {
        if row["pass"] == false {
            println!("{row}");
        }
    }
}
