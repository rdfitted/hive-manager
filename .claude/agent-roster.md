# Agent roster

This is a portable starting point. Edit the CLI and model slots for the tools installed on your machine. A slot is guidance for a workflow, not permission to exceed the current task's file ownership or authority.

| Role slot | CLI | Model slot | Typical bounded work |
| --- | --- | --- | --- |
| planner | codex | `PLANNER_MODEL` | Turn requirements into an ordered plan |
| explorer | codex | `EXPLORER_MODEL` | Locate code paths and return evidence |
| implementer | codex | `IMPLEMENTER_MODEL` | Make one scoped change and validate it |
| reviewer | codex | `REVIEWER_MODEL` | Find concrete defects in a proposed diff |

Replace each model slot with a model identifier supported by your chosen CLI, or set the corresponding environment variable in your own shell. Do not assume the table's CLI is installed. A skill should resolve its roster from the same installed skills root: the repository uses `workflow-pack/agent-roster.md`; a user installation puts the roster beside the installed `skills/` directory. `CODEX_HOME` may change the Codex root.

Before delegation, state the objective, authoritative inputs, allowed paths, read/write mode, required evidence, and stop conditions. Writing agents need non-overlapping paths. Keep shared files, lockfiles, generated artifacts, and git operations with one owner. The parent reviews all results and runs integrated validation.

If the configured `codex` CLI is absent, use native Claude sub-agents for the same bounded roles. If native delegation is unavailable too, perform the roles sequentially in the current session. A fallback inherits the same ownership and stop conditions; it never widens the assignment. For another configured CLI, use its native delegation only when available and authorized.

## Invoking a slot

For the `explorer` slot, set `EXPLORER_MODEL` to a model supported by the CLI in the table. From the project root, check that the CLI exists and run a non-interactive, read-only task. Replace the example prompt with the assigned area's objective, authoritative inputs, allowed paths, evidence to return, and stop conditions. These commands use the same model slot on POSIX and PowerShell:

```sh
command -v codex >/dev/null || { echo 'codex unavailable'; exit 1; }
: "${EXPLORER_MODEL:?Set EXPLORER_MODEL}"
codex exec --sandbox read-only -m "$EXPLORER_MODEL" "Read-only: map the settings entry points and tests. Read repository instructions and project .ai-docs. Return paths and observed behavior; stop at restricted paths or scope conflicts."
```

```powershell
if (-not (Get-Command codex -ErrorAction SilentlyContinue)) { throw 'codex unavailable' }
if (-not $env:EXPLORER_MODEL) { throw 'Set EXPLORER_MODEL' }
codex exec --sandbox read-only -m $env:EXPLORER_MODEL 'Read-only: map the settings entry points and tests. Read repository instructions and project .ai-docs. Return paths and observed behavior; stop at restricted paths or scope conflicts.'
```

Use the matching model variable for another slot. If a slot names another CLI, use that CLI's documented non-interactive command and keep the same bounded task; do not reuse Codex flags blindly. If Codex is absent, create a native Claude Task/Agent child with role `explorer` and the same read-only prompt, inputs, evidence, and stop conditions. Launch independent children only when the current task authorizes delegation. Wait for all results; the parent checks citations, resolves conflicts, and merges the answer or changes.
