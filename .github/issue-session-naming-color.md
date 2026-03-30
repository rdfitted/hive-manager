## Summary

Add the ability for users to **name** and **color code** sessions in the sessions sidebar, with the chosen color displayed as an accent on the **terminal header top bar** of the active session. This improves multi-session workflows where sessions targeting the same project are currently indistinguishable (identified only by folder name).

## Context

| Field | Value |
|-------|-------|
| **Type** | Feature |
| **Scope** | Small-Medium (full-stack, 4 layers) |
| **Complexity** | Low-Medium (UI is the most complex part) |
| **Priority** | Medium |
| **Estimated effort** | 3-5 hours, or 2 Hive workers in parallel |

### Problem

Sessions are currently identified in the sidebar by `project_path.split(/[/\\]/).pop()` (SessionSidebar.svelte:223). When multiple sessions target the same project (common for A/B testing, separate feature branches, or different tasks), every sidebar entry shows an identical folder name differentiated only by timestamp. This makes it cognitively expensive to locate the right session.

## Relevant Files

| File | Lines | Relevance | What Changes |
|------|-------|-----------|-------------|
| `src-tauri/src/session/controller.rs` | 209-221 | HIGH | Add `name: Option<String>`, `color: Option<String>` to `Session` struct |
| `src-tauri/src/storage/mod.rs` | 74-100 | HIGH | Add fields to `PersistedSession` (line 85) and `SessionSummary` (line 74) with `#[serde(default)]` |
| `src-tauri/src/http/handlers/sessions.rs` | 15-160 | HIGH | Add fields to `SessionInfo` (line 16), new `PATCH /api/sessions/{id}` handler |
| `src-tauri/src/http/routes.rs` | — | HIGH | Register new PATCH route |
| `src-tauri/src/commands/session_commands.rs` | — | MEDIUM | Add `update_session_metadata` Tauri command |
| `src/lib/stores/sessions.ts` | 120-131 | HIGH | Add `name?: string`, `color?: string` to `Session` interface, add `updateSessionMetadata()` method |
| `src/lib/components/SessionSidebar.svelte` | 219-276 | HIGH | Display session name (fallback to folder), color dot/border, inline edit + color picker |
| `src/routes/+page.svelte` | 247-262, 481 | HIGH | Apply session color as accent on `.terminal-header` |
| `src/lib/components/TerminalGrid.svelte` | — | HIGH | Same color accent on grid-mode headers |
| `src/lib/components/StatusPanel.svelte` | — | MEDIUM | Display session name and color in info section |
| `src/lib/components/LaunchDialog.svelte` | — | MEDIUM | Optional name/color fields at session creation |
| `src/lib/components/AgentStatusBar.svelte` | 26-32 | LOW | Reference pattern for `getStatusColor()` and existing color palette |

## Analysis

### Architecture & Data Flow

1. **Rust model layer**: Add `name: Option<String>` and `color: Option<String>` to `Session`, `PersistedSession`, `SessionSummary`, and `SessionInfo` structs. Use `#[serde(default)]` for backward compatibility with existing persisted sessions.

2. **HTTP API layer**: Add `PATCH /api/sessions/{id}` endpoint accepting `{ name?: string, color?: string }`. Update the in-memory session, persist via `save_session()`, and emit `session-update` event. Also add optional `name`/`color` to all launch request structs.

3. **TypeScript store layer**: Extend `Session` interface, add `updateSessionMetadata(id, name, color)` method that invokes the Tauri command or HTTP endpoint.

4. **UI layer**:
   - **Sidebar**: Show custom name as primary label (folder name as secondary). Color dot or left-border accent per session item. Inline rename (click-to-edit or pencil icon) + color palette picker.
   - **Terminal header**: Apply session color as `border-top: 3px solid {color}` or background tint on `.terminal-header`.
   - **Grid mode**: Same treatment on TerminalGrid headers.

### Event Propagation

The existing `session-update` Tauri event (sessions.ts:149) already serializes the full `Session` struct. Adding fields to the struct will automatically flow through to the frontend — no new event plumbing needed.

### Existing Patterns to Follow

- `AgentConfig.label: Option<String>` — precedent for optional user-defined labels
- `#[serde(default)]` on `PersistedSession` fields (storage/mod.rs:92-99) — backward compat pattern
- `getStatusColor()` in AgentStatusBar.svelte — existing dynamic color pattern
- `validate_session_id()` in handlers/mod.rs — input validation pattern to reuse

