# Meta-Harness Modernization

## Overview

Modernize Hive Manager for delegation-capable coding harnesses without replacing its durable operator, workspace, queue, artifact, and review infrastructure.

The target operating model is:

- an operator-selected Queen (Opus by default) owns intent, topology, assignment, synthesis, integration, and final judgment;
- a small operator-selected set of visible coding principals (Codex GPT-5.6 / Sol by default) own coherent implementation workstreams;
- capable principals may create harness-native children inside their assignment and workspace;
- managed principals and native children are different execution planes;
- worktree topology is selected independently from delegation topology;
- Hive is the configurable swarm container, so Swarm is removed from new-session UI while legacy Swarm sessions and APIs remain compatible;
- Master Planner remains a first-class phase, but emits a topology/ownership/validation contract instead of fixed GPT-5.5 scout scripts and one task per predeclared worker.

This work extends the current controller incrementally. It does not attempt to replace the 14k-line controller or migrate every active session to the newer domain model in this PR.

## Requirements

### Operator control

- Keep per-agent CLI/model selection authoritative.
- Default the Queen to Claude Opus and coding principals to Codex GPT-5.6.
- Add Fable 5 and GPT-5.6 model/effort presets while retaining older presets.
- Let the operator choose native-delegation guidance separately for the Queen and coding principals.
- Let the operator choose one shared Hive worktree or isolated worktrees per visible principal.
- Default a new Hive to a small principal roster, not the historical six-role worker swarm.
- Keep evaluator/QA configuration available without making it part of the primary topology mental model.

### Execution planes

- A managed principal is a Hive Manager PTY process with durable session state, queue identity, terminal, events, and workspace attachment.
- A native child is created inside Claude Code or Codex, inherits its parent principal's assignment/workspace, and is not a Cell, queue row, or managed terminal.
- Native delegation must never widen a principal's file ownership or git authority.
- UI copy and prompts must not call managed Workers and native children the same thing.

### Capability-aware prompts

- Resolve a launch-time Capability Card from CLI, canonical model ID, role, and operator policy.
- Preserve `unknown` as a real capability result; do not claim tools merely from a nickname.
- Use concise prompt layers: role kernel, capability card, assignment contract, required gates, and references to generated runbooks.
- Remove unconditional claims that every CLI has Claude Code tools.
- Remove hardcoded external `codex exec -m gpt-5.5` scout recipes from current Hive bootstraps; retain the legacy Swarm prompt for compatibility.
- Explicitly authorize Sol delegation when the operator selects the encouraged policy; modern Codex is otherwise conservative.
- Keep detailed HTTP, polling, fallback, learning, and QA procedures in generated tool/runbook files and load them on demand.

### Compatibility

- Preserve `SessionType::Swarm`, `SessionTypeInfo::Swarm`, Swarm persistence, list/get/display, resume, pending planning continuation, action, Tauri command, and HTTP route.
- Remove Swarm only from the primary new-session builder/event chain and mark remaining launch methods as legacy in code/docs.
- Missing new policy fields must deserialize safely and preserve legacy isolated-worker behavior.
- Preserve explicit Solo semantics. `workers: []` is currently the implicit Solo sentinel, so new Hive launches must carry an explicit launch kind before a zero-principal Hive can be represented.
- Preserve Research as a Hive profile and its read-only/no-git guarantees.

## Architecture

### Launch policy

Add backward-compatible domain types for:

- `HiveLaunchKind`: `auto` (legacy sentinel behavior), `hive`, `solo`;
- `NativeDelegationMode`: `disabled`, `auto`, `encouraged`;
- `DelegationPolicy`: mode plus optional child/depth guidance;
- `HiveExecutionPolicy`: launch kind, workspace strategy, Queen delegation, principal delegation;
- `CapabilityCard`: resolved harness/model/role support, allowed delegation, limits, workspace inheritance, and inference source.

`HiveExecutionPolicy::default()` is the legacy-compatible policy: `auto` launch kind and isolated-cell workspaces. The frontend sends an explicit recommended policy for every new Hive.

Persist the policy on the active and persisted session with serde defaults. Existing session JSON therefore remains readable and resumed sessions retain explicit operator choices.

### Topology planner

Replace `orchestrator/session_orchestrator.rs`'s placeholder with a small pure topology planner consumed by the existing controller. It determines:

- one primary Hive Cell;
- shared-cell versus per-principal worktree allocation;
- stable branch/cell names;
- Queen and managed-principal workspace attachment;
- native-child workspace inheritance.

The controller continues to own PTY, git, task-file, watcher, event, and rollback mechanics.

### Workspace behavior

- `shared_cell`: create one managed primary worktree/branch and run Queen plus all managed principals there;
- `isolated_cell`: retain the existing Queen/principal worktree behavior for compatibility and operator-selected isolation;
- Fusion remains isolated by candidate Hive;
- Research remains no-git/current-project and read-only.

Planning and non-planning Hive launches must converge on the same workspace policy. Dynamic managed-principal spawning must reuse the primary workspace in shared-cell mode and must not delete it during worker-launch rollback.

### Prompt compiler

Use the capability resolver from all live Hive bootstrap builders:

1. Master Planner
2. Queen
3. managed coding principal

Each bootstrap contains:

1. Role Kernel
2. Capability Card
3. Assignment Contract
4. Session/topology facts
5. Required Gates
6. Protocol References
7. Objective/current task

The Master Planner may use bounded native read-only scouts when supported. It produces workstreams, file ownership, serialized hotspots, phase gates, validation, and stop conditions. It does not force a fixed number of scouts or exactly one task per roster slot.

The Queen may use native children for bounded planning/review work but delegates implementation to coding principals. A coding principal may use native children inside its workstream when policy permits.

### Frontend model

The primary Hive builder shows:

