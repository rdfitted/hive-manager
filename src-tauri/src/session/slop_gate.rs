//! Prompt-only UI gate contract. Detector policy and JSONL writes belong to the skill.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlopGateMode {
    Off,
    #[default]
    Report,
    Block,
}

pub(crate) const REPORT_COMMAND: &str = r#"python -I "<script>" <owned-ui-files> --root "<workspace>" --baseline "<baseline.json>" --json "<session-root>/evidence/slop-check/<task>-<run>.json" --log "<session-root>/state/slop-check.jsonl" --session "<session>" --task "<task>" --mode report"#;
pub(crate) const BLOCK_COMMAND: &str = r#"python -I "<script>" <owned-ui-files> --root "<workspace>" --baseline "<baseline.json>" --json "<session-root>/evidence/slop-check/<task>-<run>.json" --log "<session-root>/state/slop-check.jsonl" --session "<session>" --task "<task>" --mode block"#;

pub(crate) fn configured_mode(storage: Option<&crate::storage::SessionStorage>) -> SlopGateMode {
    match storage.map(|storage| storage.load_config()) {
        Some(Ok(config)) => config.slop_check_mode.unwrap_or_default(),
        Some(Err(error)) => {
            tracing::warn!("Cannot load slop-check mode; retaining report contract: {error}");
            SlopGateMode::Report
        }
        None => SlopGateMode::Report,
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn script_path(workspace: &Path) -> Option<PathBuf> {
    let mut roots = vec![workspace.join(".claude"), workspace.join(".agents")];
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from);
    if let Some(home) = home {
        roots.push(home.join(".claude"));
        roots.push(home.join(".codex"));
    }
    roots.into_iter().map(|root| root.join("skills/slop-check/scripts/slop_check.py"))
        .find(|path| path.is_file())
}

fn merge_base(workspace: &Path) -> Option<String> {
    let mut command = Command::new("git");
    command.args(["merge-base", "HEAD", "origin/main"]).current_dir(workspace);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(sha)
}

// Quote concrete values as shell literals. Tokens remain for completion-time inputs.
fn shell_value(value: &str) -> String {
    if value.contains(['"', '$', '`', '\n', '\r']) {
        let escaped = if cfg!(windows) {
            value.replace('\'', "''")
        } else {
            value.replace('\'', "'\"'\"'")
        };
        format!("'{escaped}'")
    } else {
        format!("\"{value}\"")
    }
}

pub(crate) fn render_command(
    mode: SlopGateMode, script: &str, workspace: &str, root: &str,
    session: &str, task: &str,
) -> String {
    let template = match mode {
        SlopGateMode::Off => return String::new(),
        SlopGateMode::Report => REPORT_COMMAND,
        SlopGateMode::Block => BLOCK_COMMAND,
    };
    template.replace("\"<script>\"", &shell_value(script))
        .replace("\"<workspace>\"", &shell_value(workspace))
        .replace("\"<session-root>/evidence/slop-check/<task>-<run>.json\"",
            &shell_value(&format!("{root}/evidence/slop-check/<task>-<run>.json")))
        .replace("\"<session-root>/state/slop-check.jsonl\"",
            &shell_value(&format!("{root}/state/slop-check.jsonl")))
        .replace("\"<session>\"", &shell_value(session))
        .replace("\"<task>\"", &shell_value(task))
}

pub(crate) fn render(
    mode: SlopGateMode, session_root: &Path, workspace: &Path,
    session_id: &str, plan_task_id: Option<&str>, reviewer: bool,
) -> String {
    if mode == SlopGateMode::Off {
        return String::new();
    }
    let script = script_path(workspace);
    let script_name = script.as_deref().map(display_path).unwrap_or_else(|| "<script>".into());
    let task = plan_task_id.unwrap_or("<task>");
    let command = render_command(mode, &script_name, &display_path(workspace),
        &display_path(session_root), session_id, task);
    let base = merge_base(workspace).map(|sha| format!("Session base SHA: `{sha}`."))
        .unwrap_or_else(|| "Resolve the session base before scanning: `git merge-base HEAD origin/main`. If it fails or returns no full SHA, record unavailable; never substitute HEAD.".into());
    let task_rule = if plan_task_id.is_some() {
        format!("The command carries the literal Plan Task ID {}. Replace the evidence filename's <task> with a filesystem-safe encoding of this ID; keep --task unchanged.", shell_value(task))
    } else {
        "No plan binding was supplied at spawn. Replace <task> with the exact JSON-decoded Plan Task ID from the authoritative task file; if absent, use the stable session worker task ID and document it.".into()
    };
    let mode_rule = match mode {
        SlopGateMode::Report => "Report mode: a block is logged and reported; the task may complete. An error is recorded as unavailable, logged, and noted by the reviewer; the task may complete.",
        SlopGateMode::Block => "Block mode: block means clean up, re-run, continue until pass. Do not ship around the gate or ask permission to fix owned findings. Error or unavailable requires escalation to the Queen.",
        SlopGateMode::Off => unreachable!(),
    };
    let review_rule = if reviewer {
        "Reviewer: a UI review without verdict JSON is incomplete, except explicit unavailable evidence in report mode. Verify the workstream's task/file ownership, base SHA and baseline extraction manifest, preexisting subtraction, and every ignored reason (not merely intentional). Banned rules cannot be ignored. Label each disputed finding with `python -I <script> label --log <absolute-log> --run <run_id> --finding <finding_key> --label fp --by reviewer`; cite its verdict path and mode. Review the assigned workstream even when your own diff is empty."
    } else {
        "Provide the verdict JSON and baseline extraction manifest to the reviewer. The reviewer verifies the base, ownership and specific ignore reasons, and records false-positive labels per disputed finding."
    };
    format!(r#"## UI slop-check gate

At completion, every coding principal checks (git diff --name-only <base> UNION git ls-files --others --exclude-standard) INTERSECT the task's owned paths. Use NUL-delimited output when collecting filenames. Gate changed .html/.tsx/.jsx/.vue/.svelte/.astro/.css/.scss files, including staged, unstaged and untracked files; do not scan a co-tenant's edits or rely on ...HEAD alone.
{base}
Extract each existing owned UI file with git show <base>:<relpath> into a temporary tree preserving repository-relative paths. New files have no baseline counterpart; deleted files have no current scan target. Preserve the same .slop-check.json policy in both roots. Scan baseline copies with `python -I "{script_name}" <baseline-ui-files> --root "<baseline-root>" --json "<baseline.json>"`. If there are no baseline counterparts, use a successful empty verdict v2 baseline with a manifest recording that fact. A failed baseline is unavailable, never an empty successful scan. The current --root is the workspace; baseline --root is the temporary tree, so relpath keys match. Save a base manifest containing the SHA, owned relpaths and extraction mapping next to the baseline verdict. Preexisting findings, including bans, are reported but do not gate.

Resolve the complete skill/script in order: project .claude/skills/slop-check/scripts/slop_check.py, project .agents/skills/slop-check/scripts/slop_check.py, user ~/.claude/skills/slop-check/scripts/slop_check.py, then user ~/.codex/skills/slop-check/scripts/slop_check.py. The explicit selected script is "{script_name}". Missing or incompatible script, missing engine, unsupported platform, scan error or baseline failure is unavailable. Never stub the engine. Use static scans for Tauri; add rendered scans only when the plan provides a dev-server URL and the project is not Tauri-only.

Final command:
{command}

{task_rule} Replace <owned-ui-files> with individually shell-quoted owned UI filenames, <baseline.json> with the absolute baseline verdict path, and <run> with a fresh unique filename token. Create evidence/log parent directories. Evidence and log paths are absolute from session_root_path, not a relative .hive-manager path in the worktree. Never commit evidence or logs; cite source findings by relpath.

{mode_rule}
Error or unavailable is never a pass. Mode metadata never changes the detector verdict. If the script can run, use its --log writer. If unavailable prevents it from running, write verdict_version 2 unavailable evidence with the failure reason and empty buckets; append one I2 run row in one write with row_version 1, type run, unique run_id, session_id, task_id, mode, verdict unavailable, zero bucket counts, empty rules/findings, engine_version null, verdict_path and UTC timestamp. A log write failure is an error; report it explicitly to the Queen and never claim a row was written. Report mode records it; block mode escalates. Jev shadow answers never alter this verdict or log row.

{review_rule}
Handoff names the verdict path, actual verdict, mode, base SHA, task id and absolute log path in the completion summary. Do not add heartbeat evidence fields. Report mode lasts for the two-week window starting at merge; only the operator changes config.json slop_check_mode to block after reviewing rates.
"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slop_gate_missing_config_field_resolves_report() {
        let config: crate::storage::AppConfig = serde_json::from_str(
            r#"{"clis":{},"default_roles":{}}"#).unwrap();
        assert_eq!(config.slop_check_mode.unwrap_or_default(), SlopGateMode::Report);
        for (name, mode) in [("off", SlopGateMode::Off), ("report", SlopGateMode::Report), ("block", SlopGateMode::Block)] {
            assert_eq!(serde_json::from_str::<SlopGateMode>(&format!("\"{name}\"")).unwrap(), mode);
        }
        assert!(serde_json::from_str::<SlopGateMode>("\"typo\"").is_err());
    }

    #[test]
    fn slop_gate_command_quotes_shell_values() {
        let command = render_command(SlopGateMode::Report, "script", "D:/project", "D:/root", "session", "T$(secret)");
        assert!(command.contains("--task 'T$(secret)'"));
        let command = render_command(SlopGateMode::Report, "script", "D:/project", "D:/root", "session", "T'O$(secret)");
        let quoted = if cfg!(windows) { "--task 'T''O$(secret)'" } else { "--task 'T'\"'\"'O$(secret)'" };
        assert!(command.contains(quoted));
    }

    #[test]
    fn slop_gate_script_resolution_prefers_complete_project_copy() {
        let temp = tempfile::tempdir().unwrap();
        let agents = temp.path().join(".agents/skills/slop-check/scripts/slop_check.py");
        let claude = temp.path().join(".claude/skills/slop-check/scripts/slop_check.py");
        std::fs::create_dir_all(agents.parent().unwrap()).unwrap();
        std::fs::write(&agents, "script").unwrap();
        assert_eq!(script_path(temp.path()), Some(agents));
        std::fs::create_dir_all(claude.parent().unwrap()).unwrap();
        std::fs::write(&claude, "script").unwrap();
        assert_eq!(script_path(temp.path()), Some(claude));
    }
}
