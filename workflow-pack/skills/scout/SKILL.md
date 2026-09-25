---
name: scout
description: Locate relevant code, contracts, and tests before making a change.
---

# Scout

## Inputs

Take a concrete question or task, repository root, and any path or privacy restrictions. Read the project's `.ai-docs/project-dna.md` when present. If a shared wiki is configured, use its `index.md` to find relevant pages; `docs/wiki-starter/` shows the expected layout. Missing knowledge files are normal and do not block scouting.

## Workflow

1. Convert the request into two or three search questions. Start with filenames (`rg --files`) and symbols or phrases (`rg -n`). On systems without `rg`, use the shell's built-in file search and text search.
2. Read the smallest relevant call path and nearby tests. Follow callers and data writes far enough to identify the actual boundary.
3. Check existing conventions and explicit file ownership before suggesting edits. Do not cross a privacy barrier or broaden the assigned paths.
4. Return a short map: relevant files, entry points, behavior, tests, uncertainty, and any scope conflict. Give file paths and quoted symbols as evidence; do not claim a test was run unless it was.

## Delegation

For independent search questions, use the `explorer` slot and [invocation steps](../../agent-roster.md#invoking-a-slot). For a settings issue, fan out three read-only prompts over `src/` UI entry points, `src-tauri/` storage handlers, and related tests. Give each the same issue question, its assigned area, evidence format, and stop conditions. If Codex is absent, use native Claude Task/Agent children with the same prompts, or scout sequentially. The parent merges the three file maps, checks conflicting claims, and returns one bounded answer.
