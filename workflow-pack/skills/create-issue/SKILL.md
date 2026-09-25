---
name: create-issue
description: Investigate a repository problem and draft a source-backed issue.
---

# Create issue

## Inputs and investigation

Take the reported symptom, repository and issue tracker, and any owner constraints. Search for the relevant code path and existing issues. Read project `.ai-docs/` conventions; use a configured wiki index only for relevant shared context. `docs/wiki-starter/` shows the wiki structure when starting fresh.

Reproduce the problem when practical. Record exact commands, observed results, expected results, environment, and affected files. If reproduction is unavailable, say so and distinguish the user's report from verified findings. Do not put secrets or private data into an issue or fixture.

## Issue draft

Write a concise title and sections for problem, evidence, expected behavior, likely cause or open question, proposed scope, acceptance criteria, and validation. Link related issues and identify decisions or dependencies. Prefer an acceptance test that fails before the fix and passes after it. Ask for approval before publishing when the task only authorized a draft.

## Delegation

The installed [agent roster](../../agent-roster.md#invoking-a-slot) defines `explorer` and `reviewer` slots. For a reported settings failure, fan out read-only explorer prompts over `src/` UI behavior and `src-tauri/` persistence, then ask a reviewer to inspect the evidence and draft. If Codex is absent, use native Claude Task/Agent children with the same bounded prompts, or work sequentially. The parent merges the findings, verifies every claim against the repository, and owns the final issue and publication.
