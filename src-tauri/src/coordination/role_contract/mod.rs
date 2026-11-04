//! Role contracts and the admission-time rule check (ATC rule-check half).
//!
//! Each role carries a `contract` block in its `roles/*.md` front matter:
//! `must` lists required behaviors, and `must_never` lists prohibited acts.
//! Roles the runtime uses that have no role definition yet (QA worker,
//! sub-planner, fusion judge) carry a contract-only file in `roles/contracts/`.
//! The check maps a message to acts ([`acts::map_message`]) and compares them
//! with the role's contract and its `AuthorityScope`. Authority stays the
//! primary safeguard: this is an extra, message-level layer.
//!
//! Rollout is report-only. A violation writes a shadow row to the judgment
//! ledger store, `judgments/role-contracts.jsonl` (surface `hive.role-contract.message`), and the message is still
//! admitted. `HIVE_ROLE_CONTRACT_CHECK=off` disables the check. Blocking and
//! the LLM rewrite are not implemented; see `tools/role-contracts/README.md`.

pub mod acts;
#[cfg(test)]
mod eval;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map};
use sha2::{Digest, Sha256};

pub use acts::{Act, MessageActs};

use crate::judgment::ledger::{DecisionInput, Judge, JudgmentLedger, JudgmentMode};
use crate::orchestrator::org_graph::AuthorityScope;
use crate::pty::AgentRole;

pub const SURFACE: &str = "hive.role-contract.message";
pub const MODE_ENV: &str = "HIVE_ROLE_CONTRACT_CHECK";
pub const LEDGER_FILE: &str = "role-contracts.jsonl";
/// Bumped whenever detector patterns or contract semantics change, so ledger
/// rows from different rule sets are never pooled.
pub const RULESET_VERSION: &str = "role-contract-rules/v1";

#[cfg(test)]
pub(crate) static ROLE_CONTRACT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

const EMBEDDED_CONTRACT_SOURCES: &[(&str, &str)] = &[
    ("backend", include_str!("../../../../roles/backend.md")),
    ("frontend", include_str!("../../../../roles/frontend.md")),
    ("coherence", include_str!("../../../../roles/coherence.md")),
    ("simplify", include_str!("../../../../roles/simplify.md")),
    ("reviewer", include_str!("../../../../roles/reviewer.md")),
    ("reviewer-quick", include_str!("../../../../roles/reviewer-quick.md")),
    ("resolver", include_str!("../../../../roles/resolver.md")),
    ("tester", include_str!("../../../../roles/tester.md")),
    ("code-quality", include_str!("../../../../roles/code-quality.md")),
    ("researcher", include_str!("../../../../roles/researcher.md")),
    ("general", include_str!("../../../../roles/general.md")),
    ("master-planner", include_str!("../../../../roles/master-planner.md")),
    ("queen", include_str!("../../../../roles/queen.md")),
    ("evaluator", include_str!("../../../../roles/evaluator.md")),
    ("prince", include_str!("../../../../roles/prince.md")),
    ("qa-worker", include_str!("../../../../roles/contracts/qa-worker.md")),
    ("planner", include_str!("../../../../roles/contracts/planner.md")),
    ("fusion-judge", include_str!("../../../../roles/contracts/fusion-judge.md")),
];

