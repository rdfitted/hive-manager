# Lattice Design System Migration

## What
Migrate Hive Manager's visual identity from Tokyo Night to the Lattice design system — a retro-futuristic "mission control" aesthetic with deep space backgrounds, cyan/amber accents, glow effects, sharp edges, and CRT atmospheric overlays.

## Why
Push Svelte/Rust desktop app to its visual limits. The Lattice design system is purpose-built for agent orchestration UIs and brings a cohesive, high-contrast, industrial aesthetic that better fits the mission-control nature of Hive Manager.

## Current State
- **Theme**: Tokyo Night (soft dark, lavender/blue palette)
- **Font**: Inter only (400-700)
- **Radius**: 4-12px rounded corners
- **Effects**: None — flat surfaces, no atmospheric overlays
- **Tokens**: CSS variables scattered across `+page.svelte` `:root` and component-level overrides with hardcoded hex
- **Status colors**: Soft pastels (green `#9ece6a`, yellow `#e0af68`, red `#f7768e`)

## Target State
- **Theme**: Lattice (deep space void, cyan/amber accents, phosphor glows)
- **Fonts**: Rajdhani (display), Titillium Web (body), IBM Plex Mono (code/data)
- **Radius**: 0-2px sharp edges throughout
- **Effects**: CRT scanlines, noise texture, vignette overlay
- **Tokens**: Centralized in `src/lib/styles/lattice-tokens.css` + `lattice-base.css`, imported from layout
- **Status colors**: High-contrast with glow (green `#00FF66`, cyan `#00E5FF`, red `#FF3366`)

## Key Decisions
1. **Terminal untouched** — xterm.js theme stays Tokyo Night; only container/chrome around terminals changes
2. **Pure CSS migration** — no component logic changes, no Rust changes
3. **Token vars everywhere** — replace all hardcoded hex with `var(--token)` references
4. **All three Lattice fonts** — replace Inter entirely
5. **Sharp edges** — 0-2px radius, no exceptions outside terminal

## Constraints
- No functional regressions — hover states, transitions, collapse behavior must all survive
- Terminal content rendering must not change
- Status color semantics must remain clear (success/warning/error/running distinguishable)

## Success Criteria
- [ ] App visually matches Lattice aesthetic (deep dark, sharp, glowing)
- [ ] All components use token variables, zero hardcoded hex outside terminal
- [ ] CRT effects visible (scanlines, noise, vignette)
- [ ] Three-font typography hierarchy in use
- [ ] No functional regressions
