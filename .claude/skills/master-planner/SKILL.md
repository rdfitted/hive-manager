---
name: master-planner
description: Coordinate a substantial implementation plan with clear dependencies and gates.
---

# Master planner

Use this for work with several independent areas or a release-level integration gate. For a small change, use the regular `plan` skill.

## Workflow

1. Establish the authoritative request, current base, scope, non-goals, ownership, and decisions already made. Read relevant project `.ai-docs/` guidance and the configured wiki index when available; the `docs/wiki-starter/` tree is a portable example.
2. Scout each affected subsystem. Record verified entry points and uncertain assumptions separately. Avoid reopening questions already settled by an authoritative handoff.
3. Define acceptance criteria as observable behavior and name the command or review that proves each one.
4. Build a dependency graph with narrow tasks, explicit output paths, and one owner per shared file. Sequence migrations, generated files, package metadata, and integration edits.
5. Include baseline measurements, focused checks, full integration gates, and recovery steps for plausible failure modes. A green gate must exercise the behavior it claims to prove.
6. Review the plan for missing dependencies and conflicting ownership, then publish the final task order and unresolved operator decisions.

## Delegation

Use the installed [agent roster](../../agent-roster.md#invoking-a-slot) for `explorer`, `planner`, and `reviewer` slots. For a release plan, fan out read-only explorer prompts over `.github/workflows/` CI, `scripts/` packaging, and `src/` update UI. Each returns dependencies, existing gates, and risks for its assigned area. If Codex is absent, use native Claude Task/Agent children with identical bounded prompts, or work sequentially. The parent merges the three dependency maps into one task graph and validates cross-area gates. Do not delegate shared-file writes or authority decisions.
