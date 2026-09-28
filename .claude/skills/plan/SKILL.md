---
name: plan
description: Produce an implementation plan grounded in repository evidence.
---

# Plan

## Inputs

Collect the intended outcome, acceptance criteria, constraints, owned paths, and any approved design. Read relevant project `.ai-docs/` conventions. A configured shared wiki may add context through its `index.md`; the layout in `docs/wiki-starter/` is a template, not a required dependency.

## Workflow

1. Scout the affected entry points, data flow, and existing tests. Keep factual findings separate from assumptions.
2. State the outcome and observable acceptance checks. Call out compatibility, failure, and persistence behavior where relevant.
3. Break work into small tasks with dependencies, explicit file ownership, and a validation gate for each meaningful risk. Put shared-file edits in one task or sequence them.
4. Identify decisions that require the operator. Continue planning independent work while those decisions are pending.
5. Return the plan with scope, tasks, tests, risks, and a clear stopping point. Do not implement merely because a plan was requested.

## Delegation

The installed [agent roster](../../agent-roster.md#invoking-a-slot) supplies `explorer`, `planner`, and `reviewer` CLI/model slots and a concrete invocation example. For a settings change, fan out two read-only explorer prompts: one maps UI state and tests under `src/`, and one maps persistence and tests under `src-tauri/`. Ask each for paths, observed contracts, and uncertainty. If Codex is absent, use native Claude Task/Agent children with those same bounded prompts, or work sequentially if delegation is unavailable. The parent reconciles both maps and writes one plan with integrated validation.
