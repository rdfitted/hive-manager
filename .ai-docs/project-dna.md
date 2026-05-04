# Project DNA

How we do things in this project. Updated by AI sessions.

## Patterns That Work

### Session-Scoped Storage with `_session` Suffix Methods
- Create new methods with `_session` suffix (e.g., `append_learning_session`) while keeping legacy methods for backward compatibility
- Store session data in `%APPDATA%/hive-manager/sessions/{session_id}/lessons/` not project root
- This provides true isolation between sessions and enables multi-project support

### Atomic File Writes with `tempfile` Crate
- Use `NamedTempFile::new_in(dir)` + `persist(target_path)` for atomic JSONL rewrites
- Benefits: unique filenames prevent races, auto-cleanup on drop if crash before persist
- Always create temp file in same directory as target to ensure same-filesystem rename

### Shared Validation/Filter Helpers
- Extract common logic into helper functions (e.g., `validate_session_id()`, `filter_learnings()`)
- Avoids duplicating filter closures across handlers
- Use `to_lowercase()` + `HashSet<String>` for O(1) case-insensitive keyword matching

### Deterministic IDs with UUID v5 Content Hashing
- When legacy entries lack an `id` field, use `Uuid::new_v5(NAMESPACE_DNS, content.as_bytes())` for stable IDs
- Pattern: `serde(default)` returns empty string sentinel, read paths detect empty and assign content-based hash
- Ensures same entry always gets same ID across reads (critical for delete-by-id)

### Multi-Agent PR Review Verification
- Spawn 3 verification agents per concern using different models for consensus-based triage
- Consolidate raw PR comments into distinct concerns before spawning agents (12 comments -> 7 concerns -> 21 agents instead of 36)
- Workflow: fetch comments -> group by concern -> spawn 3 agents -> categorize by consensus -> implement fixes -> commit
-> global: practices/multi-agent-workflows.md

### Session Defaults Propagation
- Store `default_cli` and `default_model` on PersistedSession/Session structs with `#[serde(default)]` for backward compat
- HTTP handlers read session defaults instead of hardcoding "claude"/"opus"
- Add `model` field to AddWorkerRequest to allow per-worker model overrides
- Reuse first session lookup result instead of re-acquiring locks for each handler that needs defaults

### Fusion Mode Coordination
- `StateManager` tracks per-variant state with JSON persistence (Serialize/Deserialize on all new types)
- `InjectionManager.judge_inject()` sends evaluation context to judge PTY including variant paths and evaluation report location
- `TaskFileWatcher` detects `fusion-variant-N-task.md` completion events, emits `fusion-variant-completed` and `all-variants-completed` events
- Pattern: extend existing coordination modules (state, injection, watcher) rather than creating new ones

### Evaluator Peer Architecture (PR #37)
- Evaluator is a root peer alongside Queen (parent_id: None), NOT a child agent
- QA workers are children of Evaluator, separate from Queen's worker hierarchy
- Bidirectional authority enforcement: Queen blocked from QA workers, Evaluator blocked from Queen workers
- Peer communication via atomic file writes in `.hive-manager/{id}/peer/` directory
- State machine: `MilestoneReady` -> `SpawningEvaluator` -> `QaInProgress` -> `QaPassed`/`QaFailed` -> `QaMaxRetriesExceeded`
- `is_active()` vs `is_monitorable()`: use `is_monitorable()` for heartbeat monitoring (excludes post-verdict states)
- Peer-state persistence must happen AFTER successful PTY delivery, not before (atomicity)

### Serde Enum Normalization in Frontend
- Rust Serde externally-tagged enums serialize as `{ "VariantName": data }` or `{ "VariantName": null }`, not plain strings
- Frontend must normalize with a helper like `serdeEnumVariantName(value)` that extracts the key from object variants
- Apply to ALL comparison sites: role checks, status badges, sidebar filters, tree rendering
- Easy to miss: `agent.status` comparisons break silently when they receive `{ "Running": null }` instead of `"Running"`

### Integration Tests Over Unit Tests
- Prefer integration tests that exercise the full HTTP handler stack (Axum router via `oneshot()`)
- Unit tests that only verify serde deserialization are redundant when integration tests cover the same validation end-to-end
- Integration tests catch more: routing, deserialization, validation, storage, response serialization

