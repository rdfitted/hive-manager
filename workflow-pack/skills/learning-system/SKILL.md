---
name: learning-system
description: Load relevant knowledge, capture durable lessons, and route curation between project and shared layers.
---

# Learning system

Use this skill to choose the right knowledge layer before work and to review lessons after a meaningful change. Project `.ai-docs/` belongs to one repository. The optional shared wiki holds guidance that has been checked for wider use. Do not use one layer as a fallback write location for the other.

## Before work

1. Resolve the project root and read its applicable instructions. Read `.ai-docs/project-dna.md` and `.ai-docs/bug-patterns.md` when present.
2. Search `.ai-docs/learnings.jsonl` by task terms and inspect a small recent tail; do not load a large raw log without a reason. Treat raw records as leads to verify, not established rules.
3. When a shared wiki is configured, use its `index.md` to choose only relevant pages. Check their `last_updated` dates and verify stale or consequential claims.
4. Keep a short working brief with source paths, current conventions, known failure modes, checks, and unresolved conflicts. Current code and runtime results take priority over outdated notes.

If the project layer is absent, report that fact. Use `init-project-dna` when initialization is requested or accepted; never create a home-level project layer as a substitute.

## After work

Review what validation actually showed. Capture a lesson only when it is a durable, non-obvious pattern, failure mode, workaround, architecture decision, or test lesson. File changes, routine progress, and a command exiting zero alone are not lessons. Include source paths or other reproducible evidence and note uncertainty.

For a direct project log, append one JSON object per line to `.ai-docs/learnings.jsonl` using the host file API or Node's `node:fs` and `JSON.stringify`; preserve the existing project schema. When no stricter schema exists, use `date`, `session`, `task`, `outcome`, `keywords`, `insight`, and `files_touched`. Set a new, uncurated record's `outcome` to `unreviewed`. Re-read the appended line and parse it as JSON before reporting success.

For a managed Hive task, submit the record through the task's session learning API with `session`, `task`, `outcome`, `keywords`, `insight`, and `files_touched`. Follow its documented fallback if the API is unavailable. Do not append the same record to project `.ai-docs/learnings.jsonl`.

## Curate and share

Run `curate-learnings` to review new project records, validate their evidence, and promote only durable project guidance to `project-dna.md` or `bug-patterns.md`. A lesson that may apply across projects is a candidate for a separate `wiki` review; check its generality and remove project-specific details before adding it to the shared index. Report records captured, curated, deferred, or rejected with their evidence and remaining uncertainty.
