# Hive Manager vNext — Technical Implementation Plan

## Purpose

This document translates the vNext PRD into a practical implementation plan for the current Hive Manager codebase.

- Repo: `hive-manager/`
- Tauri v2, Svelte 5 frontend, Rust backend
- Current version: 0.34.0
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
- an operator-controlled meta-harness with explicit launch topology
- explicit lifecycle state machines
- cell-level workspace ownership
- structured events and artifacts
- first-class Fusion + Resolver support
- managed principals at the macro layer and bounded native children at the micro layer

---

## Core Domain Entities

### Session
```rust
struct Session {
    id: String,
    name: String,
    objective: String,
    project_path: PathBuf,
    mode: SessionMode,        // Hive | Fusion | Debate
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
    workspace: Workspace,     // current /cells projection is singular
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
    role: AgentRole,          // Queen | Principal | Resolver | Reviewer | Tester
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

The shipped `Cell` projection still exposes one `workspace: Workspace`. The
execution policy can create a shared primary worktree or isolated Queen and
principal worktrees, but `/cells` does not yet project that full list after a
reload. The richer target projection is:

```rust
struct WorkspacePlan {
    strategy: WorkspaceStrategy,  // SharedCell | IsolatedCell
    workspaces: Vec<Workspace>,   // one shared, or one per managed principal
}

struct Workspace {
    principal_id: Option<AgentId>, // None for the shared-cell workspace
    repo_path: PathBuf,
    base_branch: String,
    branch_name: String,
    worktree_path: Option<PathBuf>,
    is_dirty: bool,
}
```

The Cell conceptually owns that workspace plan. A shared workspace is the
built-in recommendation, while `IsolatedCell` creates explicit per-principal
assignments selected by the operator. Replacing the singular API field with
`WorkspacePlan` is follow-up work, not behavior claimed by this release.

### Execution policy and capability facts
```rust
enum HiveLaunchKind { Auto, Hive, Solo }
enum NativeDelegationMode { Disabled, Auto, Encouraged }
enum CapabilitySupport { Supported, Unsupported, Unknown }

struct DelegationPolicy {
    mode: NativeDelegationMode,
    max_children: Option<u8>,
    max_depth: Option<u8>,
}

struct HiveExecutionPolicy {
    launch_kind: HiveLaunchKind,
    workspace_strategy: WorkspaceStrategy,
    queen_delegation: DelegationPolicy,
    principal_delegation: DelegationPolicy,
}

struct CapabilityCard {
    native_delegation: CapabilitySupport,
}
```

`CapabilityCard` is factual and conservative within its declared source: today it reflects Hive Manager's CLI adapter profile, not a live binary/version/feature probe. Claude and Codex are adapter-declared supported, while unprofiled harnesses remain `Unknown`. Policy is authorization: `Disabled` always wins, `Auto` permits declared support, and `Encouraged` records explicit operator authorization without mutating the support fact. A known `Unsupported` capability remains off. `max_children` and `max_depth` are prompt-level guidance until a CLI adapter can map them to harness-enforced runtime controls; native harness limits remain authoritative.

### Assignment contract
```rust
struct AssignmentContract {
    objective: String,
    acceptance_criteria: Vec<String>,
    owned_paths: Vec<PathBuf>,
    prohibited_actions: Vec<String>,
    required_validation: Vec<String>,
    delivery_format: String,
}
```

Master Planner produces these contracts and stops before implementation. Manager-launched principals receive one contract each. Native children inherit their parent's contract and may narrow scope, but may not expand authority or path ownership.

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
  execution.rs
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
- converting Master Planner output into reviewable Assignment Contracts
- creating cells
- assigning workspaces
- deciding launch order
- resolving operator policy against factual harness capabilities
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
  antigravity.rs
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

Responsibilities: command generation, env injection, model arg mapping, status detection, prompt wrapping, and factual capability reporting. During the incremental migration, `CliRegistry` is the capability resolver; the adapter contract may own those facts once every launch path uses adapters.

Canonical model IDs flow through configuration and launch specs. Use `gpt-5.6-sol` and `fable`; **GPT-5.6 Sol** and **Fable 5** are presentation names. Built-in recommendations use Opus for Queens and Codex `gpt-5.6-sol` for backend/frontend coding principals. Normalize the legacy Codex value `gpt-5.6` at the launch boundary so older persisted sessions and templates remain runnable. Older models remain selectable, and explicit operator configuration is authoritative.

### Prompt and delegation contract

Prompt construction has three layers:

1. **Master Planner contract prompt:** inspect context, emit bounded Assignment Contracts, and stop before implementation.
2. **Managed principal prompt:** include the Assignment Contract, Capability Card, resolved delegation policy, workspace, and reporting obligations.
3. **Native child prompt:** inherit the principal's Assignment Contract and limits; it cannot broaden owned paths, authority, validation requirements, or delivery obligations.

The UI and event stream identify manager-launched principals as the macro topology. Native children are micro topology internal to a capable harness. They do not silently become manager-owned cells. Configured child/depth values are repeated in their assignment contract as guidance and must not be presented as manager-enforced runtime caps.

Implement prompt construction as a deterministic compiler, not scattered string mutation. Its inputs are the agent role, Assignment Contract, Capability Card, resolved `HiveExecutionPolicy`, workspace assignment, CLI behavior profile, and reporting endpoints. Its output must preserve the factual capability value, state whether native delegation is authorized, include child/depth limits, and repeat the non-expansion rule. Master Planner uses a separate contract-only target that cannot fall through to an implementation prompt.

### 5. Workspace manager

```
workspace/
  mod.rs
  manager.rs
  git.rs