### Worktree-Scoped Worker Prompt Files (#108)
- Worker prompts must live INSIDE the worker worktree at `<worktree_root>/.hive-manager/prompts/<filename>`
- Queen, master-planner, fusion-queen run from project root — their prompts stay at `<project>/.hive-manager/{session_id}/prompts/`
- Helper: `write_worker_prompt_file(worktree_root, worker_index, filename, content)` mirrors `write_tool_files`
- All 8 worker spawn callsites in `controller.rs` MUST use the helper; non-worker callsites untouched
- Why: gemini sandboxes file-read tools to the worktree; placing prompts outside causes 15min stalls
-> global: practices/multi-agent-workflows.md

### WSL Path Conversion for Cursor CLI (#108 follow-up)
- `cursor` CLI runs via `wsl /root/.local/bin/agent`; Linux-side process cannot read Windows `D:\...` paths
- Helper: `to_wsl_path(path)` converts `D:\foo\bar` (or `D:/foo/bar`) → `/mnt/d/foo/bar`, lowercases drive letter
- Apply in `add_prompt_to_args` BEFORE building the prompt argument
- **Critical gotcha**: `build_command` maps `cli == "cursor"` to spawn name `"wsl"` BEFORE downstream helpers see it. Match `matches!(cli, "cursor" | "wsl")` so the converter actually fires at runtime
-> global: practices/multi-agent-workflows.md

### Learnings JSONL Fallback (`learnings.pending.jsonl`)
- Workers POST learnings to `/api/sessions/{id}/learnings`; on curl exit code 7 / non-zero, fall back to writing `.hive-manager/{session_id}/learnings.pending.jsonl`
- Workers MUST NEVER write directly to `.ai-docs/learnings.jsonl` — that's the consolidated repo store
- Queen consolidation Step 0.a: `mkdir -p .ai-docs && touch .ai-docs/learnings.jsonl` BEFORE the flush pipeline (otherwise `grep -Fxq` errors and drops records on first run in fresh repos)
- Queen ingest validates JSONL value types (non-empty strings, outcome enum, array shapes, no path traversal in `files_touched`) BEFORE both POST and dedup-append
- Append to root JSONL **unconditionally** after validation — POST success doesn't bypass repo preservation; on POST failure log a warning and still preserve

### Evaluator Config — Nested with Legacy Fallback (#106)
- Frontend now sends ONLY `evaluator_config: AgentConfig` (no legacy `evaluator_cli`/`evaluator_model` scalars)
- Backend handlers (`CreateSessionRequest`, `LaunchSwarmRequest`, `LaunchSoloRequest`) accept BOTH shapes:
  - Add `evaluator_config: Option<AgentConfig>` field
  - Centralized helper `evaluator_config_from_request(...)` prefers `evaluator_config`, falls back to legacy scalars, validates `cli` against allowlist
- Without the backend deserializer change, serde silently drops the new field → HTTP launches lose evaluator selection
- Tests: post the new shape AND assert legacy scalars are absent from the payload

### AgentConfigEditor Preset Selector (#109 follow-up)
- Default selection: `"Custom (keep current model)"` — does NOT auto-detect a matching preset
- Generic opus branch uses `model === 'opus'` (exact equality), NOT `model.includes('opus')` — broader match misclassifies versioned `claude-opus-4-6` etc.
- Versioned Claude opus checks (`claude-opus-4-6`, `claude-opus-4-5`) run BEFORE the generic `opus` branch
- Removing legacy `parseClaudeEffort`/`parseCodexEffort`/`detectPreset` helpers is fine once the dropdown defaults to `'custom'`

### Default Role CLI Assignments (Personal-App Preference)
- **Queen**: `claude` / `opus` (LaunchDialog `queenConfig` initial state)
- **Evaluator**: `claude` / `opus` (defaultRoles.evaluator)
- **Frontend**: `gemini` / `gemini-2.5-pro` (defaultRoles.frontend)
- **Everything else** (backend, coherence, simplify, reviewer, reviewer-quick, resolver, tester, code-quality, qa-worker, general): `codex` / `gpt-5.5`
- Defaults are mirrored in `src/lib/config/clis.ts` AND `src-tauri/src/storage/mod.rs::default_roles` — must stay in sync
- `test_default_role_models_match_frontend_defaults` enforces sync; update it when changing defaults
- Why: this is a personal app; the user manually overrode these every session before this change

### Heartbeat Snippet Centralization
- All prompt templates (worker, queen, tool files) use a single `templates::heartbeat_snippet()` helper
- Avoids drift when heartbeat payload shape evolves
- Pattern reusable for any cross-template snippet that's at risk of duplication

