## Summary

Update model selector presets and defaults across all session modes (Solo, Hive, Swarm, Fusion) for Claude, Gemini, Codex, Cursor, and Droid CLIs. Currently only Claude and Codex have model preset dropdowns; Gemini, Cursor, and Droid lack model selectors entirely. Additionally, `cliOptions` arrays are duplicated across two frontend components with no shared constant.

**Exclusions:** Qwen and OpenCode model selectors should remain unchanged.

## Context

| Attribute | Value |
|-----------|-------|
| **Type** | Enhancement |
| **Scope** | Medium (~70 lines across 9+ files) |
| **Complexity** | Low-Medium (mostly data updates + minor UI logic) |
| **Priority** | Medium |

## Relevant Files

| File | Relevance | What to Change |
|------|-----------|---------------|
| `src/lib/components/AgentConfigEditor.svelte` (L16-37, L53-76, L290) | HIGH | Add gemini presets, extend `handleCliChange()` and preset conditional beyond claude/codex |
| `src/lib/components/AddWorkerDialog.svelte` (L31-39) | HIGH | Sync duplicate `cliOptions`; ideally extract to shared module |
| `src/lib/components/LaunchDialog.svelte` (L20-119, L128-168) | HIGH | Align `predefinedRoles` CLI defaults with backend `default_roles` |
| `src-tauri/src/storage/mod.rs` (L337-426) | HIGH | Update `default_model` strings in `default_config()` and `default_roles()` |
| `src-tauri/src/http/handlers/mod.rs` (L12) | MEDIUM | `VALID_CLIS` - no change needed unless CLIs added/removed |
| `src-tauri/src/cli/registry.rs` (L118-126, L160-323) | MEDIUM | Update test fixtures if model strings change |
| `src-tauri/src/session/controller.rs` (L409, L590-790) | MEDIUM | Audit hardcoded `"opus-4-6"` and model strings in `build_command()` |
| `src-tauri/src/http/tests.rs` | LOW | Update test fixtures with new model names |
| `src-tauri/src/pty/session.rs` | LOW | Verify no stale model literals |

## Analysis

### Current State
- **Claude**: Has preset dropdown with Opus 4.6 (high/low effort) and Sonnet 4.5
- **Codex**: Has preset dropdown with GPT-5.3 (low/medium/high/xhigh effort)
- **Gemini**: No preset dropdown despite having `model_flag: "-m"` - could support model selection
- **Cursor**: No preset dropdown; `model_flag: None` - uses global model setting
- **Droid**: No preset dropdown; `model_flag: None` - model selected via `/model` TUI command

### Key Issues
1. **Incomplete model presets**: Only claude/codex have `Model & Effort` dropdowns (conditional at line 290). Gemini supports `-m` flag but has no presets.
2. **Duplicated `cliOptions`**: Identical arrays in `AgentConfigEditor.svelte` and `AddWorkerDialog.svelte` - prone to drift.
3. **Three parallel CLI registries**: Frontend `cliOptions`, backend `VALID_CLIS`, and `storage/mod.rs` `default_config()` must stay manually in sync.
4. **Hardcoded role defaults**: All `predefinedRoles` in `LaunchDialog.svelte` default to `cli: 'claude'`, while backend `default_roles` maps frontend->gemini, coherence->droid, simplify->codex.
5. **Stale model strings**: Hardcoded model names scattered across frontend and backend may need updating.

### Gotchas
- **Cursor/Droid have no `model_flag`**: Adding presets for these CLIs would be cosmetic unless model passing mechanisms are added. Recommend hiding presets for CLIs that can't pass models programmatically.
- **Claude/Codex effort flags**: These presets set both model AND flags. A generic preset system must preserve this side-effect logic.
- **`stripManagedEffortFlags()`** (AgentConfigEditor line 56): Only handles claude/codex. If other CLIs get effort presets, this needs updating.

## Acceptance Criteria

- [ ] Gemini model presets added to `AgentConfigEditor.svelte` (at minimum: `gemini-2.5-pro`, `gemini-2.5-flash`)
- [ ] `handleCliChange()` updated to set default model when switching to gemini
- [ ] Preset dropdown conditional (line 290) extended to show for gemini (and any CLI with `model_flag`)
- [ ] `cliOptions` extracted to shared module (e.g., `$lib/config/clis.ts`) imported by both `AgentConfigEditor.svelte` and `AddWorkerDialog.svelte`
- [ ] `predefinedRoles` in `LaunchDialog.svelte` aligned with `default_roles` in `storage/mod.rs` (backend=claude, frontend=gemini, coherence=droid, simplify=codex)
- [ ] CLIs without `model_flag` (cursor, droid) either hide preset dropdown or show informational label
- [ ] Qwen and OpenCode selectors remain unchanged
- [ ] All three registries (frontend cliOptions, VALID_CLIS, storage default_config) remain in sync
- [ ] `cargo check --tests` passes after changes
- [ ] No regression in Solo/Hive/Swarm/Fusion launch flows

## Testing Requirements

- **Backend**: Extend `cli/registry.rs` tests to verify `build_command` output for gemini with custom models
- **Frontend**: Verify preset selection updates `AgentConfig.model` correctly for all CLIs
- **Manual**: Launch Solo session with Gemini using non-default model, verify `-m <model>` in spawned command

## Implementation Notes

**Recommended approach** (from multi-agent analysis):
1. Extract shared `cliOptions` to `$lib/config/clis.ts`
2. Add `geminiPresets` array alongside existing `claudePresets`/`codexPresets`
3. Refactor preset conditional to be data-driven (lookup from `cliPresets` map) instead of hardcoded if/else
4. Align `LaunchDialog.svelte` role defaults with backend
5. Audit `controller.rs` for hardcoded model strings
6. Validate with `cargo check --tests`

---

> Investigation: 10 agents (7 scouts + 3 planners) | Confidence: HIGH | Files analyzed: 12+
