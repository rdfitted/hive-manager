# Hive Manager vNext — Product Requirements

## Product Intent

Hive Manager is a **human-first desktop cockpit for CLI agents**.

It helps an operator launch, supervise, compare, and steer multi-agent coding sessions without turning into a bloated infrastructure platform.

The next version simplifies the mental model:

- **Hive** = one collaborative cell in one shared worktree
- **Fusion** = multiple peer Hives in separate worktrees
- **Resolver** = synthesis across Fusion outputs
- **Swarm** = deprecated from active investment

---

## Why This Matters

Hive Manager already has the right instinct: keep the human in the loop and make agent work visible.

What it lacks is a cleaner internal model. Right now the product risks being pulled between a lightweight personal operator tool and a heavier multi-agent orchestration framework.

This plan keeps it in the first camp while stealing only the best architectural ideas needed for scale and durability.

---

## Product Goals

### Primary goals
1. Make Hive the default, strongest workflow
2. Make Fusion the advanced workflow for high-stakes or ambiguous tasks
3. Introduce Resolver as a first-class synthesis role
4. Replace terminal-only visibility with state, events, and artifacts
5. Preserve speed and operator clarity

### Non-goals
- Kubernetes support
- Distributed control plane / hub-broker architecture
- Enterprise credential isolation systems
- Generic agent infrastructure platform ambitions
- Deeper investment in nested Swarm orchestration

---

## Target User

A power user running multiple coding CLIs locally who wants:
- less terminal chaos
- better visibility into what each agent is doing
- safer parallel experiments
- easier comparison of outcomes
- quicker intervention when agents drift or stall

### Likely behaviors
- launches parallel coding attempts
- mixes models/CLIs intentionally
- wants to supervise without micromanaging
- values speed over theoretical purity
- prefers concrete outputs over orchestration ceremony

---

## Core Product Model

### Session
The top-level run.

A Session contains:
- one objective
- one project/repo target
- one mode (`Hive` or `Fusion`)
- one or more Cells
- state, events, artifacts, and operator actions

### Cell
The true unit of execution and isolation.

A Cell contains:
- one shared workspace/worktree
- one or more agents
- one local objective
- one lifecycle state
- one artifact bundle
- one event stream

### HiveCell
A collaborating unit:
- Queen + Workers
- shared worktree
- shared branch strategy

### ResolverCell
A synthesis unit:
- compares candidate cells
- reads summaries, diffs, test results, artifacts
- picks a winner or proposes a hybrid path
- **recommendation-only by default** (does not write code)
- single agent by default (review pair available as a template variant)

### Workspace
Attached to the **Cell**, not the individual agent.

- Agents **within** a cell share one workspace/worktree
- Cells **between** each other have separate worktrees when isolation matters

**Principle:** Share context where collaboration is the point. Split context where comparison is the point.

### Artifact
Standardized output from each Cell:
- summary
- changed files
- commits
- branch/worktree path
- test results
- diff summary
- unresolved issues
- confidence/self-critique
- recommended next step

**Mandatory fields** (minimum for session completion): `summary`, `changedFiles`, `branch`, `commits`. All other fields are optional but encouraged. A cell that produced commits but no summary is "completed with warnings," not blocked.

### Event
Structured record of something meaningful that happened. Raw terminal output still exists but is not the primary source of truth.

---

## Product Principles

1. **Cells, not terminals, are the main object**
2. **Shared context inside a Hive; isolation between peer Hives**
3. **Artifacts matter more than raw output volume**
4. **State should be explicit, not inferred from vibes**
5. **Fusion is parallel exploration; Resolver is judgment**
6. **If a feature makes the app feel like infra software, it's probably wrong**

---

## Mode Strategy

### Hive
One collaborative cell in one shared worktree. The default mode for most real work.

**Success criteria:**
- clear roster of agents
- shared workspace visible
- strong Queen coordination
- easy intervention when an agent stalls
- clean cell-level summary at the end

### Fusion
Multiple peer HiveCells in separate worktrees tackling the same objective. The mode for ambiguity, high stakes, and comparison.

