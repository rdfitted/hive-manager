# Implementation Plan

## Approach
Incremental CSS-only migration in 5 phases. Each phase is independently shippable — the app works after every phase, just with mixed old/new styling until complete.

---

## Phase 1: Token Foundation
Create centralized design token files.

**Deliverables:**
- `src/lib/styles/lattice-tokens.css` — all CSS custom properties
- `src/lib/styles/lattice-base.css` — typography utilities, component base classes, CRT effects, animations

**Token categories:**
- Surfaces: `--bg-void`, `--bg-surface`, `--bg-elevated`
- Borders: `--border-structural`, `--border-active`
- Text: `--text-primary`, `--text-secondary`, `--text-disabled`
- Accents: `--accent-cyan`, `--accent-amber`, `--accent-chrome`
- Status: `--status-success`, `--status-running`, `--status-warning`, `--status-error`, `--status-blocked`, `--status-queued`, `--status-canceled` (with RGB variants)
- Typography: `--font-display`, `--font-body`, `--font-mono`, `--text-micro` through `--text-h1`
- Spacing: `--space-1` through `--space-7` (4px base unit)
- Radius: `--radius-none` (0px), `--radius-sm` (2px)
- Motion: `--transition-fast` (200ms ease-out)

**Base classes:**
- `.font-display`, `.font-body`, `.font-mono`
- `.lattice-panel`, `.lattice-panel--active`
- `.lattice-btn`, `.lattice-btn--primary`, `.lattice-btn--secondary`, `.lattice-btn--ghost`, `.lattice-btn--danger`
- `.status-badge` variants for all status states
- `.lattice-input`, `.lattice-tab`
- CRT effects: `body::after` scanlines, `.fx-noise`, `.fx-vignette`

**Animations:**
- `@keyframes pulse-blocked`, `pulse-error`, `pulse`, `scan`, `spin`

---

## Phase 2: App Shell Migration
Wire token files into the app and replace root-level styling.

**Changes:**
- `src/app.html` — Replace Inter with Rajdhani + Titillium Web + IBM Plex Mono Google Font imports, update `background` to `--bg-void` (#060709)
- `src/routes/+layout.svelte` — Import `lattice-tokens.css` and `lattice-base.css`
- `src/routes/+page.svelte` — Remove all `:root` CSS variable definitions (now in tokens file), update layout-level styles to use new tokens
- Add CRT overlay elements (scanlines via `body::after`, noise/vignette containers)

**Exit criteria:** App loads with new background color, fonts rendering, CRT effects visible. Components still use old colors (mixed state is expected).

---

## Phase 3: Component Token Adoption
Replace all hardcoded hex values in component `<style>` blocks with token variables.

**Affected components (all `.svelte` files with `<style>` blocks):**
- Sidebars: `SessionSidebar.svelte`, `RightDrawer.svelte`
- Session: `SessionOverview.svelte`, `SessionHeader.svelte`
- Agents: `AgentStatusBar.svelte`, `AgentChip.svelte`
- Dialogs: `AddWorkerDialog.svelte`, `NewSessionDialog.svelte`
- Terminal chrome: `Terminal.svelte` (container only — xterm theme untouched)
- Any new components on the branch: `ArtifactBrowser.svelte`, replay/timeline components
- Conversation: `ConversationViewer.svelte`

**Pattern for each component:**
1. Find all hardcoded hex values and `rgba()` calls
2. Map to nearest Lattice token
3. Replace with `var(--token-name)`
4. Update border-radius to `var(--radius-none)` or `var(--radius-sm)`
5. Update font-family references to use token vars

**Terminal exception:** Inside `Terminal.svelte`, the xterm.js `theme` object and terminal content styling stays as-is. Only the surrounding container/header/chrome styles migrate.

---

## Phase 4: Status Colors & Glow Effects
Update all status-related styling to use Lattice's high-contrast palette with glow effects.

**Changes:**
- `AgentStatusBar.svelte` — `getStatusColor()` function values updated to Lattice status palette
- Status dots gain `box-shadow` glow using RGB status token variants
- Status badges gain Lattice styling (monospace, uppercase, tinted backgrounds, glow)
- Conversation sender colors updated to Lattice palette
- Session color indicators updated
- Add pulse animations for blocked/error states

---

## Phase 5: Polish & Verification
Final pass to ensure visual consistency and no regressions.

**Tasks:**
- Visual audit of every view/panel for stray Tokyo Night colors
- Verify all hover/focus states use Lattice tokens
- Verify transitions still work (sidebar collapse, dialog open/close)
- Verify terminal content rendering unchanged
- Test all status states render with correct colors and glows
- Check font rendering across all text contexts
- Verify CRT effects don't interfere with interaction (pointer-events: none)
- Remove any unused Tokyo Night CSS variables

---

## Risk Register
| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Missed hardcoded hex in a component | High | Low | Grep for `#[0-9a-fA-F]` after migration |
| CRT effects interfere with clicks | Low | High | `pointer-events: none` + high z-index |
| Font loading flash (FOUT) | Medium | Low | `font-display: swap` in Google Fonts URL |
| Status colors too similar in new palette | Low | Medium | Test all status states side-by-side |