### Mtime + Cached-Hash Hot-Reload Dirty Check
- Use `session.json` mtime PLUS a cached "last-synced serialized snapshot" as the dirty check
- Hot-reload disk state into memory only when current in-memory persisted view still matches the last synced snapshot
- Prevents stale disk writes from clobbering newer in-memory state while still reconciling clean external updates
- Re-verify cleanliness under the write lock before applying refreshed storage data

### `spawn_blocking` for Sync git/FS in Async Handlers
- Push synchronous git HEAD resolution and peer-file writes onto `tokio::task::spawn_blocking` to keep async runtime threads free
- Especially critical for evaluator/QA handlers that resolve commit SHAs

### Stable Worker Base SHA + Atomic Verdict Persistence
- Persist a worker branch's creation-time base SHA on the agent — avoids false-positive commit-gating when project HEAD moves later
- The same controller mutation can atomically persist evaluator `commit_sha` with a QA verdict, eliminating follow-up writes and string-based error classification
- Use `CompletionBlockedError` struct for structured 409 responses (`current_state`, `unblock_paths`, `remaining_quiescence_seconds`) — reusable for any blocked-state guidance

### Reconciliation Mirror Pattern in Worktrees
- Shared session artifacts under `<project>/.hive-manager/{session_id}/...` sit OUTSIDE worker worktree git toplevels
- When a task requires both a tracked deliverable AND a shared handoff file: write the file inside the worktree (tracked) AND copy it to the shared session path (consumed by Queen/peers)
- Avoids "file not in this repo" git errors while preserving cross-agent handoff

### Master Planner Scout Choices (Personal Setup)
- Hive + Swarm Master Planner spawn 3 codex scouts via Task tool: `gpt-5.5` low / low / medium reasoning effort
- Pattern unchanged: `Task(subagent_type="general-purpose", prompt="...IMMEDIATELY run: codex exec --dangerously-bypass-approvals-and-sandbox -m gpt-5.5 -c model_reasoning_effort=\"<level>\" '...'")`
- Synthesis remains inline in the planner (no separate synthesis agent)
- Fusion master-planner has no scouts (writes plan directly)

## Patterns That Failed

### Random UUIDs for Serde Defaults
- `serde(default = "generate_random_uuid")` generates different IDs on every deserialization
- Makes delete-by-id unreliable for entries missing the `id` field
- Fix: use deterministic content-based hashing (UUID v5) instead

### Inconsistent Validation Across Handlers
- `resume_session()` in controller.rs had path traversal validation but HTTP learnings handlers didn't
- Validation must be applied consistently at every entry point, not just some
- Consider centralized middleware/extractors for common validation

### Documentation Drift from Code
- Tool documentation strings embedded in prompt templates easily drift from handler validation
- submit-learning.md documented wrong outcome values, causing workers to get 400 errors
- Always update embedded docs when changing validation rules

### Reactive Store Race Conditions in Svelte Modals
- Confirmation modals for destructive actions must capture the target entity ID when opened, not when confirmed
- Svelte reactive stores (`$activeSession`) can change between modal open and confirm
- Never read store at confirm time for the action target — capture into a local variable at open time

### Defaults Sync Sites Multiplied Silently
- Model and role defaults are mirrored in FIVE places: `clis.ts`, `AgentConfigEditor.svelte` model selectors, `LaunchDialog.svelte` initial configs, backend `cli/registry.rs`, backend `storage/mod.rs`
- Updating only the shared map is insufficient if launch UI initializers still hardcode an older CLI
- Mitigation: rely on `defaultRoles` map in `LaunchDialog` (`createDefaultConfig`) instead of string literals; backend test enforces frontend↔backend parity

### `add_prompt_to_args` Ran on Spawn-Command Name, Not CLI Identifier
- `build_command` mutates `"cursor"` → `"wsl"` BEFORE downstream callers invoke `add_prompt_to_args(&cmd, ...)`
- A first-pass cursor fix that only matched `cli == "cursor"` never fired at runtime
- Generic lesson: when a name-mapping function happens upstream, downstream branches on the original identifier are dead code. Either match BOTH the source and target identifiers (`matches!(cli, "cursor" | "wsl")`) or thread the original identifier through.

### Active Sessions Payload Shape Misparse
- `GET /api/sessions/active` returns `{ sessions: [...] }` with `ActiveAgentInfo { id, last_activity }`
- Frontend store was parsing as array | object and reading `timestamp`/`last_update` — both wrong
- Treat missing `last_activity` as not-yet-heartbeated, NOT stale, to avoid false QA-stale badges and cross-session leakage

