//! Prompt contract for the code maintainability gate. The checker owns the policy and the log.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::slop_gate::SlopGateMode;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaintainGateMode {
    Off,
    #[default]
    Report,
    Block,
}

/// The gate modes one spawned principal runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GateModes {
    pub slop: SlopGateMode,
    pub maintain: MaintainGateMode,
}

/// A bare UI mode leaves the maintainability gate off, so a caller that names
/// only the UI gate gets exactly the prompt it got before this gate existed.
impl From<SlopGateMode> for GateModes {
    fn from(slop: SlopGateMode) -> Self {
        Self { slop, maintain: MaintainGateMode::Off }
    }
}

impl GateModes {
    /// Research roles change no code, so they get no maintainability gate.
    pub(crate) fn maintain_unless(self, is_research: bool) -> MaintainGateMode {
        if is_research { MaintainGateMode::Off } else { self.maintain }
    }
}

pub(crate) fn configured_modes(storage: Option<&crate::storage::SessionStorage>) -> GateModes {
    let maintain = match storage.map(|storage| storage.load_config()) {
        Some(Ok(config)) => config.maintain_check_mode.unwrap_or_default(),
        Some(Err(error)) => {
            tracing::warn!("Cannot load maintain-check mode; retaining report contract: {error}");
            MaintainGateMode::Report
        }
        None => MaintainGateMode::Report,
    };
    GateModes { slop: super::slop_gate::configured_mode(storage), maintain }
}

/// Marker every compatible checker carries. A copy without it is skipped, so a
/// stale install earlier in the search order cannot shadow a working one.
const CONTRACT_MARKER: &str = "maintain-check: a delta gate";
const SCRIPT_RELATIVE: &str = "skills/maintain-check/scripts/maintain_check.py";

fn search_roots(workspace: &Path) -> Vec<PathBuf> {
    let mut roots = vec![workspace.join(".claude"), workspace.join(".agents")];
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    if let Some(home) = home {
        roots.push(home.join(".claude"));
        roots.push(home.join(".codex"));
    }
    roots
}

/// The first compatible checker for a workspace, in the documented search order.
pub(crate) fn resolve_checker(workspace: &Path) -> Option<PathBuf> {
    script_in(&search_roots(workspace))
}

fn script_in(roots: &[PathBuf]) -> Option<PathBuf> {
    roots.iter().map(|root| root.join(SCRIPT_RELATIVE)).find(|path| {
        std::fs::read_to_string(path).is_ok_and(|text| text.contains(CONTRACT_MARKER))
    })
}

fn quoted(value: &str) -> String {
    if value.contains(['"', '$', '`', '\\', '\n', '\r']) {
        let escaped = if cfg!(windows) { value.replace('\'', "''") } else { value.replace('\'', "'\"'\"'") };
        format!("'{escaped}'")
    } else {
        format!("\"{value}\"")
    }
}

fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn render_command(
    mode: MaintainGateMode, script: &str, workspace: &str, root: &str, session: &str, task: &str,
) -> String {
    let mode_name = match mode {
        MaintainGateMode::Off => return String::new(),
        MaintainGateMode::Report => "report",
        MaintainGateMode::Block => "block",
    };
    format!(
        "python -I {script} <owned-code-files> --root {workspace} --base \"<base-sha>\" --json {evidence} --log {log} --session {session} --task {task} --mode {mode_name}",
        script = quoted(script),
        workspace = quoted(workspace),
        evidence = quoted(&format!("{root}/evidence/maintain-check/<task>-<run>.json")),
        log = quoted(&format!("{root}/state/maintain-check.jsonl")),
        session = quoted(session),
        task = quoted(task),
    )
}

pub(crate) fn render(
    mode: MaintainGateMode, session_root: &Path, workspace: &Path,
    session_id: &str, plan_task_id: Option<&str>, reviewer: bool,
) -> String {
    render_with_roots(mode, session_root, workspace, session_id, plan_task_id, reviewer, &search_roots(workspace))
}

