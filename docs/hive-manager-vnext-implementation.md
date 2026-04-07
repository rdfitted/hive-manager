# Hive Manager vNext — Technical Implementation Plan

## Purpose

This document translates the vNext PRD into a practical implementation plan for the current Hive Manager codebase.

- Repo: `hive-manager/`
- Tauri v2, Svelte 5 frontend, Rust backend
- Current version: 0.17.1
- HTTP API on port 18800 (Axum) — **sole communication layer** for both frontend and agents

This is not a greenfield redesign. It is an incremental refactor and extension plan.

---

## Strategic Shift

Move from:
- terminal/process-centric orchestration
- mode-heavy top-level concepts
- loosely inferred state

To:
- **Session / Cell / Agent** data model
- explicit lifecycle state machines
- cell-level workspace ownership
- structured events and artifacts
- first-class Fusion + Resolver support

---

## Core Domain Entities

### Session
```rust
struct Session {
    id: String,
    name: String,
    objective: String,
    project_path: PathBuf,
    mode: SessionMode,        // Hive | Fusion
    status: SessionStatus,
    created_at: DateTime,
    updated_at: DateTime,
    cells: Vec<CellId>,
    launch_config: LaunchConfig,
    artifacts: Vec<ArtifactBundle>,
    events: Vec<EventId>,
}
```

### Cell
```rust
struct Cell {
    id: String,
    session_id: String,
    cell_type: CellType,      // Hive | Resolver
    name: String,
    status: CellStatus,
    objective: String,
    workspace: Workspace,
    agents: Vec<AgentId>,
    artifacts: Option<ArtifactBundle>,
    events: Vec<EventId>,
    depends_on: Vec<CellId>,
}
```

### Agent
```rust
struct Agent {
    id: String,
    cell_id: String,
    role: AgentRole,          // Queen | Worker | Resolver | Reviewer | Tester
    label: String,
    cli: String,
    model: Option<String>,
    status: AgentStatus,
    process_ref: Option<ProcessRef>,
    terminal_ref: Option<TerminalRef>,
    last_event_at: Option<DateTime>,
}
```

### Workspace
```rust
struct Workspace {
    strategy: WorkspaceStrategy,  // SharedCell | IsolatedCell
    repo_path: PathBuf,
    base_branch: String,
    branch_name: String,
    worktree_path: Option<PathBuf>,
    is_dirty: bool,
}
```

### ArtifactBundle
```rust
struct ArtifactBundle {
    summary: Option<String>,          // mandatory for completion
    changed_files: Vec<String>,       // mandatory for completion
    commits: Vec<String>,             // mandatory for completion
    branch: String,                   // mandatory for completion
    test_results: Option<TestResults>,
    diff_summary: Option<String>,
    unresolved_issues: Vec<String>,
    confidence: Option<f32>,
    recommended_next_step: Option<String>,
}
```

### Event
```rust
struct Event {
    id: String,
    session_id: String,
    cell_id: Option<String>,
    agent_id: Option<String>,
    event_type: EventType,
    timestamp: DateTime,
    payload: serde_json::Value,
    severity: Severity,
}
```

---

## Lifecycle State Machines

Implement explicitly in Rust. Backend owns all state transitions.

### SessionStatus
`drafting` → `preparing` → `launching` → `active` → `resolving` → `completed`

Failure branches: `partial_failure`, `failed`, `cancelled`

### CellStatus
`queued` → `preparing` → `launching` → `running` → `summarizing` → `completed`

Side states: `waiting_input`, `failed`, `killed`

### AgentStatus
`queued` → `launching` → `running` → `completed`

Side states: `waiting_input`, `failed`, `killed`

**Do not derive primary state from terminal text.** Terminal parsing may contribute signals, but the backend owns transitions.

---

## Backend Modules (Rust)

### 1. Domain layer

New modules under `src-tauri/src/`:

```
domain/
  mod.rs
  session.rs
  cell.rs
  agent.rs
  workspace.rs
  artifact.rs
  event.rs
  status.rs
```

Separate business model from launch mechanics. Types are serializable and shared to frontend over HTTP.

### 2. Orchestration layer

Split from current `controller.rs` (4200+ lines).

```
orchestrator/
  mod.rs
  planner.rs
  session_orchestrator.rs
  fusion.rs
  resolver.rs
```

Responsibilities:
- building session plans
- creating cells
- assigning workspaces
- deciding launch order
- transitioning states
- collecting artifacts

