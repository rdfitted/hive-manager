---
name: resolvegitissue
description: Resolve a tracked repository issue within explicit ownership and validation gates.
---

# Resolve Git issue

## Workflow

1. Read the issue, acceptance criteria, current repository instructions, and project `.ai-docs/` conventions. Consult a configured wiki through its index if useful; `docs/wiki-starter/` is the portable starting layout.
2. Reproduce the failure or identify a concrete before-state. Locate callers, data writes, and existing tests. State any mismatch between the issue and current behavior.
3. Make the smallest scoped fix. Preserve compatibility and failure behavior unless the issue explicitly changes them. Keep changes within owned paths and ask the coordinator about a genuine scope conflict.
4. Add or update focused tests that verify the user-visible behavior. Run them, then the required integration checks. Record command, count, and failures; distinguish baseline failures from new ones.
5. Review the diff for unrelated edits and sensitive content. Report what changed, why, validation, risks, and the issue reference. Follow the repository's approval and git ownership rules before committing or publishing.

## Delegation

Use the installed [agent roster](../../agent-roster.md#invoking-a-slot) to assign bounded work. For a settings fix, fan out read-only explorer prompts over `src/` callers and `src-tauri/` storage contracts, then use a reviewer slot to check the proposed diff and focused test evidence. If Codex is absent, use native Claude Task/Agent children with identical paths, evidence, and stop conditions, or work sequentially. Delegate writes only when paths are disjoint and authorized. The parent integrates the findings and runs final validation.