/// The contract as a section to append to a completion protocol: empty when the gate is off.
pub(crate) fn section(
    mode: MaintainGateMode, session_root: &Path, workspace: &Path,
    session_id: &str, plan_task_id: Option<&str>, reviewer: bool,
) -> String {
    let contract = render(mode, session_root, workspace, session_id, plan_task_id, reviewer);
    if contract.is_empty() { contract } else { format!("\n{contract}") }
}

fn render_with_roots(
    mode: MaintainGateMode, session_root: &Path, workspace: &Path,
    session_id: &str, plan_task_id: Option<&str>, reviewer: bool, roots: &[PathBuf],
) -> String {
    if mode == MaintainGateMode::Off {
        return String::new();
    }
    let script = script_in(roots);
    let script_name = script.as_deref().map(display).unwrap_or_else(|| "<script>".into());
    let task = plan_task_id.unwrap_or("<task>");
    let command = render_command(mode, &script_name, &display(workspace), &display(session_root), session_id, task);
    let script_rule = if script.is_some() {
        format!("The selected checker is \"{script_name}\".")
    } else {
        "No compatible checker was found at spawn. Look again in this order: project .claude, project .agents, user ~/.claude, user ~/.codex, each under skills/maintain-check/scripts/maintain_check.py. Skip a copy that rejects --base. If none runs, the gate is unavailable.".into()
    };
    let task_rule = if plan_task_id.is_some() {
        format!("The command carries the literal Plan Task ID {}. In the evidence filename replace <task> with a filesystem-safe encoding of it; keep --task unchanged.", quoted(task))
    } else {
        "No plan binding was supplied at spawn. Replace <task> with the exact Plan Task ID from the authoritative task file; if there is none, use the stable session worker task ID and say so.".into()
    };
    let mode_rule = match mode {
        MaintainGateMode::Report => "Report mode: a block verdict is logged and reported, and the task may complete. Error or unavailable is recorded and noted by the reviewer, and the task may complete.",
        MaintainGateMode::Block => "Block mode: a block verdict means clean up, re-run, and continue until the verdict is pass. Move new code into a new module or split the function. Do not ship around the gate and do not ask permission to fix findings in files you own. Error or unavailable requires escalation to the Queen. The app runs this checker itself on your workspace when you send the completed heartbeat. If that heartbeat is refused with HTTP 409, read <session-root>/evidence/maintain-check/<your-agent-id>-app.json, clean up, and send it again.",
        MaintainGateMode::Off => unreachable!(),
    };
    let review_rule = if reviewer {
        "Reviewer: a code review without the verdict JSON is incomplete, except explicit unavailable evidence in report mode. Check the base SHA, that the scanned files are the workstream's own, and that every ignore in .maintain-check.json names a specific reason. A new suppression added to pass any gate is itself a finding. Label a disputed finding with `python -I <script> label --log <absolute-log> --run <run_id> --finding <finding_key> --label fp --by reviewer`."
    } else {
        "Give the reviewer the verdict path. The reviewer checks the base, file ownership and ignore reasons, and labels disputed findings."
    };
    format!(r#"## Maintainability gate

At completion, every coding principal runs the maintainability checker on the code files it owns: (git diff --name-only <base> UNION git ls-files --others --exclude-standard) INTERSECT the task's owned paths. Use NUL-delimited output when collecting filenames. Do not scan a co-tenant's edits.
Resolve the session base with `git merge-base HEAD origin/main` and pass the full SHA as --base. If it fails or returns no full SHA, the gate is unavailable; never substitute HEAD. The checker reads the base version of each file itself, so no baseline scan is needed.
{script_rule}

Command:
{command}

{task_rule} Replace <owned-code-files> with individually shell-quoted owned filenames, <base-sha> with the session base, and <run> with a fresh unique filename token. Evidence and log paths are absolute under the session root, never a relative .hive-manager path in the worktree. Never commit evidence or logs.

The checker gates only what this change introduced: a file or function that is new over its budget, crosses it, or was already over and grew past the allowance; a new lint or type suppression. Findings that existed at the base are reported as preexisting and do not gate. Budgets and reasoned ignores live in .maintain-check.json at the repository root; never add an ignore or a suppression merely to pass.
{mode_rule}
Error or unavailable is never a pass. If the checker cannot run, write verdict_version 1 evidence with verdict unavailable, the reason and empty buckets, and append one run row with row_version 1, type run, tool maintain-check, a unique run_id, session_id, task_id, mode, verdict unavailable and a UTC timestamp. A log write failure is an error to report, not to hide.

{review_rule}
Handoff names the verdict path, the verdict, the score, the mode, the base SHA and the absolute log path in the completion summary. Only the operator changes config.json maintain_check_mode.
"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(root: &Path, body: &str) -> PathBuf {
        let path = root.join(SCRIPT_RELATIVE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn maintain_gate_missing_config_field_resolves_report() {
        let config: crate::storage::AppConfig =
            serde_json::from_str(r#"{"clis":{},"default_roles":{}}"#).unwrap();
        assert_eq!(config.maintain_check_mode.unwrap_or_default(), MaintainGateMode::Report);
        for (name, mode) in [("off", MaintainGateMode::Off), ("report", MaintainGateMode::Report), ("block", MaintainGateMode::Block)] {
            assert_eq!(serde_json::from_str::<MaintainGateMode>(&format!("\"{name}\"")).unwrap(), mode);
        }
        assert!(serde_json::from_str::<MaintainGateMode>("\"typo\"").is_err());
    }

    #[test]
    fn maintain_gate_bare_ui_mode_leaves_this_gate_off() {
        let modes = GateModes::from(SlopGateMode::Block);
        assert_eq!((modes.slop, modes.maintain), (SlopGateMode::Block, MaintainGateMode::Off));
    }

    #[test]
    fn maintain_gate_skips_an_incompatible_copy_earlier_in_the_order() {
        let temp = tempfile::tempdir().unwrap();
        let (first, second) = (temp.path().join("first"), temp.path().join("second"));
        checker(&first, "an older script without the contract");
        let good = checker(&second, "\"\"\"maintain-check: a delta gate for code maintainability.\"\"\"");
        assert_eq!(script_in(&[first.clone(), second.clone()]), Some(good.clone()));
        assert_eq!(script_in(&[first]), None);
        assert_eq!(script_in(&[second]), Some(good));
    }

    #[test]
    fn maintain_gate_command_carries_mode_absolute_paths_and_quoted_task() {
        let command = render_command(MaintainGateMode::Block, "script", "D:/project", "D:/root", "session", "T$(secret)");
        assert!(command.contains("--mode block"), "{command}");
        assert!(command.contains("--task 'T$(secret)'"), "{command}");
        assert!(command.contains("--json \"D:/root/evidence/maintain-check/<task>-<run>.json\""), "{command}");
        assert!(command.contains("--log \"D:/root/state/maintain-check.jsonl\""), "{command}");
        assert_eq!(render_command(MaintainGateMode::Off, "s", "w", "r", "x", "t"), "");
    }

    #[test]
    fn maintain_gate_contract_by_mode_and_role() {
        let temp = tempfile::tempdir().unwrap();
        let roots = [temp.path().join("install")];
        checker(&roots[0], CONTRACT_MARKER);
        let render = |mode, reviewer| render_with_roots(
            mode, &temp.path().join("root"), temp.path(), "session", Some("T7"), reviewer, &roots);
        assert_eq!(render(MaintainGateMode::Off, false), "");
        let report = render(MaintainGateMode::Report, false);
        assert!(report.starts_with("## Maintainability gate"));
        assert!(report.contains("Report mode:") && report.contains("--mode report"));
        assert!(report.contains("Plan Task ID \"T7\"") && !report.contains("<script>"));
        let block = render(MaintainGateMode::Block, true);
        assert!(block.contains("Block mode:") && block.contains("--mode block"));
        assert!(block.contains("Reviewer: a code review without the verdict JSON is incomplete"));
    }

    #[test]
    fn maintain_gate_without_a_checker_says_unavailable_not_pass() {
        let temp = tempfile::tempdir().unwrap();
        let prompt = render_with_roots(
            MaintainGateMode::Report, temp.path(), temp.path(), "session", None, false, &[temp.path().join("none")]);
        assert!(prompt.contains("No compatible checker was found at spawn"));
        assert!(prompt.contains("the gate is unavailable"));
        assert!(prompt.contains("No plan binding was supplied at spawn"));
    }
}
