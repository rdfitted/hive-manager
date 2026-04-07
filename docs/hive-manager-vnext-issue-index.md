# Hive Manager vNext — Issue Index

## Overview

This document maps the vNext implementation issues to their phases, dependencies, and delivery order.

Current version: **0.17.1**
Target versions: **0.18 → 0.22**

---

## Phase 1 — Foundation Cleanup (v0.18)

Goal: Real internal spine without changing external personality.

| # | Issue | Dependencies | Risk |
|---|-------|-------------|------|
| #41 | Domain layer — Session, Cell, Agent, Workspace types | None (foundation) | Low |
| #42 | Runtime trait and process launching | #41 | Medium |
| #43 | CLI adapter normalization | #41, #42 | Low |
| #44 | Structured event pipeline | #41 | Medium |
| #45 | Split controller.rs into orchestration layer | #41, #42, #43, #44 | **High** |
| #46 | Extend HTTP API with cell/agent/event endpoints | #41, #44, #45 | Medium |

### Recommended order
```
#41 (domain types)
  ├── #42 (runtime trait)      ─┐
  ├── #43 (CLI adapters)        ├── #45 (orchestrator refactor)
  └── #44 (event pipeline)     ─┘        │
                                          └── #46 (HTTP API)
```

**Start with #41.** Then #42, #43, #44 can be worked in parallel. #45 depends on all four. #46 depends on #45.

---

## Phase 2 — Cell-Based Workspace Model (v0.19)

Goal: Worktree ownership matches the architecture.

| # | Issue | Dependencies | Risk |
|---|-------|-------------|------|
| #47 | WorkspaceManager with cell-based worktree rules | #41, #45 | Medium |
| #48 | Cell-first frontend — stores, components, workspace visibility | #41, #46, #47 | Medium |

### Recommended order
```
#47 (WorkspaceManager) → #48 (frontend refactor)
```

**#47 first** — frontend needs workspace data to display.

---

## Phase 3 — First-Class Fusion + Resolver (v0.20)

Goal: Fusion becomes a complete workflow.

| # | Issue | Dependencies | Risk |
|---|-------|-------------|------|
| #49 | Artifact bundle schema, collection, Resolver inputs | #41, #47, #46 | Medium |
| #50 | ResolverCell backend — orchestration, launch, output | #41, #45, #49 | **High** |
| #51 | Fusion comparison UI and Resolver panel | #48, #49, #50 | Medium |

### Recommended order
```
#49 (artifacts) → #50 (ResolverCell) → #51 (Fusion UI)
```

**Sequential chain.** Each step feeds the next.

---

## Phase 4 — Templates and Repeatability (v0.21)

Goal: Strong workflows become reusable.

| # | Issue | Dependencies | Risk |
|---|-------|-------------|------|
| #52 | Session templates, role packs, launch presets | #48, #47, #43 | Low |

---

## Phase 5 — Observability (v0.22)

Goal: Sessions are inspectable after the fact.

| # | Issue | Dependencies | Risk |
|---|-------|-------------|------|
| #53 | Timeline view, event filters, session replay | #44, #49, #48 | Low |

---

## Dependency Graph (full)

```
#41 Domain Types ──────────────────────────────────────────────────┐
 ├── #42 Runtime Trait ──────────┐                                 │
 ├── #43 CLI Adapters ───────────┤                                 │
 ├── #44 Event Pipeline ─────────┤                                 │
 │                               ├── #45 Orchestrator Refactor     │
 │                               │    ├── #46 HTTP API ────────────┤
 │                               │    │    ├── #47 WorkspaceManager│
 │                               │    │    │    ├── #48 Frontend ──┤
 │                               │    │    │    │    ├── #51 Fusion UI
 │                               │    │    │    │    ├── #52 Templates
 │                               │    │    │    │    └── #53 Observability
 │                               │    │    ├── #49 Artifacts ──────┤
 │                               │    │    │    └── #50 ResolverCell│
 │                               │    │    │         └── #51       │
```

---

## Critical Path

The longest dependency chain determines minimum delivery time:

**#41 → #42/#43/#44 → #45 → #46 → #47 → #48 → #49 → #50 → #51**

Phase 1 (#41–#46) is the bottleneck. Parallelize #42, #43, #44 to compress it.

---

## High-Risk Issues

1. **#45 — Orchestrator refactor**: Extracting from 4200-line controller.rs. Do incrementally — one flow at a time.
2. **#50 — ResolverCell**: First time building the full Resolver flow. Artifact quality from #49 determines success.