## Code Conventions

### HTTP Handler Structure
- Validate inputs at top of handler (path params first, then body fields)
- Use `ApiError::bad_request()` / `ApiError::not_found()` / `ApiError::internal()` for errors
- Return `(StatusCode::CREATED, Json(...))` for POST, `StatusCode::NO_CONTENT` for DELETE
- Session-scoped handlers: always call `validate_session_id()` first

### Storage Layer
- JSONL format for append-friendly data (learnings)
- Markdown for curated content (project-dna.md)
- Legacy methods marked `DEPRECATED: Use *_session for new code`
- `tempfile` crate for atomic rewrites, `fs::OpenOptions::append` for appends

### Prompt Template Escaping
- `{var}` in format strings must be escaped as `{{var}}` to produce literal braces
- Template placeholders use `{{session_id}}` (double braces) for Mustache-style interpolation
- `{id}` in raw string literals passed to `format!()` will be interpreted as format args

### Dependency Management
- `tempfile` in `[dependencies]` (not just `[dev-dependencies]`) when used in production code
- `uuid` features: `v4` for random generation, `v5` for deterministic content-based hashing

### Test Rigor for Launch Endpoints
- Launch tests that depend on PTY/worktree setup MUST tolerate `INTERNAL_SERVER_ERROR` (env can fail mid-spawn)
- Pattern: `assert!(status == CREATED || status == INTERNAL_SERVER_ERROR)`, then gate evaluator/session assertions and `close_session` behind `if status == CREATED { ... }`
- Don't hard-require 201 in tests where the failure mode is environmental rather than logical

## Architecture Notes

### Dual Storage Paths
- **Legacy (project-scoped)**: `.ai-docs/` in project root - deprecated but preserved
- **Session-scoped**: `%APPDATA%/hive-manager/sessions/{session_id}/lessons/` - current standard
- Both paths have corresponding HTTP endpoints (`/api/learnings` vs `/api/sessions/{id}/learnings`)
- Legacy endpoints infer project path from active sessions; error if multiple projects active

