---
name: curate-learnings
description: Review raw project learnings and promote verified, durable guidance into project knowledge.
---

# Curate project learnings

Use this skill when asked to review `.ai-docs/learnings.jsonl` or refresh project knowledge from captured lessons. Work in the selected project root. In a managed Hive session, follow the task's session learning API and curation instructions instead of editing a project log directly.

## Read the bounded input

1. Read `.ai-docs/project-dna.md`, `.ai-docs/bug-patterns.md`, and `.ai-docs/curation-state.json`. If the layer does not exist, use `init-project-dna` only when initialization is authorized.
2. Parse `.ai-docs/learnings.jsonl` line by line as JSON. `last_curated_line` is a count of physical lines already reviewed, starting at zero. Inspect only later lines and a small amount of earlier context needed to detect duplicates. Stop on malformed JSON or a cursor beyond the file; report the line without advancing the cursor.
3. For each new record, locate the stated files, tests, or other evidence. Check whether the claim is still true. Do not promote a record based only on its wording or a successful command exit.

## Decide where each record belongs

- Promote a durable project convention, architecture decision, or repeatable successful approach to `project-dna.md` with a short explanation and current source references.
- Promote a recurring, verified failure mode to `bug-patterns.md` with its symptom, cause, fix or workaround, and a check that detects it.
- Mark a duplicate, routine activity, unsupported claim, or superseded fact as reviewed without adding it to curated guidance. Keep the raw log append-only.
- Leave a disputed claim unresolved in the report and do not turn it into policy.

Keep the curated text concise. Update an existing entry when it is the same fact; remove contradictory stale wording only after checking the source. Do not move project-specific knowledge into a shared wiki during this workflow.

## Save safely

Use the host file editing API or Node's built-in `node:fs` module on either POSIX or Windows. Write the updated curated pages first, re-read them, then set `last_curated_line` to the final physical line reviewed. Preserve any other fields already in `curation-state.json`. If a write fails, leave the cursor unchanged so a later run can retry. Archive retired curated text only when its history is useful, and explain why it was retired.

Report the reviewed line interval, records promoted or deferred, paths changed, evidence checked, and the final cursor. If nothing qualifies, say so and still advance the cursor across valid reviewed records.
