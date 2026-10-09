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