```

Rules:
- Hive mode + `shared_cell` → one recommended collaborative worktree for the HiveCell
- Hive mode + `isolated_cell` → one isolated worktree per manager-launched principal
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

Execution-policy and capability additions are additive within the current persisted schema. Apply `#[serde(default)]` to new policy fields so sessions created before this capability contract still load. The serde fallback preserves legacy behavior (`Auto` launch, `IsolatedCell` workspace, and `Auto` delegation) rather than fabricating explicit operator intent; the new-session UI may still recommend `SharedCell`. Existing Swarm sessions and programmatic Swarm callers remain compatible even though Swarm is absent from the primary launch flow.

---

## Phased Delivery

### Phase 1 (v0.18) — Domain + State Refactor

**Tasks:**
- Add Rust domain types in `domain/`
- Add explicit state enums
- Add execution policy, capability support, and serde defaults for legacy sessions
- Resolve capability facts independently from operator delegation authorization
- Wire structured session/cell/agent updates over HTTP
- Keep current UI mostly working while consuming richer state

**Exit criteria:**
- Backend owns authoritative session/cell/agent state
- Frontend can render status without terminal heuristics alone

### Phase 2 (v0.19) — Cell-Based Workspace Management

**Tasks:**
- Implement WorkspaceManager
- Create shared-cell and isolated-per-principal flows for Hive
- Create isolated candidate worktrees for Fusion
- Expose branch/worktree in UI

**Exit criteria:**
- Hive sessions honor the operator-selected `shared_cell` or `isolated_cell` strategy
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
- Topology preview distinguishing managed principals from possible native children
- Master Planner Assignment Contract templates

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
- Capability inference for Claude, Codex, and unknown harnesses
- Delegation-policy matrix: Disabled, Auto, Encouraged × Supported, Unsupported, Unknown
- Legacy session deserialization with omitted execution-policy fields
- Programmatic legacy Swarm launch compatibility
- Event emission
- Artifact normalization
- Resolver input construction

Run focused `cargo test` filters for changed modules first. Use `cargo check --tests` as a compilation fallback when a machine-specific Windows runtime dependency prevents test execution.

### Frontend tests
- Session builder flows
- Cell status rendering
- Fusion comparison UI
- Resolver display logic
- Terminal-to-agent association

### Manual integration tests
1. Launch single Hive
2. Launch Fusion with 2 candidate Hives + Resolver
3. Launch Solo without creating a managed-principal topology
4. Confirm Disabled blocks native delegation even on Claude/Codex
5. Confirm Auto permits known-supported harnesses and leaves unknown harnesses off
6. Confirm Encouraged authorizes unknown support without relabeling it Supported
7. Stop one failed Fusion candidate while others continue
8. Detect waiting-for-input state and surface it clearly
9. Persist and reload a pre-policy session with artifacts intact
10. Exercise a legacy Swarm launch through its programmatic surface, not the primary builder

---

## Implementation Risks

### Known follow-up limitations

- `/cells` still exposes a singular workspace and cannot enumerate every
  isolated principal worktree after reload.
- A worker-queue claim that succeeds immediately before process creation fails
  can remain `Running` until stale-row reconciliation reclaims it.
- The legacy sequential Swarm/SharedCell worker journal still uses its original
  progression model; the new direct Hive path does not depend on that redesign.

| Risk | Mitigation |
|------|-----------|
| Rewriting all UI at once | Land domain/state first, then reshape screens |
| Resolver with weak inputs | Define artifact contract before building comparison UX |
| Runtime abstraction too generic too early | Design cleanly, implement only local runtimes for now |
| Capability mistaken for permission | Persist facts and operator policy separately; cover the full resolver matrix |
| Native child scope expansion | Inherit the Assignment Contract, carry child/depth values as prompt guidance, and rely on the native harness for hard enforcement |
| Swarm compatibility clutter | Keep programmatic backward compatibility; exclude it from the primary launch flow |

---

## Immediate Next Steps

1. Add `domain/` module — Session/Cell/Agent/Status/Event/Execution types
2. Add runtime trait, normalize existing CLI launchers behind it
3. Implement WorkspaceManager with cell-based worktree rules
4. Introduce event bus + structured event persistence
5. Extend HTTP API with cell/artifact/event endpoints
6. Refactor frontend stores around session/cell/agent hierarchy
7. Build minimal cell-first session overview screen
8. Add artifact bundle schema
9. Implement first Resolver flow for Fusion sessions
10. Add capability-aware Assignment Contract prompting for Queens and coding principals
