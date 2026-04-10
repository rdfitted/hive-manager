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
- SessionSidebar had the correct pattern; StatusPanel was inconsistent (fixed in PR review)

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
- Defense-in-depth: could also add validation in `SessionStorage::session_dir()`

### Security: CLI Allowlist Validation
- `validate_cli()` in `http/handlers/mod.rs` checks against `VALID_CLIS` allowlist
- `VALID_CLIS` must stay synchronized with `default_config()` in `storage/mod.rs`
- Applied to all handlers accepting CLI input (workers, planners)

### Security: Evaluator Authority Enforcement
- `evaluator_inject()` restricts targets to Queen and QA workers only — no planners, no other evaluators
- `queen_inject()` blocks targeting QA workers — enforces hierarchy boundary
- Use ID suffix patterns (`-evaluator`, `-qa-worker-`, `-queen`) for authority checks

### Prompt Template Structure in controller.rs
- Queen prompt (standard Hive): includes Learning Curation Protocol, tool table, sequential spawning
- Queen prompt (Swarm): similar but with planner-focused curation protocol
- Worker prompt: includes Learnings Protocol section with correct outcome values
- Tool files (`.hive-manager/{session_id}/tools/*.md`): generated by `write_tool_files()` method

### Tauri Command Pattern
- `resume_session` loads `PersistedSession` from storage, converts to active `Session`
- `PersistedAgentInfo` stores role as String (e.g., 'Queen', 'Worker(1)') requiring string parsing

### UI/UX Improvements
- Convert text inputs to dropdown selectors for constrained options (e.g., model selection in solo mode)
- Provide dynamic options based on selected parent option (e.g., model options change based on selected CLI)
- This prevents invalid selections and improves user experience

### Solo Mode as Zero-Worker Hive
- Frontend maps Solo mode to a Hive session with zero workers — backend detects this and spawns a single agent directly
- Avoids creating a separate session type flow end-to-end; reuses existing `launch_hive_v2` plumbing
- Dedicated `launch_solo()` in controller skips orchestration: no task files, no queen prompt, no watcher setup

### CLI-Specific Command Builders
- Each CLI (claude, gemini, codex, droid, cursor) has different prompt flags (`-p`, `-q`, positional)
- Dedicated solo command builder avoids coupling to orchestration-oriented defaults
- Model flag passthrough varies per CLI — must be handled per-type

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
- Hierarchy in `coordination/state.rs` must place new root agents with `parent_id: None`

### CLI Worker Reliability (Hive Sessions)
- **codex**: Performs extensive codebase indexing before producing output (8-12 min with no git diff is normal). Performs well once indexing completes. Occasional interactive approval stalls but rarer than indexing delays.
- **gemini**: Also indexes codebase before starting (~7-12 min). Good for frontend, data model changes. Reliable once indexing completes.
- **cursor**: Good for review/test tasks. WSL environment may lack Rust toolchain.
- **claude**: Most reliable for autonomous work. Starts producing output faster than codex/gemini. Best for complex multi-site refactors.
- **droid**: Fastest (~2min). Excellent for handler changes, validation, straightforward tasks. Minimal indexing overhead.
- **Strategy**: Wait at least **12-15 minutes** before declaring a worker stalled. Codex and Gemini index extensively — no git diff for 8-12 min is normal startup behavior. Only Droid and Claude start producing output quickly. Check terminal/PTY activity if possible, not just git diff. Only flag as truly stalled if zero terminal activity AND >15 min elapsed.

## Model Performance Notes

### Multi-Agent Verification
- 3 agents per concern with different models provides reliable consensus
- All 7 concerns in PR #19 were validated VALID by 3/3 agents (high confidence)
- Different models catch different aspects - useful for comprehensive analysis

## Hot Files
Files frequently modified across sessions — pay extra attention:
- `src-tauri/src/session/controller.rs` — 4200+ lines, prompt templates, state machine, match blocks for every enum. Modified in nearly every session.
- `src-tauri/src/storage/mod.rs` — Dual storage paths, role defaults, session persistence. Touched for any new role or config.
- `src-tauri/src/http/handlers/` — Each new feature adds or modifies handler files. Validation must be consistent.
- `src-tauri/src/coordination/injection.rs` — Authority enforcement, inject methods. Critical for security.
- `src/lib/stores/sessions.ts` — Frontend type unions must mirror Rust enums exactly.

---
*Curated from learnings.jsonl (23 entries) + 1 hive session payload*
*Last updated: 2026-03-30*