### 3. Runtime layer

```
runtime/
  mod.rs
  traits.rs
  local_process.rs
  local_pty.rs
  worktree.rs
```

Runtime trait:

```rust
pub trait RuntimeAdapter {
    fn launch(&self, spec: LaunchSpec) -> Result<LaunchedAgent, RuntimeError>;
    fn stop(&self, process_id: &str) -> Result<(), RuntimeError>;
    fn write(&self, process_id: &str, input: &str) -> Result<(), RuntimeError>;
    fn resize(&self, process_id: &str, cols: u16, rows: u16) -> Result<(), RuntimeError>;
}
```

Initial implementations: `LocalPtyRuntime`, `LocalProcessRuntime`. Container/remote runtimes are future work only.

### 4. CLI adapter layer

```
adapters/
  mod.rs
  claude_code.rs
  codex.rs
  gemini.rs
  opencode.rs
  droid.rs
```

Adapter trait:

```rust
pub trait CliAdapter {
    fn cli_name(&self) -> &'static str;
    fn build_launch_command(&self, spec: &AgentLaunchSpec) -> LaunchCommand;
    fn detect_status_signal(&self, line: &str) -> Option<AgentSignal>;
    fn build_bootstrap_prompt(&self, context: &BootstrapContext) -> String;
}
```

Responsibilities: command generation, env injection, model arg mapping, status detection, prompt wrapping.

### 5. Workspace manager

```
workspace/
  mod.rs
  manager.rs
  git.rs
```

Rules:
- Hive mode → one shared worktree for the HiveCell
- Fusion mode → one worktree per candidate HiveCell
- ResolverCell → no write workspace (recommendation-only by default)

Branch naming:
- Hive: `hive/<session-id>/<cell-name>`
- Fusion candidate: `fusion/<session-id>/<candidate-name>`
- Resolver: `resolver/<session-id>`

### 6. Event pipeline

```
events/
  mod.rs
  bus.rs
  emitter.rs
```

Event types: `session.created`, `session.status_changed`, `cell.created`, `cell.status_changed`, `workspace.created`, `agent.launched`, `agent.completed`, `agent.waiting_input`, `artifact.updated`, `resolver.selected_candidate`

Raw terminal output stays separate from structured events.

### 7. Artifact collection

```
artifacts/
  mod.rs
  collector.rs
  resolver_input.rs
```

Collects per-cell outputs, normalizes changed files, captures commits, stores test results, exposes consistent bundle for Resolver consumption.

### 8. Resolver orchestration

```
orchestrator/resolver.rs
```

Flow: wait for candidate HiveCells → assemble artifact bundles → launch ResolverCell → persist output → transition session to `resolving` → `completed`.

Resolver consumes structured candidate summaries first, raw logs second.

---

## HTTP API Evolution

Extend the existing Axum API on port 18800. All consumers (frontend + agents) use the same endpoints.

### New endpoints (additive)

```
POST   /api/sessions                          # create session plan
POST   /api/sessions/{id}/launch              # launch session
DELETE /api/sessions/{id}                      # stop/cancel session

GET    /api/sessions/{id}/cells               # list cells
GET    /api/sessions/{id}/cells/{cid}         # get cell detail
DELETE /api/sessions/{id}/cells/{cid}         # stop cell

GET    /api/sessions/{id}/cells/{cid}/agents  # list agents in cell
DELETE /api/sessions/{id}/agents/{aid}        # stop agent
POST   /api/sessions/{id}/agents/{aid}/input  # send agent input

GET    /api/sessions/{id}/events              # event stream
GET    /api/sessions/{id}/cells/{cid}/artifacts  # cell artifacts
POST   /api/sessions/{id}/cells/{cid}/artifacts  # agent posts artifacts
```

### SSE for real-time updates

```
GET    /api/sessions/{id}/stream              # SSE: state + event updates
```

Keep terminal output on a separate channel from structured state updates.

---

## Frontend Plan (Svelte)

### 1. Stores

```
src/lib/stores/
  sessions.ts
  cells.ts
  agents.ts
  events.ts
  artifacts.ts
  ui.ts
```

### 2. Components

