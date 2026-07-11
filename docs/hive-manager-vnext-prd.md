# Hive Manager vNext — Product Requirements

## Product Intent

Hive Manager is a **human-first, operator-controlled local meta-harness for CLI agents**.

It helps an operator launch, supervise, compare, and steer multi-agent coding sessions without turning into a bloated infrastructure platform.

The next version simplifies the mental model while exposing topology decisions:

- **Hive** = an Opus Queen plus manager-launched coding principals
- **Native children** = bounded micro-delegation inside a capable principal harness
- **Fusion** = multiple peer Hives in separate worktrees
- **Solo** = one directly supervised coding agent
- **Resolver** = synthesis across Fusion outputs
- **Swarm** = legacy-compatible programmatic surface, removed from the primary launch flow

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
6. Keep capability facts separate from operator delegation policy
7. Make Master Planner produce bounded Assignment Contracts, never implementation

### Non-goals
- Kubernetes support
- Distributed control plane / hub-broker architecture
- Enterprise credential isolation systems
- Generic agent infrastructure platform ambitions
- Deeper investment in nested Swarm orchestration
- Hidden topology expansion beyond the operator's configured policy

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
- one primary launch profile (`Hive`, `Fusion`, or `Solo`; the internal
  `SessionMode` enum still models Hive/Fusion/Debate while Solo is represented
  by the launch/session type contract)
- one or more Cells
- state, events, artifacts, and operator actions

### Cell
The true unit of execution and isolation.

A Cell contains:
- one operator-selected workspace topology
- one or more agents
- one local objective
- one lifecycle state
- one artifact bundle
- one event stream

### HiveCell
A collaborating unit:
- Queen + manager-launched coding principals
- shared worktree by recommendation, or isolated worktrees per managed principal when the operator selects `isolated_cell`
- operator-selected branch/worktree plan

### ResolverCell
A synthesis unit:
- compares candidate cells
- reads summaries, diffs, test results, artifacts
- picks a winner or proposes a hybrid path
- **recommendation-only by default** (does not write code)
- single agent by default (review pair available as a template variant)

### Workspace
Selected for the **Cell** by execution policy, not inferred from an agent's CLI.

- `shared_cell` gives the Queen and managed principals one collaborative workspace/worktree
- `isolated_cell` gives managed principals explicit per-principal isolation while preserving the Cell as the supervision boundary
- Cells **between** each other have separate worktrees when isolation matters

**Principle:** Recommend shared context where collaboration is the point, but keep workspace topology under operator control. Split context where comparison or explicit isolation is the point.

### ExecutionPolicy
The operator-owned launch contract:
- launch kind (`auto`, `hive`, or `solo`; Fusion remains an explicit session mode)
- workspace strategy
- separate Queen and principal native-delegation policies
- optional maximum child count and delegation depth

### CapabilityCard
Harness support facts declared by Hive Manager's current CLI adapter profile. This is not yet a live binary/version probe. Claude and Codex are profiled as supporting native delegation; unprofiled harnesses remain `unknown`, not guessed supported or unsupported.

Capability and authorization are deliberately different. `disabled` always wins. `auto` permits only known support. `encouraged` is explicit operator authorization without changing the underlying support fact; a harness known to be unsupported remains off.

### AssignmentContract
A bounded work package for a managed principal. It states the objective, acceptance criteria, owned paths, prohibited actions, required validation, and delivery format. Native children inherit that contract and cannot expand its authority or path ownership.

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
2. **Shared context is recommended inside a Hive; explicit principal isolation remains available**
3. **Artifacts matter more than raw output volume**
4. **State should be explicit, not inferred from vibes**
5. **Fusion is parallel exploration; Resolver is judgment**
6. **If a feature makes the app feel like infra software, it's probably wrong**
7. **The operator owns topology, workspaces, models, and delegation policy**
8. **Capability inference reports adapter-profile facts; it does not grant permission**
9. **Native children remain inside their parent's Assignment Contract**

---

## Mode Strategy

### Hive
One collaborative cell with an operator-selected workspace strategy. `shared_cell` is recommended for close collaboration; `isolated_cell` is available when each managed principal needs a separate worktree. An Opus Queen coordinates the principals, and the built-in recommendation assigns Codex `gpt-5.6` to backend and frontend work. These are defaults, and explicit operator choices remain authoritative.

**Success criteria:**
- clear roster of agents
- selected workspace topology visible
- strong Queen coordination
- easy intervention when an agent stalls
- clean cell-level summary at the end
- native children, when authorized, stay within their principal's contract

### Fusion
Multiple peer HiveCells in separate worktrees tackling the same objective. The mode for ambiguity, high stakes, and comparison.

**Must support mixed-model candidate strategies from day one.** This is one of Fusion's strongest selling points — different CLI/model per candidate cell.

**Success criteria:**
- easy side-by-side visibility across candidates
- clean worktree isolation between candidates
- consistent artifact bundle from each Hive
- Resolver can compare results without terminal archaeology

### Solo
One directly supervised agent with no manager-created principal topology. Use it when the objective is already bounded or orchestration would add ceremony without useful separation.

### Resolver
A dedicated synthesis lane, not a queen-above-queens.

**Success criteria:**
- picks or combines outputs reliably
- surfaces rationale and tradeoffs
- gives the operator a useful final recommendation

### Swarm
Removed from the session builder's primary flow and deprecated from active investment. Keep existing sessions and programmatic launch callers compatible; a labeled Legacy surface may expose it without presenting it as a recommended topology.

### Master Planner
Master Planner is a contract-authoring phase, not an implementer. It may inspect context and divide the objective into bounded Assignment Contracts, then must stop before editing code or launching an unapproved topology. The operator reviews or amends the proposed contracts before execution.

---

## Communication Architecture

**All communication uses HTTP** via the existing Axum server on port 18800.

- **Frontend-to-backend:** HTTP (same as agents)
- **Agent-to-manager:** HTTP (workers reporting status, polling tasks, posting artifacts)

One API, one set of handlers, one test surface. External CLI processes cannot invoke Tauri IPC, but they can hit localhost. Keeping a single protocol avoids maintaining two communication layers.

---

## UX Direction

### Session builder
Should ask: objective, repo/project, primary launch type (Hive, Fusion, or Solo), number of candidate Hives (if Fusion), CLI/model assignments per managed principal, workspace strategy, Queen/principal delegation policies, optional child/depth limits, and optional Resolver configuration. Topology preview must distinguish manager-launched principals from potential native children.

### Models and labels
Use canonical model IDs in configuration and launch payloads. The current canonical IDs are `gpt-5.6` and `fable`; **Sol** and **Fable** are display labels only. A direct Hive recommends an Opus Queen plus one generic Codex `gpt-5.6` principal; specialized built-in templates can provide backend/frontend principals. Older models remain selectable, and an explicit operator selection always overrides a recommendation.

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
- capability cards and persisted execution/delegation policy

### v0.19 — Cell-Based Workspace Model
"Hive workspaces are explicit: shared by recommendation, isolated when the operator chooses."

- workspace plan owned by the Cell, with per-principal assignments only when `isolated_cell` is selected
- shared-cell and isolated-per-principal Hive strategies
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
- Assignment Contract templates for Master Planner and principals

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
