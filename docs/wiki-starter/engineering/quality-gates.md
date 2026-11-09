---
title: Quality gates
category: engineering
last_updated: 2026-09-25
---

# Quality gates

Purpose: list the checks required before a change is considered ready.

## Fill in

- Check name and command: UI slop-check. The principal uses the explicit script selected from project `.claude/skills/slop-check/scripts/slop_check.py`, then project `.agents/skills/slop-check/scripts/slop_check.py`, then user `~/.claude/skills/slop-check/scripts/slop_check.py` or `~/.codex/skills/slop-check/scripts/slop_check.py`. These command templates match the generated prompt; each filename is individually shell-quoted. Replace tokens with the assigned task's values.

Report command:

```text
python -I "<script>" <owned-ui-files> --root "<workspace>" --baseline "<baseline.json>" --json "<session-root>/evidence/slop-check/<task>-<run>.json" --log "<session-root>/state/slop-check.jsonl" --session "<session>" --task "<task>" --mode report
```

Block command:

```text
python -I "<script>" <owned-ui-files> --root "<workspace>" --baseline "<baseline.json>" --json "<session-root>/evidence/slop-check/<task>-<run>.json" --log "<session-root>/state/slop-check.jsonl" --session "<session>" --task "<task>" --mode block
```

- Scope and expected result: at completion, collect `git diff --name-only <base>` union `git ls-files --others --exclude-standard`, intersect the task's owned paths, and select `.html/.tsx/.jsx/.vue/.svelte/.astro/.css/.scss`. Use NUL-delimited output to collect filenames. This includes staged, unstaged and untracked files. Resolve `git merge-base HEAD origin/main` at spawn and use the rendered literal SHA; run that command as a fallback when resolution fails. Extract existing changed files with `git show <base>:<relpath>` into a temporary tree that preserves relative paths. Scan those copies with the temporary tree as `--root`, preserving the same `.slop-check.json` policy; use its verdict as `--baseline` for the workspace scan. Save a base-SHA/extraction manifest beside it. New files have no baseline counterpart; deleted files have no current target. Findings already present at the base, including banned ones, remain reported as `preexisting` and do not gate. A failed baseline is unavailable, never an empty successful scan. Use static scans for Tauri; rendered scans are supplementary only when the plan names a dev-server URL and the app is not Tauri-only.
- Owner and execution point: every coding principal runs the enabled gate before handoff; the reviewer verifies the assigned UI workstream's verdict JSON, ownership, base manifest and specific ignore reasons, even when the reviewer's own diff is empty. `AppConfig.slop_check_mode` in `config.json` accepts `off`, `report` or `block`; a missing field means `report`, effective at the next spawn. `off` omits the gate and is not a pass verdict. The two-week report-only window starts at merge; the operator reviews the rates and explicitly switches to `block`. Report mode logs and reports a block while allowing completion. Block mode requires clean up, re-run, continue until pass. Jev shadow answers never change the verdict or log.
- Evidence location and exceptions: `<session-root>` is the absolute canonical path from `session_root_path`, never a worktree-relative `.hive-manager` path. Keep uncommitted verdicts at `<session-root>/evidence/slop-check/<task>-<run>.json` and run rows at `<session-root>/state/slop-check.jsonl`. The command carries the literal Plan Task ID when supplied at spawn; otherwise substitute the exact ID from the authoritative task file, using a documented stable worker task ID only when no plan binding exists. Encode the task ID safely for filenames and use a fresh run token. Completion evidence names the verdict path, actual verdict, mode, base SHA, task ID and log path; cite source findings by `relpath`. Reasoned `.slop-check.json` ignores must be specific, and banned rules cannot be ignored. The reviewer appends false-positive labels with the skill's `label --log ... --run ... --finding ... --label fp --by reviewer` command. Run the skill's `rates --log ... --since <merge-date>` after the report window. Missing/incompatible skill, missing engine, unsupported platform, scan error or baseline failure is `unavailable`, never pass: report mode records unavailable evidence and a run row and permits completion; block mode escalates to the Queen. If the script cannot run, append an explicit unavailable I2 row in one write. A log write failure is an error and must be reported; never claim evidence was written when it was not.

- Check name and command: code maintainability. The principal uses the first compatible checker found at project `.claude/skills/maintain-check/scripts/maintain_check.py`, then project `.agents/skills/maintain-check/scripts/maintain_check.py`, then user `~/.claude/skills/maintain-check/scripts/maintain_check.py` or `~/.codex/skills/maintain-check/scripts/maintain_check.py`. A copy that does not carry the checker's contract is skipped, so a stale install cannot shadow a working one.

Command, with `--mode report` or `--mode block`:

```text
python -I "<script>" <owned-code-files> --root "<workspace>" --base "<base-sha>" --json "<session-root>/evidence/maintain-check/<task>-<run>.json" --log "<session-root>/state/maintain-check.jsonl" --session "<session>" --task "<task>" --mode report
```

- Scope and expected result: at completion, collect `git diff --name-only <base>` union `git ls-files --others --exclude-standard`, intersect the task's owned paths, and pass those files. The base is `git merge-base HEAD origin/main`, resolved to a full SHA. The checker reads each file's base version itself and gates only what the change introduced: a file or function that is new over its budget, crosses it, or was already over and grew past the allowance, and any new lint or type suppression. Findings present at the base are reported as `preexisting` and do not gate. Test files, fixtures, vendored directories and inline Rust test modules are out of scope. The verdict carries a 0-100 score: the percent of checked files with no new gating finding.
- Owner and execution point: every coding principal runs the enabled gate before handoff, and the reviewer verifies the verdict JSON, the base SHA, file ownership and every ignore reason. `AppConfig.maintain_check_mode` in `config.json` accepts `off`, `report` or `block`; a missing field means `report`, effective at the next spawn. `off` omits the gate and is not a pass verdict. Report mode logs and reports a block while allowing completion. Block mode requires clean up, re-run, continue until pass. Only the operator changes the mode, after reviewing the rates.
- Evidence location and exceptions: verdicts are kept at `<session-root>/evidence/maintain-check/<task>-<run>.json` and run rows at `<session-root>/state/maintain-check.jsonl`, both absolute and never committed. Budgets and reasoned ignores live in `.maintain-check.json` at the repository root; an ignore without a specific reason is an error, and a suppression added to pass any gate is itself a finding. The reviewer labels a disputed finding with the checker's `label --log ... --run ... --finding ... --label fp --by reviewer` command, and `rates --log ...` reports hits, the labelled false-positive rate per rule, and runs by verdict. A missing or incompatible checker, an unresolvable base or a scan error is `unavailable`, never pass: report mode records it and permits completion; block mode escalates to the Queen.