### Security: Path Traversal Defense
- `validate_session_id()` rejects `..`, `/`, `\` characters
- Applied at HTTP handler layer (not storage layer) for all session-scoped endpoints
- `files_touched` body field also validated for traversal characters

### Security: CLI Allowlist Validation
- `validate_cli()` in `http/handlers/mod.rs` checks against `VALID_CLIS` allowlist
- `VALID_CLIS` must stay synchronized with `default_config()` in `storage/mod.rs`

### Security: Evaluator Authority Enforcement
- `evaluator_inject()` restricts targets to Queen and QA workers only — no planners, no other evaluators
- `queen_inject()` blocks targeting QA workers — enforces hierarchy boundary
- Use ID suffix patterns (`-evaluator`, `-qa-worker-`, `-queen`) for authority checks

### Prompt Template Structure in controller.rs
- Queen prompt (standard Hive): includes Learning Curation Protocol, tool table, sequential spawning
- Queen prompt (Swarm): similar but with planner-focused curation protocol
- Worker prompt: includes Learnings Protocol section with correct outcome values + File-Based Fallback block
- Tool files (`.hive-manager/{session_id}/tools/*.md`): generated by `write_tool_files()` method

### Tauri Command Pattern
- `resume_session` loads `PersistedSession` from storage, converts to active `Session`
- `PersistedAgentInfo` stores role as String (e.g., 'Queen', 'Worker(1)') requiring string parsing
- Tauri icon CLI regenerates ALL platform icons including new android/ios directories — commit them all together

### UI/UX Improvements
- Convert text inputs to dropdown selectors for constrained options
- Provide dynamic options based on selected parent option
- Default preset selectors to "Custom (keep current)" so opening an editor never silently overwrites config

### Solo Mode as Zero-Worker Hive
- Frontend maps Solo mode to a Hive session with zero workers — backend detects this and spawns a single agent directly
- Avoids creating a separate session type flow end-to-end; reuses existing `launch_hive_v2` plumbing
- Dedicated `launch_solo()` in controller skips orchestration: no task files, no queen prompt, no watcher setup
- Solo mode supports evaluator: `SoloLaunchConfig` carries `evaluator_config` and the LaunchDialog exposes the Evaluator Peer toggle for Solo too

### CLI-Specific Command Builders
- Each CLI (claude, gemini, codex, droid, cursor) has different prompt flags (`-p`, `-q`, positional)
- Dedicated solo command builder avoids coupling to orchestration-oriented defaults
- Model flag passthrough varies per CLI — must be handled per-type
- `cursor` is special: `build_command` maps it to `wsl` spawn — downstream helpers must recognize both identifiers

### SessionType Variant Synchronization
- Adding a new `SessionType` variant requires synchronized updates in:
  - `resume_session()` — deserialization/restore path
  - `session_to_persisted()` — persistence path
  - All `SessionType`/`SessionTypeInfo` match arms across controller and storage
- Missing any one causes deserialization mismatches or non-exhaustive match errors

### AgentRole Enum Expansion Protocol
- Adding a new `AgentRole` variant touches 35+ match sites across controller, handlers, commands, coordination
- Rust compiler catches all exhaustive match failures — fix ALL before merging
- Frontend `AgentRole` type must exactly mirror Rust enum serialization shape (object variants, not strings)
- Filters in workers/planners handlers must explicitly exclude new roles from listings

### CLI Worker Reliability (Hive Sessions)
- **codex**: Performs extensive codebase indexing before producing output (8-12 min with no git diff is normal). Performs well once indexing completes. Occasional interactive approval stalls but rarer than indexing delays.
- **gemini**: Also indexes codebase before starting (~7-12 min). Good for frontend, data model changes. Reliable once indexing completes.
- **cursor**: Good for review/test tasks. WSL environment may lack Rust toolchain. Prompt paths must be WSL-converted.
- **claude**: Most reliable for autonomous work. Starts producing output faster than codex/gemini. Best for complex multi-site refactors.
- **droid**: Fastest (~2min). Excellent for handler changes, validation, straightforward tasks. Minimal indexing overhead.
- **Strategy**: Wait at least **12-15 minutes** before declaring a worker stalled. Codex and Gemini index extensively. Only Droid and Claude start producing output quickly.

## Hot Files
Files frequently modified across sessions — pay extra attention:
- `src-tauri/src/session/controller.rs` — 9000+ lines, prompt templates, state machine, match blocks for every enum, `write_worker_prompt_file`/`add_prompt_to_args`/`to_wsl_path`. Modified in nearly every session.
- `src-tauri/src/storage/mod.rs` — Dual storage paths, role defaults, session persistence. `default_roles` MUST stay in sync with `src/lib/config/clis.ts`.
- `src-tauri/src/http/handlers/sessions.rs` — Launch handler request structs, evaluator_config_from_request helper. Touched whenever launch contract changes.
- `src-tauri/src/http/handlers/` — Each new feature adds or modifies handler files. Validation must be consistent.
- `src-tauri/src/coordination/injection.rs` — Authority enforcement, inject methods. Critical for security.
- `src-tauri/src/templates/mod.rs` — heartbeat_snippet() helper; test module imports must include `TemplateError` for the invalid-render test.
- `src/lib/config/clis.ts` — Frontend source of truth for cliOptions + defaultRoles. Must mirror backend `default_roles`.
- `src/lib/components/AgentConfigEditor.svelte` — Preset selector defaults to "Custom"; opus branch uses exact equality.
- `src/lib/components/LaunchDialog.svelte` — Uses `createDefaultConfig` reading from `defaultRoles`; evaluatorConfig pulls from `defaultRoles.evaluator`.
- `src/lib/stores/sessions.ts` — Frontend type unions must mirror Rust enums exactly.

## Keywords → Files Mapping
- **launch / evaluator_config**: `LaunchDialog.svelte`, `AgentConfigEditor.svelte`, `http/handlers/sessions.rs`, `clis.ts`
- **prompt files / worktree**: `controller.rs::write_worker_prompt_file`, `controller.rs::add_prompt_to_args`, `controller.rs::to_wsl_path`
- **learnings**: `http/handlers/learnings.rs`, `controller.rs` Queen consolidation block, `.ai-docs/learnings.jsonl`, `.hive-manager/{id}/learnings.pending.jsonl`
- **defaults / role config**: `clis.ts`, `storage/mod.rs::default_roles`, `LaunchDialog.svelte::createDefaultConfig`
- **heartbeat**: `templates/mod.rs::heartbeat_snippet`, controller prompt strings
- **QA verdict**: `http/handlers/evaluator.rs`, `coordination/state.rs`, `session/cell_status.rs`, `peer/qa-verdict.json`

---
*Curated from 50 entries (27 newly synthesized) + earlier hive sessions*
*Last curated: 2026-05-04*
*Archive: `.ai-docs/archive/learnings-{ts}.jsonl`*
