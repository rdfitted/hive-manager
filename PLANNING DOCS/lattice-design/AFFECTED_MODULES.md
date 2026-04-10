# Affected Modules

## New Files (to create)
| File | Impact | Description |
|------|--------|-------------|
| `src/lib/styles/lattice-tokens.css` | HIGH | All design tokens — colors, fonts, spacing, radius, motion |
| `src/lib/styles/lattice-base.css` | HIGH | Typography utilities, component base classes, CRT effects, animations |

## Modified Files
| File | Impact | What Changes |
|------|--------|-------------|
| `src/app.html` | HIGH | Font imports (Inter -> 3 Lattice fonts), background color |
| `src/routes/+layout.svelte` | HIGH | Import token/base CSS files, add CRT overlay containers |
| `src/routes/+page.svelte` | HIGH | Remove `:root` vars (~30 lines), update layout styles to tokens |
| `src/lib/components/SessionSidebar.svelte` | MEDIUM | Replace hex colors, update radius, font refs |
| `src/lib/components/RightDrawer.svelte` | MEDIUM | Replace hex colors, update radius |
| `src/lib/components/SessionOverview.svelte` | MEDIUM | Replace hex colors, update radius |
| `src/lib/components/AgentStatusBar.svelte` | HIGH | Status color function + glow effects |
| `src/lib/components/Terminal.svelte` | LOW | Container/header styles only, xterm theme untouched |
| `src/lib/components/AddWorkerDialog.svelte` | MEDIUM | Dialog chrome, button styles, radius |
| `src/lib/components/NewSessionDialog.svelte` | MEDIUM | Dialog chrome, button styles, radius |
| `src/lib/components/ConversationViewer.svelte` | MEDIUM | Sender colors, message styling |
| `src/lib/components/artifacts/ArtifactBrowser.svelte` | MEDIUM | New component — adopt tokens from start |
| `src/lib/components/replay/*.svelte` | MEDIUM | New components — adopt tokens from start |
| `src/lib/components/timeline/*.svelte` | MEDIUM | New components — adopt tokens from start |

## Untouched
| File | Reason |
|------|--------|
| `src/lib/components/Terminal.svelte` (xterm theme) | Explicit exclusion — terminal content stays Tokyo Night |
| `src-tauri/src/**` | No Rust changes — pure CSS migration |
| `src/lib/stores/**` | No logic changes |
