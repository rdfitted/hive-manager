# Task Assignment - Worker 5

## Status: COMPLETED

## Role Constraints

- **EXECUTOR**: You have full authority to implement and fix issues.
- **SCOPE**: Stay within your assigned domain/specialization.
- **GIT**: Do NOT push or commit. Provide your changes for the Queen to integrate.

## Instructions

Read `.hive-manager/70525b48-506a-434e-b7ef-3d4010f6609e/plan.md` — execute Task 5 (HIGH). Base branch: feat/worker-scope-kanban-opus47 (W1-W4 all integrated).

Review ALL diffs from Workers 1-4 on feat/worker-scope-kanban-opus47 (commits b26e621, b6b6cb5, 62206ff, 79c3eaf).

Checklist:
1. Correctness of worktree_boundary_rules helper + wiring into build_fusion_worker_prompt, build_worker_prompt, write_task_file(_with_status).
2. IDENTICAL wording across the three boundary surfaces (fusion worker / regular worker / task file Scope block). Diff them.
3. IDENTICAL Required Protocol block across build_queen_master_prompt, build_fusion_queen_prompt, build_swarm_queen_prompt, and templates/mod.rs evaluator template.
4. Opus 4.7 rewrites ACTUALLY convert soft -> hard language. Spot-check every should/consider/try/might remaining in evaluator + queen prompts; they should be MUST/MUST NOT or justified.
5. Post-Workers Protocol (7-step) present and identical in all three Queen variants. Hard rule about NOT spawning Evaluator is verbatim. /loop is explicitly disallowed.
6. No change to default model strings (opus-4-6 literals in controller.rs:673,717,750 unchanged).
7. KanbanCard.svelte hover preserves left accent (border-top/right/bottom-color only).
8. No regression in existing evaluator/worker/queen tests.
9. Run cargo check --tests (NOT cargo test). Run pnpm run check (or frontend equivalent). Report failures.

Produce review report at .hive-manager/70525b48-506a-434e-b7ef-3d4010f6609e/review-notes.md — list every finding with file:line + severity (BLOCKER/MAJOR/MINOR/NIT). If clean, state that explicitly.

WORK IN YOUR WORKTREE. Mark task COMPLETED when done.

## Completion Protocol

When task is complete, update this file:
1. Change Status to: COMPLETED
2. Add a summary under a new Result section

If blocked, change Status to: BLOCKED and describe the issue.

## Result

Review completed. Wrote findings to `.hive-manager/70525b48-506a-434e-b7ef-3d4010f6609e/review-notes.md`.

Key findings:
- MAJOR: the live evaluator template does not share the same `## Required Protocol` block as the three queen builders.
- MINOR: queen master prompt still contains soft `try` wording.
- MINOR: new prompt coherence tests only assert substring presence, so the protocol drift passes coverage.

Verification notes:
- `cargo check --tests` failed because `link.exe` resolves to `C:\Program Files\Git\usr\bin\link.exe`.
- `pnpm run check` failed because `node_modules` is missing and `svelte-kit` is unavailable.
- `git diff --check f2b766c..79c3eaf` passed.
- Learnings submission could not be completed because `localhost:18800` was unavailable.

---
Last updated: 2026-04-22T17:27:19Z