### Suggested Color Palette (Tokyo Night aligned)

| Color | Hex | Name |
|-------|-----|------|
| 🔵 | `#7aa2f7` | Blue |
| 🟣 | `#bb9af7` | Purple |
| 🟢 | `#9ece6a` | Green |
| 🟡 | `#e0af68` | Yellow |
| 🔷 | `#7dcfff` | Cyan |
| 🔴 | `#f7768e` | Red |
| 🟠 | `#ff9e64` | Orange |
| 🩷 | `#f7b1d1` | Pink |

### Security Considerations

- **Name validation**: Max 64 characters, reject path traversal chars (`..`, `/`, `\`, null bytes). Follow existing `validate_session_id()` pattern.
- **Color validation**: Use a **predefined palette allowlist** (recommended) to eliminate CSS injection risk. If freeform hex is needed, validate `^#[0-9a-fA-F]{6}$` server-side and use CSS classes rather than raw inline styles.
- **XSS**: Svelte's `{variable}` syntax auto-escapes HTML. Keep names out of `{@html}` blocks.

### UX Best Practices (from web research)

- **Color as secondary cue** — pair with labels/icons for accessibility (colorblind users)
- **Subtle indicators** — 2-4px colored border or small dot, not full-background coloring
- **60-30-10 rule** — neutral 60%, secondary 30%, accent 10%
- **Auto-name + click-to-edit** — generate defaults, allow inline editing
- **Contrast ratio ≥ 4.5:1** for any text on colored backgrounds

## Acceptance Criteria

- [ ] **AC-1: Schema extension** — `Session`, `PersistedSession`, `SessionSummary`, `SessionInfo` (Rust) and `Session` (TypeScript) include optional `name` (string, max 64 chars) and `color` (string, from predefined palette) fields
- [ ] **AC-2: Backward compatibility** — Existing persisted sessions without `name`/`color` load without error, displaying with current default behavior
- [ ] **AC-3: Launch with name/color** — All launch request types accept optional `name` and `color` fields. LaunchDialog UI exposes these as optional inputs
- [ ] **AC-4: PATCH endpoint** — `PATCH /api/sessions/{id}` accepts `{ name?: string, color?: string }`, updates in-memory session, persists, and emits `session-update` event
- [ ] **AC-5: Sidebar display** — Custom name appears as primary label; folder name as secondary. Color dot or left-border accent on sidebar items. Falls back to folder name when no custom name set
- [ ] **AC-6: Top bar color** — Terminal header displays active session's color as a visible accent (border-top or background tint). Default styling when no color is set
- [ ] **AC-7: Grid mode consistency** — TerminalGrid headers also reflect session color
- [ ] **AC-8: Input validation** — Server-side: reject names >64 chars or containing traversal chars; reject colors not in predefined palette. Return 400 with descriptive message
- [ ] **AC-9: Inline editing** — Users can rename/recolor an active session from the sidebar without navigating away. Changes reflected immediately in sidebar and top bar
- [ ] **AC-10: Recent sessions** — "Recent" section displays persisted names and colors
- [ ] **AC-11: Event propagation** — Name/color changes propagate via existing `session-update` Tauri event without page refresh

## Testing Requirements

- [ ] Backward compat: deserialize old `session.json` files without `name`/`color` fields
- [ ] PATCH endpoint: valid update, invalid name (too long, traversal chars), invalid color (not in palette)
- [ ] Sidebar rendering: with/without custom name, with/without color
- [ ] Terminal header: color accent present/absent based on session color
- [ ] Event propagation: PATCH triggers UI update without refresh
- [ ] Use `cargo check --tests` (Windows DLL issue with `cargo test`)

## Implementation Notes

**Suggested worker split for Hive session:**
- **Worker 1 (backend)**: Rust model + API + persistence — codex or droid CLI
- **Worker 2 (frontend)**: TypeScript store + Svelte UI components — claude CLI for UI polish

**Schedule after** `feat/evaluator-peer-architecture` merges to avoid conflicts in shared files (`controller.rs`, `storage/mod.rs`).

---

> 🔍 **Investigation**: 10 agents (7 scouts + 3 planners) across 5 AI providers | 12 files identified | Confidence: **HIGH**
>
> 🤖 Generated with [Claude Code](https://claude.com/claude-code)