- Queen configuration;
- one or more visible coding principals (one Sol principal by default, additional principals optional);
- native delegation guidance for Queen and principals;
- shared versus isolated worktree topology;
- planning and verification choices.

The fixed backend/frontend/coherence/simplify/reviewer/resolver roster is no longer the default. Role strings remain backward compatible; new visible implementers use a `principal` WorkerRole without adding an `AgentRole` enum variant.

Swarm is removed from `LaunchDialog`, `SessionSidebar`, and page launch events. Legacy Swarm types/store methods/tests and session badges remain.

## Implementation Steps

1. Add execution-policy and capability types, serde defaults, capability inference, and pure topology planning with unit tests.
2. Persist execution policy through `HiveLaunchConfig`, active `Session`, `PersistedSession`, restore, and storage conversion paths.
3. Normalize Hive launch behavior across planning, non-planning, and dynamic-principal spawn paths; implement shared primary worktree allocation and safe rollback.
4. Update Cells/workspace projection to expose the real session workspace strategy/path for new sessions while retaining synthetic fallback for legacy data.
5. Replace live Master Planner, Queen, and principal bootstraps with capability-aware layered prompts; keep mandatory QA/authority gates inline and reference generated runbooks for mechanics.
6. Update CLI/model defaults in lockstep: frontend config, model editor, launch defaults, backend registry, backend storage, built-in templates, and associated tests.
7. Rework the Hive launch UI around coding principals and policy controls; add stable form IDs, visible effective model/effort, exhaustive mode submission, and template workspace propagation.
8. Remove Swarm from the new-session UI/event chain while preserving legacy launch/store/backend compatibility.
9. Update vNext PRD/implementation docs, README terminology, queue comments, and project DNA to describe Hive Manager as an operator-controlled meta-harness with macro and micro execution planes.
10. Review the combined diff for ownership, legacy compatibility, prompt size, false UI controls, and shared-worktree git hazards.

## Testing Strategy

### Rust

- Capability inference: Claude/Opus, Claude/Fable, Codex/GPT-5.6, older supported models, explicitly unsupported cards, and unknown/unprofiled CLIs.
- Policy serde: explicit recommended policy round-trip and legacy JSON without policy.
- Topology planner: shared primary Cell, isolated principals, and native-child parent-workspace inheritance.
- Prompt rendering: actual CLI/model in cards; no unconditional Claude-tool claims; no fixed GPT-5.5 scout commands; workstream-driven Master Planner; delegation only when allowed; research remains read-only.
- Controller: planning/non-planning shared worktree parity; dynamic principal reuses shared CWD; isolated mode still creates per-principal worktrees; rollback never removes a shared primary worktree.
- Persistence: old Swarm and Hive sessions deserialize; execution policy survives resume.
- HTTP/action: Hive launch policy passes through; Cells expose real workspace metadata; legacy Swarm route remains compatible.

### Frontend

- Model presets include Fable 5 and GPT-5.6 and show the effective selection.
- New Hive payload includes explicit `launch_kind: hive`, workspace strategy, and delegation policies.
- Solo payload remains explicit and cannot be confused with a zero-principal Hive.
- Swarm is absent from the primary launch builder but legacy Swarm store behavior remains tested.
- Templates propagate workspace strategy and cannot submit the bare Templates state as Fusion.
- Repeated agent editors have unique label targets; policy controls expose labels/descriptions and preview state.

### Commands

- `cargo check --tests`
- targeted `cargo test` for capability, topology, controller prompt, persistence, and HTTP modules
- full `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings` if the branch baseline permits; otherwise report pre-existing findings separately
- `npm test`
- `npm run check`
- `npm run build`

### Manual smoke

- Opus Queen + GPT-5.6 Sol principal, planning enabled, shared worktree, encouraged native delegation.
- Same topology with per-principal isolation.
- Operator-disabled delegation.
- Managed principal added after launch.
- Legacy Swarm session list/get/resume and legacy launch endpoint.

## Risks and Considerations

- Shared worktrees make concurrent writes real. Prompts must assign non-overlapping paths, serialize shared hotspots, and reserve branch/commit/push/reset/stash authority for the Queen.
- Existing artifact harvesting assumes per-worker worktrees and may duplicate a shared-cell diff. Treat shared worktree artifacts as cell-level refreshes, not per-worker merges.
- Planning currently runs from the project checkout while other paths allocate worktrees differently. Do not fix only the no-planning path.
- Worker rollback currently deletes `worker-N` worktrees unconditionally. Shared workspace reuse needs an explicit ownership flag.
- The controller duplicates adapter command logic. This PR adds capability inference at the registry/domain boundary but does not attempt a full adapter migration.
- Model aliases are launch IDs; display labels such as Sol must not be persisted as model IDs.
- Do not run repo-wide `cargo fmt`; project DNA records unrelated formatting churn from that command.
- The open PR already contains substantial QC work. Preserve its untracked `.claude/epic-123-128/` and `.codex-review/` directories and keep all commits on the existing PR branch.

## Success Criteria

- A new Hive defaults to Opus Queen + GPT-5.6 Sol coding principal(s), not a six-worker pseudo-swarm.
- The operator can independently choose models, visible-principal count, native delegation, worktree topology, planning, and verification.
- Master Planner and live non-legacy Hive prompts describe only resolved capabilities and no longer hardcode GPT-5.5 external scout swarms or generic Claude tools; the legacy Swarm prompt remains compatibility-only.
- Shared and isolated Hive worktree choices both function across planning, continuation, and dynamic managed-principal spawn.
- Swarm is no longer a new-session choice, while old Swarm sessions remain readable/resumable and legacy programmatic callers remain functional.
- Existing tests pass, new contract/topology/prompt tests pass, and no unrelated user files are modified.
