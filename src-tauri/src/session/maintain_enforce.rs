//! The app runs the maintainability checker itself when an agent reports completion.
//!
//! The prompt contract in `maintain_gate` asks the agent to run the checker. This
//! module does not take the agent's word for it: it runs the same checker against
//! the agent's own workspace and decides from that verdict alone.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::maintain_gate::MaintainGateMode;

/// Longest the checker may run before the result is treated as unavailable.
const TIMEOUT: Duration = Duration::from_secs(90);
const MAX_FINDINGS_IN_MESSAGE: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    Pass { score: Option<u64> },
    Block { findings: Vec<String>, score: Option<u64> },
    /// The comparison could not be made. Never a pass.
    Unavailable { reason: String },
}

/// Who is declaring the agent complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sender {
    /// The agent, about itself.
    Agent,
    /// The Queen, marking another agent. This is the escalation path when the
    /// checker cannot run.
    QueenMark,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    Accept,
    Reject { message: String },
}

/// One completion to check.
#[derive(Debug, Clone)]
pub(crate) struct Check {
    pub workspace: PathBuf,
    pub session_root: PathBuf,
    pub session_id: String,
    pub agent_id: String,
    pub base: Option<String>,
    pub mode: MaintainGateMode,
}

impl Check {
    /// Where the app's own verdict for this agent is written. Distinct from any
    /// file the agent writes, so an agent cannot supply the evidence it is judged on.
    pub(crate) fn evidence_path(&self) -> PathBuf {
        let safe: String = self.agent_id.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.session_root.join("evidence").join("maintain-check").join(format!("{safe}-app.json"))
    }

    fn log_path(&self) -> PathBuf {
        self.session_root.join("state").join("maintain-check.jsonl")
    }
}

/// Whether a completion is accepted. Only block mode can refuse one.
pub(crate) fn decide(mode: MaintainGateMode, sender: Sender, outcome: &Outcome, evidence: &Path) -> Decision {
    if mode != MaintainGateMode::Block {
        return Decision::Accept;
    }
    let evidence = evidence.to_string_lossy().replace('\\', "/");
    match outcome {
        Outcome::Pass { .. } => Decision::Accept,
        Outcome::Block { findings, .. } => {
            let shown = findings.iter().take(MAX_FINDINGS_IN_MESSAGE).cloned().collect::<Vec<_>>().join("; ");
            let more = findings.len().saturating_sub(MAX_FINDINGS_IN_MESSAGE);
            let tail = if more > 0 { format!(" (and {more} more)") } else { String::new() };
            Decision::Reject { message: format!(
                "Maintainability gate blocked completion: {} finding(s) this change introduced. {shown}{tail}. Full verdict: {evidence}. Clean up, re-run the checker, then send the completed heartbeat again.",
                findings.len()) }
        }
        // The Queen may complete an agent whose check could not run; nobody may
        // complete one whose check ran and blocked.
        Outcome::Unavailable { .. } if sender == Sender::QueenMark => Decision::Accept,
        Outcome::Unavailable { reason } => Decision::Reject { message: format!(
            "Maintainability gate could not run, so this completion is not accepted: {reason}. Unavailable is never a pass. Report this to the Queen, who can mark you complete.") },
    }
}

/// Read a verdict file the checker wrote.
pub(crate) fn parse_verdict(text: &str) -> Outcome {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Outcome::Unavailable { reason: "the checker's verdict file is not valid JSON".into() };
    };
    let score = value.pointer("/score/value").and_then(|score| score.as_u64());
    match value.get("verdict").and_then(|verdict| verdict.as_str()) {
        Some("pass") => Outcome::Pass { score },
        Some("block") => {
            let findings = value.get("gating").and_then(|gating| gating.as_array()).into_iter().flatten()
                .map(|finding| {
                    let text = |key: &str| finding.get(key).and_then(|v| v.as_str()).unwrap_or("?");
                    let line = finding.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
                    format!("{}:{line} [{}] {}", text("relpath"), text("rule"), text("detail"))
                })
                .collect();
            Outcome::Block { findings, score }
        }
        other => {
            let errors = value.get("errors").and_then(|errors| errors.as_array()).into_iter().flatten()
                .filter_map(|error| error.as_str()).collect::<Vec<_>>().join("; ");
            let verdict = other.unwrap_or("missing");
            Outcome::Unavailable { reason: if errors.is_empty() { format!("checker verdict was {verdict}") } else { errors } }
        }
    }
}