/// A requirement that applies when the message performs `when`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MustRule {
    pub id: String,
    pub when: Act,
    pub requires: Requirement,
    pub rule: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Requirement {
    /// A commit SHA, test output, file reference, PR or issue number, or an
    /// explicit `evidence:` label.
    Evidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MustNeverRule {
    pub id: String,
    pub act: Act,
    pub rule: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleContract {
    #[serde(default)]
    pub must: Vec<MustRule>,
    #[serde(default)]
    pub must_never: Vec<MustNeverRule>,
}

/// The parts of a role document the check reads. Other definition fields are
/// owned by `org_graph::definitions` and are ignored here.
#[derive(Debug, Clone, Deserialize)]
struct ContractDocument {
    id: String,
    #[serde(default)]
    authority: AuthorityScope,
    contract: Option<RoleContract>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedContract {
    pub role_id: String,
    pub authority: AuthorityScope,
    pub contract: RoleContract,
}

fn parse_contract_document(source: &str) -> Result<ContractDocument, String> {
    let trimmed = source.trim();
    let json = trimmed
        .strip_prefix("---")
        .and_then(|rest| rest.split_once("---"))
        .map(|(front_matter, _)| front_matter.trim())
        .ok_or_else(|| "role document front matter is missing".to_string())?;
    serde_json::from_str(json).map_err(|error| error.to_string())
}

fn contracts() -> &'static BTreeMap<String, ResolvedContract> {
    static CONTRACTS: OnceLock<BTreeMap<String, ResolvedContract>> = OnceLock::new();
    CONTRACTS.get_or_init(|| {
        let mut map = BTreeMap::new();
        for (id, source) in EMBEDDED_CONTRACT_SOURCES {
            match parse_contract_document(source) {
                Ok(document) if document.id == *id => {
                    let Some(contract) = document.contract else {
                        tracing::warn!(role = id, "role definition has no contract block");
                        continue;
                    };
                    map.insert(
                        id.to_string(),
                        ResolvedContract {
                            role_id: id.to_string(),
                            authority: document.authority,
                            contract,
                        },
                    );
                }
                Ok(document) => {
                    tracing::warn!(role = id, declared = %document.id, "role contract id mismatch");
                }
                Err(error) => tracing::warn!(role = id, %error, "role contract is unreadable"),
            }
        }
        map
    })
}

pub fn contract_for(role_id: &str) -> Option<&'static ResolvedContract> {
    contracts().get(role_id)
}

/// Map a runtime agent to the role whose contract governs it. The resolved
/// role-definition id wins; positional roles fall back to their kind.
/// Scratch shells are operator-owned and are not checked.
pub fn role_id_for_agent(
    role: &AgentRole,
    role_definition_id: Option<&str>,
    worker_role_type: Option<&str>,
) -> Option<String> {
    if let Some(id) = role_definition_id.filter(|id| contract_for(id).is_some()) {
        return Some(id.to_string());
    }
    let id = match role {
        AgentRole::Queen => "queen",
        AgentRole::MasterPlanner => "master-planner",
        AgentRole::Evaluator => "evaluator",
        AgentRole::Prince => "prince",
        AgentRole::QaWorker { .. } => "qa-worker",
        AgentRole::Planner { .. } => "planner",
        AgentRole::Judge { .. } => "fusion-judge",
        AgentRole::Fusion { .. } => "general",
        AgentRole::Worker { .. } => {
            let declared = worker_role_type.map(|value| value.trim().to_ascii_lowercase());
            return Some(
                declared
                    .filter(|value| contract_for(value).is_some())
                    .unwrap_or_else(|| "general".to_string()),
            );
        }
        AgentRole::ScratchShell => return None,
    };
    Some(id.to_string())
}

/// Where a message entered the hive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Conversation,
    QueenInject,
    EvaluatorInject,
    Heartbeat,
    EvaluatorVerdict,
    PrinceVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationKind {
    MustNever,
    Authority,
    MissingRequirement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    pub rule_id: String,
    pub kind: ViolationKind,
    pub act: Act,
    pub rule: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckResult {
    pub role_id: String,
    pub acts: MessageActs,
    pub violations: Vec<Violation>,
}

impl CheckResult {
    pub fn flagged(&self) -> bool {
        !self.violations.is_empty()
    }

    pub fn rule_ids(&self) -> Vec<&str> {
        self.violations.iter().map(|violation| violation.rule_id.as_str()).collect()
    }
}

/// Acts that the role's `AuthorityScope` must grant. Keeping these derived
/// from authority, rather than restated in each contract, means the two can
/// never disagree.
fn authority_gate(act: Act, authority: &AuthorityScope) -> Option<(&'static str, &'static str)> {
    match act {
        Act::ClaimsCodeEdit if !authority.may_commit => Some((
            "authority.may_commit",
            "Editing or committing code requires may_commit.",
        )),
        Act::ClaimsPush if !authority.may_push => {
            Some(("authority.may_push", "Pushing requires may_push."))
        }
        Act::ClaimsExternalPost if !authority.may_push => Some((
            "authority.may_push",
            "Posting outside the hive (PRs, issue or PR comments) requires may_push.",
        )),
        Act::Spawns if !authority.may_spawn_subordinates => Some((
            "authority.may_spawn_subordinates",
            "Spawning hive principals requires may_spawn_subordinates.",
        )),
        _ => None,
    }
}

/// The fixed rule check: no model, no I/O.
pub fn check(contract: &ResolvedContract, acts: MessageActs) -> CheckResult {
    let mut violations = Vec::new();
    for hit in &acts.hits {
        if let Some((rule_id, rule)) = authority_gate(hit.act, &contract.authority) {
            if !violations.iter().any(|v: &Violation| v.rule_id == rule_id && v.act == hit.act) {
                violations.push(Violation {
                    rule_id: rule_id.to_string(),
                    kind: ViolationKind::Authority,
                    act: hit.act,
                    rule: rule.to_string(),
                });
            }
        }
        for never in contract.contract.must_never.iter().filter(|never| never.act == hit.act) {
            violations.push(Violation {
                rule_id: never.id.clone(),
                kind: ViolationKind::MustNever,
                act: hit.act,
                rule: never.rule.clone(),
            });
        }
    }
    for must in &contract.contract.must {
        let applies = acts.contains(must.when);
        let satisfied = match must.requires {
            Requirement::Evidence => acts.has_evidence,
        };
        if applies && !satisfied {
            violations.push(Violation {
                rule_id: must.id.clone(),
                kind: ViolationKind::MissingRequirement,
                act: must.when,
                rule: must.rule.clone(),
            });
        }
    }
    CheckResult {
        role_id: contract.role_id.clone(),
        acts,
        violations,
    }
}

/// Map and check one message for a role. `None` means the role has no contract.
pub fn check_message(role_id: &str, text: &str, transport_declares_done: bool) -> Option<CheckResult> {
    let contract = contract_for(role_id)?;
    Some(check(contract, acts::map_message(text, transport_declares_done)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    Report,
}

/// `off` (any case) disables the check; anything else is report-only.
/// There is deliberately no blocking value yet.
pub fn mode() -> Mode {
    match std::env::var(MODE_ENV) {
        Ok(value) if value.trim().eq_ignore_ascii_case("off") => Mode::Off,
        _ => Mode::Report,
    }
}

/// Role-contract rows use the judgment-ledger writer and record schema but
/// their own file, so the shared decision ledger keeps its pinned shape (the QA
/// verdict path must not create or extend `ledger.jsonl`).
pub fn ledger_path(base_dir: &Path) -> std::path::PathBuf {
    base_dir.join("judgments").join(LEDGER_FILE)
}

/// Who a flagged message would be escalated to once rewrite/blocking ship.
fn escalation_target(role_id: &str) -> &'static str {
    match role_id {
        "queen" | "evaluator" | "prince" | "master-planner" => "operator",
        "qa-worker" => "evaluator",
        _ => "queen",
    }
}

fn message_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One message presented for admission.
pub struct Admission<'a> {
    pub session_id: &'a str,
    pub agent_id: &'a str,
    pub role_id: &'a str,
    pub channel: Channel,
    pub text: &'a str,
    pub transport_declares_done: bool,
}

/// Report-only admission: check, log a violation to the judgment ledger, and
/// always admit. Returns the check result for callers and tests.
pub fn observe(ledger_path: &Path, admission: Admission<'_>) -> Option<CheckResult> {
    if mode() == Mode::Off {
        return None;
    }
    let result = check_message(
        admission.role_id,
        admission.text,
        admission.transport_declares_done,
    )?;
    if !result.flagged() {
        return Some(result);
    }
    let escalate_to = escalation_target(admission.role_id);
    tracing::warn!(
        session = admission.session_id,
        agent = admission.agent_id,
        role = admission.role_id,
        channel = ?admission.channel,
        rules = ?result.rule_ids(),
        "role contract violation (report-only; message admitted)"
    );
    let mut subject_ref = Map::new();
    subject_ref.insert("session_id".into(), json!(admission.session_id));
    subject_ref.insert("agent_id".into(), json!(admission.agent_id));
    subject_ref.insert("role".into(), json!(admission.role_id));
    subject_ref.insert("channel".into(), json!(admission.channel));
    subject_ref.insert("message_sha256".into(), json!(message_hash(admission.text)));
    let mut extra = Map::new();
    extra.insert("escalate_to".into(), json!(escalate_to));
    extra.insert("enforcement".into(), json!("report_only"));
    JudgmentLedger::new(ledger_path.to_path_buf()).record_decision(DecisionInput {
        decision_id: None,
        surface: SURFACE.into(),
        subject_ref,
        observations: json!({
            "acts": result.acts.hits,
            "has_evidence": result.acts.has_evidence,
            "message_chars": admission.text.chars().count(),
            "transport_declares_done": admission.transport_declares_done,
        }),
        answer: json!({
            "result": "violation",
            "rules": result.violations,
            "action": "admitted",
            "would_route": "escalate",
        }),
        question_id: Some("role_contract".into()),
        question_version: Some(RULESET_VERSION.into()),
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
    Some(result)
}

/// HTTP seam helper: resolve the sender's role from the live roster and run
/// [`observe`]. Unknown agents and operator-owned shells are skipped.
pub(crate) fn observe_from_state(
    state: &crate::http::state::AppState,
    session_id: &str,
    agent_id: &str,
    channel: Channel,
    text: &str,
    transport_declares_done: bool,
) -> Option<CheckResult> {
    if mode() == Mode::Off || (text.trim().is_empty() && !transport_declares_done) {
        return None;
    }
    let role_id = role_id_for_session_agent(state, session_id, agent_id)?;
    let ledger_path = ledger_path(state.storage.base_dir());
    observe(
        &ledger_path,
        Admission {
            session_id,
            agent_id,
            role_id: &role_id,
            channel,
            text,
            transport_declares_done,
        },
    )
}

/// Variant for endpoints whose sender role is fixed by the route (verdicts).
pub(crate) fn observe_fixed_role(
    state: &crate::http::state::AppState,
    session_id: &str,
    role_id: &str,
    channel: Channel,
    text: &str,
) -> Option<CheckResult> {
    if mode() == Mode::Off || text.trim().is_empty() {
        return None;
    }
    let ledger_path = ledger_path(state.storage.base_dir());
    observe(
        &ledger_path,
        Admission {
            session_id,
            agent_id: role_id,
            role_id,
            channel,
            text,
            transport_declares_done: false,
        },
    )
}

fn role_id_for_session_agent(
    state: &crate::http::state::AppState,
    session_id: &str,
    agent_id: &str,
) -> Option<String> {
    let canonical = crate::http::handlers::canonical_agent_id(session_id, agent_id);
    let session = state.session_controller.read().get_session(session_id)?;
    let agent = session
        .agents
        .iter()
        .find(|agent| agent.id == canonical || agent.id == agent_id)?;
    role_id_for_agent(
        &agent.role,
        agent.role_definition_id.as_deref(),
        agent.config.role.as_ref().map(|role| role.role_type.as_str()),
    )
}

/// Join a verdict's free-text parts into one admission text.
pub(crate) fn join_parts<'a>(parts: impl IntoIterator<Item = Option<&'a str>>) -> String {
    parts
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn role_files() -> Vec<String> {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut ids = Vec::new();
        for dir in ["roles", "roles/contracts"] {
            for entry in std::fs::read_dir(repo.join(dir)).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                    ids.push(path.file_stem().unwrap().to_str().unwrap().to_string());
                }
            }
        }
        ids.sort();
        ids
    }

    #[test]
    fn every_role_file_is_embedded_and_carries_a_contract() {
        let files = role_files();
        let embedded: Vec<String> = {
            let mut ids: Vec<String> =
                EMBEDDED_CONTRACT_SOURCES.iter().map(|(id, _)| id.to_string()).collect();
            ids.sort();
            ids
        };
        assert_eq!(files, embedded, "roles/ and EMBEDDED_CONTRACT_SOURCES drifted");
        for id in &files {
            let resolved = contract_for(id).unwrap_or_else(|| panic!("{id}: no contract block"));
            assert!(
                !resolved.contract.must_never.is_empty(),
                "{id}: a contract must prohibit something"
            );
        }
    }

    #[test]
    fn contracts_do_not_restate_authority_derived_rules() {
        // Push, commit, external posting and spawning come from AuthorityScope.
        // A must_never entry for one of those is allowed only when it is
        // STRICTER than the role's authority (the Prince: may_push is granted
        // by the definition, but its prompt reserves push for the Queen).
        for resolved in contracts().values() {
            for never in &resolved.contract.must_never {
                if let Some((rule_id, _)) = authority_gate(never.act, &resolved.authority) {
                    panic!(
                        "{}: {} duplicates {rule_id}; authority already denies it",
                        resolved.role_id, never.id
                    );
                }
            }
        }
    }

    #[test]
    fn every_runtime_role_maps_to_a_contract() {
        let roles = [
            AgentRole::Queen,
            AgentRole::MasterPlanner,
            AgentRole::Evaluator,
            AgentRole::Prince,
            AgentRole::QaWorker { index: 1, parent: None },
            AgentRole::Planner { index: 1 },
            AgentRole::Judge { session_id: "s".into() },
            AgentRole::Fusion { variant: "a".into() },
            AgentRole::Worker { index: 1, parent: None },
        ];
        for role in roles {
            let id = role_id_for_agent(&role, None, Some("backend")).unwrap();
            assert!(contract_for(&id).is_some(), "{role:?} -> {id} has no contract");
        }
        assert_eq!(role_id_for_agent(&AgentRole::ScratchShell, None, None), None);
        assert_eq!(
            role_id_for_agent(&AgentRole::Worker { index: 1, parent: None }, None, Some("custom-thing")),
            Some("general".to_string())
        );
        assert_eq!(
            role_id_for_agent(&AgentRole::Worker { index: 1, parent: None }, Some("reviewer"), None),
            Some("reviewer".to_string())
        );
    }

    #[test]
    fn violation_is_logged_report_only_and_kill_switch_disables_it() {
        let _guard = ROLE_CONTRACT_ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let ledger = temp.path().join("judgments").join("ledger.jsonl");
        let admission = |text| Admission {
            session_id: "s1",
            agent_id: "s1-worker-1",
            role_id: "evaluator",
            channel: Channel::Conversation,
            text,
            transport_declares_done: false,
        };

        std::env::remove_var(MODE_ENV);
        let result = observe(&ledger, admission("I patched src/foo.rs myself.")).unwrap();
        assert!(result.flagged());
        let rows = std::fs::read_to_string(&ledger).unwrap();
        let row: Value = serde_json::from_str(rows.lines().next().unwrap()).unwrap();
        assert_eq!(row["surface"], SURFACE);
        assert_eq!(row["mode"], "shadow");
        assert_eq!(row["judge"], "code");
        assert_eq!(row["answer"]["action"], "admitted");
        assert_eq!(row["answer"]["rules"][0]["rule_id"], "authority.may_commit");
        assert_eq!(row["escalate_to"], "operator");
        assert!(!rows.contains("patched src/foo.rs myself"), "raw message must not enter the ledger row");

        // Clean messages write nothing.
        assert!(!observe(&ledger, admission("Verdict: FAIL. cargo test exits 101 at a.rs:3.")).unwrap().flagged());
        assert_eq!(std::fs::read_to_string(&ledger).unwrap().lines().count(), 1);

        std::env::set_var(MODE_ENV, "OFF");
        assert!(observe(&ledger, admission("I patched src/foo.rs myself.")).is_none());
        std::env::remove_var(MODE_ENV);
        assert_eq!(std::fs::read_to_string(&ledger).unwrap().lines().count(), 1);
    }
}