```
src/lib/components/
  session/
    SessionHeader.svelte
    SessionOverview.svelte
    SessionTimeline.svelte
    SessionBuilder.svelte
  cell/
    CellCard.svelte
    CellGrid.svelte
    CellDetailPanel.svelte
    WorkspaceBadge.svelte
  agent/
    AgentList.svelte
    AgentStatusBadge.svelte
    AgentTerminalPanel.svelte
  artifacts/
    ArtifactSummary.svelte
    TestResultsPanel.svelte
    DiffSummaryPanel.svelte
  fusion/
    FusionComparisonView.svelte
    ResolverPanel.svelte
```

### 3. Types

```
src/lib/types/domain.ts
```

Mirror Rust domain entities. Single source of truth for frontend type definitions.

### UI priority shift
Current UI emphasizes terminals. vNext emphasizes: cell status → worktree/branch → artifacts → terminal access. Terminals remain available as drill-down, not the entire product.

---

## Persistence

### Session storage

```
app-data/
  sessions/
    <session-id>/
      session.json
      cells.json
      events.jsonl
      artifacts/
        <cell-id>.json
      terminals/
        <agent-id>.log
```

JSON/JSONL first — easy to debug, inspect, and migrate while the model is evolving. SQLite can come later if needed.

### Migration from existing data
Old session data (`.hive-manager/` task files, existing JSONL) is **read-only** under the new model. No automatic migration. Previous sessions remain inspectable but do not conform to the new structure.

---

## Phased Delivery

### Phase 1 (v0.18) — Domain + State Refactor

**Tasks:**
- Add Rust domain types in `domain/`
- Add explicit state enums
- Wire structured session/cell/agent updates over HTTP
- Keep current UI mostly working while consuming richer state

**Exit criteria:**
- Backend owns authoritative session/cell/agent state
- Frontend can render status without terminal heuristics alone

### Phase 2 (v0.19) — Cell-Based Workspace Management

**Tasks:**
- Implement WorkspaceManager
- Create shared worktree flow for Hive
- Create isolated candidate worktrees for Fusion
- Expose branch/worktree in UI

**Exit criteria:**
- Hive sessions create one workspace per HiveCell
- Fusion sessions create one workspace per candidate HiveCell

### Phase 3 (v0.20) — Artifact Bundles + Resolver

**Tasks:**
- Define artifact bundle contract
- Collect cell outputs
- Add ResolverCell backend flow
- Add Resolver UI surface
- Fusion comparison view

**Exit criteria:**
- Candidate cells emit comparable artifacts
- Resolver can select or hybridize outputs
- Operator can identify winning candidate without reading every terminal

### Phase 4 (v0.21) — Templates and Launch UX

**Tasks:**
- Session templates
- Role packs
- Launch presets
- Improved session builder

**Exit criteria:**
- User can launch a useful session in under 30 seconds from a template

### Phase 5 (v0.22) — Observability + Replay

**Tasks:**
- Timeline view
- Event filters
- Artifact browsing
- Session replay

**Exit criteria:**
- Operator can reconstruct what happened from structured data

---

## Testing Strategy

### Backend tests
- State transitions
- Workspace creation rules
- CLI adapter command generation
- Event emission
- Artifact normalization
- Resolver input construction

Note: `cargo test` has a known Windows DLL issue — use `cargo check --tests` for compilation validation.

### Frontend tests
- Session builder flows
- Cell status rendering
- Fusion comparison UI
- Resolver display logic
- Terminal-to-agent association

### Manual integration tests
1. Launch single Hive
2. Launch Fusion with 2 candidate Hives + Resolver
3. Stop one failed candidate while others continue
4. Detect waiting-for-input state and surface it clearly
5. Persist and reload previous session with artifacts intact

---

## Implementation Risks

| Risk | Mitigation |
|------|-----------|
| Rewriting all UI at once | Land domain/state first, then reshape screens |
| Resolver with weak inputs | Define artifact contract before building comparison UX |
| Runtime abstraction too generic too early | Design cleanly, implement only local runtimes for now |
| Swarm compatibility clutter | Keep backward compat if needed, don't let it dictate vNext design |

---

## Immediate Next Steps

1. Add `domain/` module — Session/Cell/Agent/Status/Event types
2. Add runtime trait, normalize existing CLI launchers behind it
3. Implement WorkspaceManager with cell-based worktree rules
4. Introduce event bus + structured event persistence
5. Extend HTTP API with cell/artifact/event endpoints
6. Refactor frontend stores around session/cell/agent hierarchy
7. Build minimal cell-first session overview screen
8. Add artifact bundle schema
9. Implement first Resolver flow for Fusion sessions