**Must support mixed-model candidate strategies from day one.** This is one of Fusion's strongest selling points — different CLI/model per candidate cell.

**Success criteria:**
- easy side-by-side visibility across candidates
- clean worktree isolation between candidates
- consistent artifact bundle from each Hive
- Resolver can compare results without terminal archaeology

### Resolver
A dedicated synthesis lane, not a queen-above-queens.

**Success criteria:**
- picks or combines outputs reliably
- surfaces rationale and tradeoffs
- gives the operator a useful final recommendation

### Swarm
Deprecated from active roadmap. Keep launchable for backward compat but remove from the session builder's primary flow. A "Legacy" or "Advanced" section is fine. If nobody complains, remove entirely in a later version.

---

## Communication Architecture

**All communication uses HTTP** via the existing Axum server on port 18800.

- **Frontend-to-backend:** HTTP (same as agents)
- **Agent-to-manager:** HTTP (workers reporting status, polling tasks, posting artifacts)

One API, one set of handlers, one test surface. External CLI processes cannot invoke Tauri IPC, but they can hit localhost. Keeping a single protocol avoids maintaining two communication layers.

---

## UX Direction

### Session builder
Should ask: objective, repo/project, mode (Hive or Fusion), number of candidate Hives (if Fusion), CLI/model assignments per cell, workspace strategy, optional Resolver configuration.

### Main session screen
Should emphasize: Cells, statuses, worktrees/branches, summaries, artifacts, terminal drill-downs. Not just a wall of terminals.

### Fusion screen
Must make it obvious at a glance: which candidate is progressing, which failed, which changed the most, which passed tests, what the Resolver concluded.

---

## Roadmap

### v0.18 — Foundation Cleanup
"Cleaner guts. Better state. Less weirdness."

- formal runtime abstraction
- explicit session/cell/agent lifecycle states
- structured event log alongside terminal output
- normalized CLI adapter contract

### v0.19 — Cell-Based Workspace Model
"Hive now works the way it should: one collaborative cell, one shared workspace."

- worktree attached to Cell, not individual agent
- shared-worktree Hive mode
- isolated worktree per Fusion candidate
- branch/worktree visibility in UI
- launch preview before session start

### v0.20 — First-Class Fusion + Resolver
"Fusion is now real: parallel Hives, side-by-side comparison, Resolver verdicts."

- first-class ResolverCell
- candidate artifact bundles
- side-by-side candidate comparison UI
- Resolver panel with selection rationale
- Swarm deprioritized in UI

### v0.21 — Templates and Repeatability
"Save your best workflows and launch them fast."

- session templates
- role packs
- saved launch presets
- editable bootstrap prompt bundles

### v0.22 — Observability
"See what happened, not just what scrolled by."

- timeline/event view
- artifact inspection panels
- replayable session history
- structured comparison tooling for Fusion sessions

---

## Migration

### Existing sessions
Old session data (`.hive-manager/` task files, existing JSONL) is read-only under the new model. No automatic migration — previous sessions remain inspectable but do not conform to the new cell/artifact structure.

### What to keep
- current terminal functionality
- existing Tauri/Svelte shell
- current supported CLI roster
- HTTP API on port 18800

### What to refactor
- session state model
- mode model
- orchestration logic
- launch flow
- runtime abstractions

### What to de-emphasize
- Swarm-specific assumptions
- planner-heavy hierarchy as primary product mental model

---

## Key Risks

| Risk | Mitigation |
|------|-----------|
| Over-architecting | Keep everything local-first and operator-first |
| Terminal-first inertia | Push structured events and artifact bundles early |
| Swarm baggage | De-emphasize quickly, stop designing around it |
| Weak Resolver output | Resolver consumes structured candidate bundles, not raw chaos |

---

## What Not to Build Yet

- Kubernetes support
- Multi-machine hub/broker orchestration
- Full credential sandboxing
- Per-agent container isolation as default
- Further Swarm expansion

These can wait unless real-world workflow proves they are necessary.