/// Run the installed checker for one completion.
pub(crate) fn run(check: &Check) -> Outcome {
    match super::maintain_gate::resolve_checker(&check.workspace) {
        Some(script) => run_with(&script, check),
        None => Outcome::Unavailable { reason: "no compatible maintain-check script is installed".into() },
    }
}

/// Run a specific checker script. No window, no inherited standard streams.
pub(crate) fn run_with(script: &Path, check: &Check) -> Outcome {
    let evidence = check.evidence_path();
    // A verdict left by an earlier run must never be read as this run's answer.
    let _ = std::fs::remove_file(&evidence);
    let mode = if check.mode == MaintainGateMode::Block { "block" } else { "report" };
    let mut command = Command::new("python");
    command.arg("-I").arg(script).arg("--root").arg(&check.workspace)
        .arg("--json").arg(&evidence).arg("--log").arg(check.log_path())
        .arg("--session").arg(&check.session_id)
        .arg("--task").arg(format!("app:{}", check.agent_id))
        .arg("--mode").arg(mode).arg("--quiet")
        .current_dir(&check.workspace)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(base) = check.base.as_deref() {
        command.arg("--base").arg(base);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Outcome::Unavailable { reason: format!("could not start python: {error}") },
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() >= TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Outcome::Unavailable { reason: format!("the checker did not finish within {} seconds", TIMEOUT.as_secs()) };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return Outcome::Unavailable { reason: format!("could not wait for the checker: {error}") },
        }
    }
    match std::fs::read_to_string(&evidence) {
        Ok(text) => parse_verdict(&text),
        Err(_) => Outcome::Unavailable { reason: "the checker exited without writing a verdict".into() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(root: &Path, mode: MaintainGateMode) -> Check {
        Check {
            workspace: root.join("repo"), session_root: root.join("session"),
            session_id: "s1".into(), agent_id: "s1-worker-1".into(), base: None, mode,
        }
    }

    fn blocked() -> Outcome {
        Outcome::Block { findings: vec!["src/a.rs:1 [file-growth] 900 lines".into()], score: Some(50) }
    }

    fn unavailable() -> Outcome {
        Outcome::Unavailable { reason: "no checker".into() }
    }

    #[test]
    fn maintain_enforce_only_block_mode_can_refuse_a_completion() {
        let evidence = Path::new("D:/root/v.json");
        for mode in [MaintainGateMode::Off, MaintainGateMode::Report] {
            for outcome in [blocked(), unavailable(), Outcome::Pass { score: None }] {
                assert_eq!(decide(mode, Sender::Agent, &outcome, evidence), Decision::Accept, "{mode:?} {outcome:?}");
            }
        }
        assert_eq!(decide(MaintainGateMode::Block, Sender::Agent, &Outcome::Pass { score: Some(100) }, evidence), Decision::Accept);
    }

    #[test]
    fn maintain_enforce_a_block_verdict_refuses_the_agent_and_the_queen() {
        let evidence = Path::new("D:\\root\\v.json");
        for sender in [Sender::Agent, Sender::QueenMark] {
            let Decision::Reject { message } = decide(MaintainGateMode::Block, sender, &blocked(), evidence) else {
                panic!("a block verdict must be refused for {sender:?}");
            };
            assert!(message.contains("1 finding(s)") && message.contains("src/a.rs:1 [file-growth]"), "{message}");
            assert!(message.contains("D:/root/v.json"), "{message}");
        }
    }

    #[test]
    fn maintain_enforce_unavailable_refuses_the_agent_but_the_queen_can_escalate() {
        let evidence = Path::new("v.json");
        let Decision::Reject { message } = decide(MaintainGateMode::Block, Sender::Agent, &unavailable(), evidence) else {
            panic!("unavailable must never be accepted as a pass from the agent itself");
        };
        assert!(message.contains("never a pass") && message.contains("no checker"), "{message}");
        assert_eq!(decide(MaintainGateMode::Block, Sender::QueenMark, &unavailable(), evidence), Decision::Accept);
    }

    #[test]
    fn maintain_enforce_long_finding_lists_are_capped_in_the_message() {
        let findings: Vec<String> = (0..20).map(|i| format!("f{i}")).collect();
        let outcome = Outcome::Block { findings, score: None };
        let Decision::Reject { message } = decide(MaintainGateMode::Block, Sender::Agent, &outcome, Path::new("v.json")) else {
            panic!("expected a refusal");
        };
        assert!(message.contains("20 finding(s)") && message.contains("f7") && !message.contains("f8;"), "{message}");
        assert!(message.contains("(and 12 more)"), "{message}");
    }

    #[test]
    fn maintain_enforce_parses_each_verdict() {
        assert_eq!(parse_verdict(r#"{"verdict":"pass","score":{"value":100},"gating":[]}"#), Outcome::Pass { score: Some(100) });
        let block = parse_verdict(r#"{"verdict":"block","score":{"value":50},"gating":[{"relpath":"src/a.rs","line":7,"rule":"function-length","detail":"run: 140 lines (new)"}]}"#);
        assert_eq!(block, Outcome::Block { findings: vec!["src/a.rs:7 [function-length] run: 140 lines (new)".into()], score: Some(50) });
        assert_eq!(parse_verdict(r#"{"verdict":"unavailable","errors":["not inside a git repository"]}"#),
            Outcome::Unavailable { reason: "not inside a git repository".into() });
        assert_eq!(parse_verdict(r#"{"verdict":"error","errors":[]}"#), Outcome::Unavailable { reason: "checker verdict was error".into() });
        assert!(matches!(parse_verdict("{}"), Outcome::Unavailable { .. }));
        assert!(matches!(parse_verdict("not json"), Outcome::Unavailable { .. }));
    }

    #[test]
    fn maintain_enforce_evidence_is_the_apps_own_file_with_a_safe_name() {
        let mut check = check(Path::new("root"), MaintainGateMode::Block);
        check.agent_id = "s1/../worker 1".into();
        let path = check.evidence_path().to_string_lossy().replace('\\', "/");
        assert_eq!(path, "root/session/evidence/maintain-check/s1____worker_1-app.json");
    }

    #[test]
    fn maintain_enforce_a_missing_script_is_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("repo")).unwrap();
        let outcome = run_with(&temp.path().join("no-such-checker.py"), &check(temp.path(), MaintainGateMode::Block));
        assert!(matches!(outcome, Outcome::Unavailable { .. }), "{outcome:?}");
    }

    /// Runs the real checker from the workflow pack against a throwaway repository.
    /// Skipped where `python` or `git` is not on PATH.
    #[test]
    fn maintain_enforce_runs_the_real_checker_end_to_end() {
        let tool = |name: &str| Command::new(name).arg("--version").stdout(Stdio::null()).stderr(Stdio::null())
            .stdin(Stdio::null()).status().is_ok_and(|status| status.success());
        if !tool("python") || !tool("git") {
            eprintln!("skipping: python or git is not available");
            return;
        }
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../workflow-pack/skills/maintain-check/scripts/maintain_check.py");
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"])
                .args(args).current_dir(&repo).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(repo.join("small.py"), "value = 1\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "base"]);
        let mut check = check(temp.path(), MaintainGateMode::Block);
        check.base = Some("HEAD".into());

        std::fs::write(repo.join("small.py"), "value = 2\n").unwrap();
        assert_eq!(run_with(&script, &check), Outcome::Pass { score: Some(100) });

        let big: String = (0..900).map(|i| format!("x_{i} = {i}\n")).collect();
        std::fs::write(repo.join("big.py"), big).unwrap();
        let Outcome::Block { findings, score } = run_with(&script, &check) else { panic!("an oversized new file must block") };
        assert_eq!(score, Some(50));
        assert!(findings.len() == 1 && findings[0].starts_with("big.py:1 [file-growth]"), "{findings:?}");
        assert!(check.evidence_path().is_file() && check.log_path().is_file());
    }
}
