---
name: init-project-dna
description: Initialize a project's local knowledge files from verified repository evidence.
---

# Initialize project knowledge

Use this skill when asked to create a project's `.ai-docs/` layer or when a substantial project task needs that layer and the user accepts initialization. Run from the project root; `git rev-parse --show-toplevel` can locate it on any supported shell. If the directory is not a Git repository, use the project root supplied by the user. Never use a shared wiki as a substitute for project knowledge.

## Inspect before writing

1. Read applicable repository instructions, the README, configuration, representative code, and relevant tests. Read existing `.ai-docs/` files if present.
2. Write a brief list of conventions and failure modes with file or test references. Distinguish observed behavior from assumptions. Keep unrelated projects out of this layer.
3. Identify which files are missing. Preserve existing files and merge only requested corrections; initialization is not permission to replace prior knowledge.

## Create the project layout

Use your host's file editing API or Node's built-in `node:fs` module so the procedure works on POSIX and Windows. Create `.ai-docs/archive/` and only the missing files below:

```text
.ai-docs/
  project-dna.md
  bug-patterns.md
  learnings.jsonl
  curation-state.json
  archive/
```

- `project-dna.md`: start with `# Project DNA`, then concise sections for architecture, conventions, validation, and open questions. Put only repository-backed facts in the first three sections and cite their paths.
- `bug-patterns.md`: start with `# Bug Patterns` and an empty section for verified recurring failures. Do not turn a single unexplained failure into a pattern.
- `learnings.jsonl`: create an empty file. It is an append-only stream of one valid JSON object per line when the project uses direct project learning capture.
- `curation-state.json`: initialize as `{"last_curated_line":0}`. The number counts physical lines already reviewed by curation.
- `archive/`: create an empty directory for material intentionally retired during later curation.

If a managed Hive task supplies a session learning API, use that task's API for new learnings. Do not append those records directly to project `.ai-docs/learnings.jsonl`.

## Check and report

Confirm each created file is in the selected project, JSON parses, the curation cursor is zero for a new log, and cited source paths exist. Report files created, source evidence, assumptions left open, and any pre-existing file preserved. Do not initialize a home-level project layer or copy shared wiki pages into the project.
