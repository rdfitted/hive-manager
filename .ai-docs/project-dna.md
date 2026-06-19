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

### Peer-CLI Architecture (vs Aliasing)
- When deprecating one CLI in favor of another, prefer **two adapters as peers** over **one adapter with an alias**
- Both adapters live in `adapters/` as separate `.rs` files, both in `VALID_CLIS`, both in `get_adapter()` factory, both with their own preset dropdowns in `AgentConfigEditor.svelte`
- Choose between them via `default_roles` (data-driven), not via runtime aliasing
- Why: an alias (e.g., `get_adapter("gemini") -> AntigravityAdapter`) masks the original adapter when you actually want both available
- Example: `gemini` and `antigravity` (`agy`) are peer CLIs in this codebase; `default_roles.frontend.cli` selects which is default
- Generalizable rule: when a deprecated thing is being replaced but the replacement isn't yet 100% ready, keep both as peers with the working one as the default — don't alias them
- See also: PR #116 walked this back from #112's alias approach
- -> global: [patterns/cli-adapter-pattern.md](../../.ai-docs/wiki/patterns/cli-adapter-pattern.md)

### Name-vs-Executable Routing in Adapters
- When `build_command` remaps a CLI name to a different executable (e.g., `cli == "antigravity"` → `cmd == "agy"`, `cli == "cursor"` → `cmd == "wsl"`), every downstream function that receives `cmd` must also match on the **executable name**, not just the CLI name
- The `cursor` / `wsl` pair in `add_prompt_to_args` was the right precedent; the `antigravity` / `agy` pair was missed in PR #112, causing the bug at the heart of issues #115 / #116
- Mechanical check: any match block on `cli: &str` that follows a `build_command` call should accept BOTH the cli-name AND the remapped-binary-name as match arms
- Symptom when missed: the function falls through to `_ => positional` and produces a syntactically-valid-but-semantically-wrong command (CLI ignores the flag)
- -> global: [patterns/cli-adapter-pattern.md](../../.ai-docs/wiki/patterns/cli-adapter-pattern.md)

### CLI Migration End-to-End Smoke Test Gate
- Any change touching the prompt-injection path (`add_prompt_to_args`, `add_inline_task_to_args`, adapter `build_launch_command`) **must** include a manual end-to-end smoke test step in acceptance criteria — not just unit tests that assert the flag is in the args
- Unit tests assert `args.contains("-i")` — necessary but insufficient. They don't catch "the binary ignores the flag's content for execution" or "the binary never receives the flag at all because routing fell through"
- The cost of skipping E2E: PR #112 shipped (0.29.0), was installed, and the bug only surfaced when a real worker spawned
- Acceptance criteria template for prompt-injection changes:
  1. `cargo check --tests` clean
  2. `npm run check` baseline unchanged
  3. **Manual: spawn a real frontend worker → confirm PTY shows the expected command line → confirm the worker actually executes the task file (not just launches)**
- See also: PR #116 acceptance criteria explicitly required the manual smoke as the FIRST item
- -> global: [practices/cli-migration-checklist.md](../../.ai-docs/wiki/practices/cli-migration-checklist.md)

### Belt-and-Suspenders Backward Compat for CLI Renames
- When renaming a CLI in code (e.g., persisted `cli: "gemini"` should become `cli: "antigravity"`), use BOTH a runtime alias AND a load-time rewrite
- Runtime alias: `get_adapter("gemini") -> AntigravityAdapter` — catches paths that bypass storage normalization (test fixtures, hand-crafted state)
- Load-time rewrite: `normalize_legacy_cli_names()` runs on every `load_session()` / `load_config()` — cleans up persisted state lazily as it's read
- Single-pronged defense leaves a quiet failure mode — either approach alone is incomplete
- Note: this pattern only applies during a transition window; once the rename is permanent and all callsites are migrated, the alias and the helpers can be removed (as PR #116 did with the gemini→antigravity alias)
- -> global: [patterns/storage-backward-compat.md](../../.ai-docs/wiki/patterns/storage-backward-compat.md)

### Default-Mirroring Across 5 Files
- Default role configurations (which CLI/model for backend, frontend, evaluator, etc.) are mirrored across **5 files** that must stay in lockstep:
  1. `src/lib/config/clis.ts` (`defaultRoles` map + `cliOptions[].defaultModel`)
  2. `src/lib/components/AgentConfigEditor.svelte` (`handleCliChange` model-on-CLI-switch + preset arrays)
  3. `src/lib/components/LaunchDialog.svelte` (initial role configs)
  4. `src-tauri/src/cli/registry.rs` (`default_model()` + `get_behavior()`)
  5. `src-tauri/src/storage/mod.rs::default_config()` (`default_roles` + `CliConfig` entries)
- LaunchDialog initializers using string literals silently override shared defaults — always source from `defaultRoles`
- Lockstep enforcement: `test_default_role_models_match_frontend_defaults` (storage + integration test) + `test_add_worker_accepts_<cli>` (VALID_CLIS lockstep between `adapters/mod.rs` and `http/handlers/mod.rs`)
- When changing a default, grep all 5 files; missing one creates UX drift that's invisible until a worker is spawned via the legacy path
- Frontend preset dropdown intentionally defaults to "Custom (keep current model)" — opening the editor never silently overwrites a configured model

### Hot-Reload with mtime + Content Hash Dirty Check
- For session hot-reload: combine `session.json` mtime precheck with a cached persisted-session hash to avoid repeated full-JSON dirty checks
- The controller re-verifies cleanliness under the write lock before applying refreshed storage data — prevents stale disk writes from clobbering newer in-memory state
- Pattern: only hot-reload disk → memory when the current in-memory persisted view still matches the last synced snapshot
- For async session/evaluator handlers: push synchronous git HEAD resolution and peer-file writes onto `spawn_blocking` to keep runtime threads free

### Worker-Worktree Prompt File Routing
- Worker prompt files for worktree-bound agents live under `<worktree>/.hive-manager/prompts/`
- Queen / planner / evaluator prompts (which run from project root) live under `<project_root>/.hive-manager/{session_id}/prompts/`
- Post-launch QA workers should keep project-root prompt files when their PTY cwd is the project root rather than an isolated worktree
- In isolated worker git worktrees, shared session artifacts under `<project_root>/.hive-manager/{session_id}/...` can sit outside the worktree git toplevel — when a task requires both a shared handoff file AND a branch commit, mirror the tracked artifact inside the worktree and copy it to the shared session path for direct consumption

### Cursor CLI via WSL — Path Conversion
- Cursor runs prompt-file orchestration inside WSL, so Windows absolute prompt paths (`D:\repo\...`) must be converted to `/mnt/d/repo/...` before being passed as positional prompts
- `to_wsl_path()` helper in `controller.rs` handles drive-letter and forward-slash variants
- `build_command` remaps `cli == "cursor"` to spawn command `wsl`, so downstream prompt-argument helpers receive the spawn command name — path-conversion logic must match on **both** `cursor` AND `wsl` identifiers
- This is the precedent that the antigravity/agy pair should have followed (see Name-vs-Executable Routing above)

### Stable Worker Base SHA + Atomic QA Verdict Persistence
- Persist a worker branch's **creation-time base SHA** on the agent — avoids false-positive commit gating when project HEAD moves later
- The same controller mutation can atomically persist evaluator `commit_sha` alongside a QA verdict, so handlers don't need follow-up writes or string-based error classification
- Pattern: typed completion-blocked errors (`CompletionBlockedError` struct with `current_state`, `unblock_paths`, `remaining_quiescence_seconds`) are reusable for any blocked-state HTTP 409 response

### QA Heartbeat / Staleness API Contract
- `GET /api/sessions/active` returns `{ sessions: [...] }` with `ActiveAgentInfo` fields `{ id, last_activity }` — **NOT** an array, **NOT** `timestamp`/`last_update`
- Frontend store must: parse `data.sessions` → find by id (fallback to first if only one) → read `agent.last_activity` only
- Blank or invalid `last_activity` values should be treated as **not-yet-heartbeated**, NOT stale (otherwise false stale badges on fresh agents)
- Heartbeat UI state should only populate from an **exact session match** — guards against cross-session leakage when switching sessions
- Treating dead evaluators as stale IDs inside `launch_evaluator` is enough to support safe respawn without changing watcher semantics
- 3-min staleness threshold computed client-side from timestamp; no new polling endpoint needed

### Heartbeat Snippet Centralization
- Heartbeat command snippets duplicated across prompt templates (Queen / Worker / tool-files) drift quickly as heartbeat payloads evolve
- Route all heartbeat instructions through a single `templates::heartbeat_snippet()` helper
- General lesson: any string that must match a regex / parser across N templates should be generated, not copy-pasted

### Evaluator Config Migration (Scalar → Nested Object)
- When migrating launch payloads from legacy scalar fields (`evaluator_cli`, `evaluator_model`) to nested objects (`evaluator_config: { cli, model, ... }`):
  - Every backend entrypoint must deserialize AND prefer the new object — otherwise serde silently ignores it and HTTP launches lose evaluator selection
  - Reuse the existing `AgentConfig` shape for `evaluator_config` (don't define a parallel type)
  - Centralize legacy scalar fallback in one helper (`coerce_legacy_evaluator_fields()`)
  - Test that legacy scalar fields are **absent** from the payload when `evaluator_config` is provided (assertion of NOT-present catches silent fallbacks)
- Frontend declarations and tests for the legacy fields are dead once the launch dialog emits `evaluator_config` — clean them up

### Bot Reviewer Effectiveness (CodeRabbit + gemini-code-assist)
- The 10-minute monitor window after PR push is well-spent — bots can surface 1-2 high-quality cross-file issues within ~5 minutes of push
- gemini-code-assist + CodeRabbit catch real cross-file inconsistencies that human reviewers easily miss (require diffing N files mentally)
- Examples from PR #112 / #116: caught the `-i` vs `-p` inline_task inconsistency, asymmetric model-clearing between two normalize helpers, default model drift across 5 files, and the name-vs-executable routing bug
- Monitor must poll BOTH `/pulls/N/comments` (inline review comments) AND `/issues/N/comments` (general PR discussion) — they're separate API endpoints
- Addressing bot findings in the same session (rather than after merge) keeps the PR coherent

### Externally-Probe-Before-Trust for CLI Migrations
- When migrating to a new CLI, **probe the installed binary directly** before trusting third-party blogs / migration guides
- Concrete sequence: `cli --help`, `cli --invented-flag` (to confirm it errors), check actual filesystem layout (`~/.config/<cli>/`, `~/.<cli>/`, `%LOCALAPPDATA%`), inspect existing settings/config files
- One blog about `agy` claimed `--model gemini-3-pro` and `~/.config/antigravity/config.yaml` — both wrong (probe revealed no `--model` flag, config lives at `~/.gemini/antigravity-cli/settings.json`)
- Documentation often lags releases; the binary is the source of truth
- -> global: [practices/cli-migration-checklist.md](../../.ai-docs/wiki/practices/cli-migration-checklist.md)

### Solo Launch Payload Mirroring
- Solo mode and Hive mode share the launch dialog UI but submit different payload shapes
- Solo mode was historically missing `evaluator_config` fields + the UI selector was hidden — fixed by updating `SoloLaunchConfig` to the same shape as `HiveLaunchConfig` and unhiding the selector
- General principle: when two modes share a UI, prefer one payload shape with mode-discriminator rather than two divergent shapes

### Debate Mode as a Fusion-Topology Profile (#120)
- New session modes that need N-parallel-agents-plus-a-judge can clone Fusion's isolated-cell topology rather than inventing new orchestration: debaters spawn as Fusion variants under the hood, mapped to their own worktrees for side-by-side terminal views.
- The genuinely new part is **multi-round coordination of non-code artifacts**: round state lives in session-scoped `.hive-manager/{session_id}/debate/rounds` files, and the `TaskFileWatcher` needs a **distinct debate task-file pattern** (e.g. `debate-round-N-debater-M`) to advance rounds reliably — reusing the fusion-variant pattern silently breaks round progression.
- The judge reuses the prompt-driven `queen-research` wiki load-in + Draft→PR capture (branch `debate/<topic-slug>`, base `main`, graceful no-op if `global_wiki_path` unset). There is NO `wiki-context` HTTP endpoint — that was a phantom assumption from #119/#120; the wiki flow is entirely prompt-driven.
- Frontend `DebatePanel` scans the judge report + coordination log with a regex for the captured PR URL to surface it in the verdict view.
- Adding `SessionType::Debate { variants }` touched the same four-enum drift surface as any new variant (see "SessionType Variant Synchronization") — the Rust compiler caught every exhaustive-match site; the TS mirror + `{ "Debate": ... }` object-tag did NOT (needed `serdeEnumVariantName`).

### ActionRegistry Route Consolidation + `{ renderer, data }` Envelope (#129)
- Typed session HTTP/Tauri routes (`get_info`, `stop`, `close`, `launch_*`) can be repointed to dispatch through a single `ActionRegistry` instead of calling `state.session_controller` directly — this is where the canonical session validators should live (`actions/session/mod.rs`), eliminating the triplicated `validate_session_name`/`validate_session_color`/`is_valid_hex_session_color`.
- `pty` and `coordination` commands can be action-backed while preserving security by having the actions **reject non-frontend callers** (don't widen HTTP action authority).
- The `{ renderer?, data }` render envelope is applied at the **HTTP-action result boundary and the `emit_conversation_message` boundary** without changing raw registry outputs. `renderer` is a **plain string** ("diff"/"table"), NOT a serde-tagged enum (frontend integration contract). Detect structured-table shape BEFORE the git/worktree "diff" name-heuristic, or `git.*` outputs mask real tables.

### Windows `cargo test` conpty Loader Fix via cfg(test) Shims
- Root cause of the long-standing `0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND`: a Tauri **lib test binary** fails to load BEFORE the Rust harness runs because unit tests link native desktop/runtime code they never exercise — `portable-pty`'s ConPTY strings AND the Tauri/Wry desktop bootstrap.
- Fix: isolate those paths behind `cfg(test)` shims — `pty/session_stub.rs`, `runtime/local_pty_stub.rs`, and a `tauri_shim.rs` that re-exports real `AppHandle`/`Emitter` in production but a no-op emitter under test; gate `mod commands;` + the `run()` bootstrap behind `#[cfg(not(test))]`. Production `portable-pty`/Tauri behavior still compiles; `cargo test` links only lightweight doubles.
- **Scope the stubs to `cfg(all(test, windows))`** — the loader crash is Windows-specific, so non-Windows test builds should exercise the real PTY/runtime modules.
- This made `cargo test` run for the FIRST time in this environment (389 tests). It also means the old "gate on `cargo check --tests`, can't run `cargo test`" rule is now superseded for branches carrying this fix — a Windows CI `cargo test` gate (`.github/workflows/rust-tests.yml`) enforces it.

### Multi-Agent Reconciliation: Verify Bot Findings Against Current Code Before Fixing
- In the post-PR quality loop, a Reconciler should **re-verify each external bot finding against the current code before prioritizing** — a non-trivial fraction are stale or false-positive. Concrete examples from PR #144: CodeRabbit/Gemini flagged `UpdateSessionMetadata` as missing `validate_input` (it already had it); a failing `test_patch_session_rejects_invalid_color` was sending a *valid* `#ffffff` (the test was wrong, not the product).
- Separate "real green-CI product regressions" from "harness/contract drift": of 28 failures surfaced when the test suite first ran, the real regressions were Swarm default-CLI persistence, PATCH null tri-state, and `get_info` live-vs-persisted reload; the rest were test-isolation, harness URI-builder bugs, and 404-vs-400 / 200-vs-404 contract drift.
- Run the quality loop through **spawned codex workers** (Reconciler → Resolvers), not inline — the Queen orchestrates and integrates. Give Resolvers **disjoint file ownership** (e.g. backend+tests vs frontend+CI) so their fixes cherry-pick without conflicts.

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

### Cross-Adapter Behavior Copy Without Audit
- When migrating from one adapter to another, **don't blindly copy** the old adapter's behavior verbatim
- The old adapter may have inconsistencies you're inheriting — audit all call sites for the legacy behavior first
- Example: original `gemini.rs` used `-i` for both `inline_task` and `prompt_file` paths; copying that into `antigravity.rs` was wrong because the controller's `add_inline_task_to_args` ALREADY used `-p` for the gemini solo path. Adapter and controller were inconsistent in the old code too; the migration was the right moment to fix it, not perpetuate it.
- Mechanical check: when writing a pair of `normalize_X` / `normalize_Y` helpers (or any pair of migration helpers operating on related data), diff their bodies — any field one touches, the other should also touch (or there should be an explicit comment explaining why not)

### Assuming Same-Named Flags Have Same Semantics Across CLIs
- Two CLIs with a flag of the same name (e.g., gemini's `-i` and agy's `-i`) do NOT necessarily have the same semantics
- gemini's `-i "Read X and execute"` actually executed the prompt; agy's `-i` was hypothesized to do the same — but actually our routing was broken (see Name-vs-Executable Routing above)
- Migration acceptance criteria must validate **observed behavior**, not flag-name parity

### Filing Investigation Issues Before Smoke Testing
- Issue #115 was filed hypothesizing an `agy -i` semantics bug; turned out to be our own routing bug in `add_prompt_to_args` (matched on cli name "antigravity", callers passed binary name "agy")
- The hypothesis was wrong, but the bot caught the real cause on PR review before the day was lost
- **Rule**: when filing an upstream / external investigation issue, first verify with the actual command-line (PTY log, strace, or just check the constructed args) that the symptom is indeed the external system's behavior — not your own integration bug
- Premature issue filing introduces motivated reasoning (the bug "must be" external) that delays seeing the real cause

### Tolerating a Persistent Warning Baseline
- 22 svelte-check warnings was the accepted baseline for many PRs; any new warning introduced was invisible noise
- Engineers used "no change to count" as "no regression" smoke test, which only works if the baseline is stable
- Fix: periodically drive the warning baseline toward zero — any new warning then surfaces immediately
- See issue [#117](https://github.com/rdfitted/hive-manager/issues/117) for the cleanup pass
- General rule: an accepted-warning-count is a signal-to-noise problem, not just lint noise

### `Option<Option<T>>` Does Not Preserve JSON null-vs-absent by Default
- A PATCH endpoint that needs tri-state semantics (`absent` = leave unchanged, `null` = clear, `value` = set) CANNOT just use `Option<Option<T>>` on the DTO — serde deserializes both absent and `null` to `None`, so `{"color": null}` silently fails to clear the field.
- Fix requires BOTH: a custom deserializer (`absent → None`, `null → Some(None)`, value → `Some(Some(v))`) on the DTO, AND the action/HTTP payload must **omit absent fields** rather than serializing them as `null` when it forwards to the action layer.
- This was the root cause of `test_patch_session_null_clears_field` failing once the suite became runnable. Apply at the HTTP DTO and the action DTO together — fixing only one leaves the bug.

### Codex Workers Run Repo-Wide `cargo fmt` (Integration Hazard)
- Codex CLI workers frequently run a repo-wide `cargo fmt` as part of their flow, reformatting 40-70 files unrelated to their task. Because rustfmt is deterministic the churn is identical across workers (so it mostly merges), but it bloats the PR, buries real changes from bot reviewers, and conflicts where formatting abuts another worker's semantic edit.
- Integration mitigation: detect fmt-only files with a `rustfmt`-normalized diff (`fmt(base) == fmt(worker-version)` ⇒ fmt-only) and `git restore` them, keeping only the semantically-changed files; do NOT run a final global `cargo fmt` if the repo has pre-existing rustfmt debt (it would reintroduce the churn). Broadcast "no repo-wide fmt" to workers early — some self-revert.
- The CI gate added in this epic runs `cargo check --tests` + `clippy` + `cargo test`, NOT `cargo fmt --check`, precisely because the repo carries pre-existing formatting debt.

### Swarm Persisted Queen Config as Session Defaults
- Swarm session creation (both direct and planning paths in `controller.rs`) persisted `queen_config.cli/model` as the session's `default_cli/default_model`, so a request asking for `codex` defaults stored `qwen` (the Queen's CLI) instead.
- Request-level `default_cli`/`default_model` must be carried through `SwarmLaunchConfig` separately from the Queen config. Symptom surfaced as `test_launch_swarm_accepts_model_capable_configs` once the suite ran. Generalizes the existing "Session Defaults Propagation" pattern — the request default and the Queen's CLI are NOT the same thing.

## Code Conventions

### HTTP Handler Structure
- Validate inputs at top of handler (path params first, then body fields)
- Use `ApiError::bad_request()` / `ApiError::not_found()` / `ApiError::internal()` for errors
- Return `(StatusCode::CREATED, Json(...))` for POST, `StatusCode::NO_CONTENT` for DELETE
- Session-scoped handlers: always call `validate_session_id()` first
- Pending learning ingestion: validate JSONL value types and file path constraints **before both API submission and fallback append**
- Exact-line dedup comments must match grep/comm implementations to avoid misleading integrators

### Storage Layer
- JSONL format for append-friendly data (learnings)
- Markdown for curated content (project-dna.md)
- Legacy methods marked `DEPRECATED: Use *_session for new code`
- `tempfile` crate for atomic rewrites, `fs::OpenOptions::append` for appends
- Worker fallback `learnings.pending.jsonl` records include an extra `date` field but still deserialize through `SubmitLearningRequest` because serde ignores unknown fields by default
- Integration coverage should POST the raw JSONL line to the session endpoint and read the session store (don't mock the file format)

### Prompt Template Escaping
- `{var}` in format strings must be escaped as `{{var}}` to produce literal braces
- Template placeholders use `{{session_id}}` (double braces) for Mustache-style interpolation
- `{id}` in raw string literals passed to `format!()` will be interpreted as format args

### Dependency Management
- `tempfile` in `[dependencies]` (not just `[dev-dependencies]`) when used in production code
- `uuid` features: `v4` for random generation, `v5` for deterministic content-based hashing
- Svelte checks may require `npm ci` in fresh worktrees because `node_modules` is gitignored

### Versioning (project scheme)
- `x.Y.0` for new features
- `x.x.Z` for fixes/extensions
- Three files in lockstep: `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`
- Don't forget `Cargo.lock` (the version line for `hive-manager` regenerates on first `cargo` command but should be committed)

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
- `VALID_CLIS` must stay synchronized with `default_config()` in `storage/mod.rs` AND `adapters/mod.rs::VALID_CLIS`
- Applied to all handlers accepting CLI input (workers, planners)
- Regression guards: `test_add_worker_accepts_<cli>` integration test per CLI

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
- When a CLI has no model flag (e.g., antigravity), set `defaultModel: ''` in `clis.ts` — the UI uses empty string as the signal to hide the model field entirely
- Show an informational note in place of the hidden dropdown (e.g., "Model is set globally in ~/.gemini/antigravity-cli/settings.json")

### Solo Mode as Zero-Worker Hive
- Frontend maps Solo mode to a Hive session with zero workers — backend detects this and spawns a single agent directly
- Avoids creating a separate session type flow end-to-end; reuses existing `launch_hive_v2` plumbing
- Dedicated `launch_solo()` in controller skips orchestration: no task files, no queen prompt, no watcher setup

### CLI-Specific Command Builders
- Each CLI (claude, gemini, antigravity, codex, droid, cursor) has different prompt flags (`-p`, `-q`, positional, `-i`)
- Dedicated solo command builder avoids coupling to orchestration-oriented defaults
- Model flag passthrough varies per CLI — must be handled per-type
- Antigravity (`agy`) has NO `--model` flag — model + verbosity live in `~/.gemini/antigravity-cli/settings.json`. `agy --help` is the source of truth.
- Solo-mode `agy -p` is affected by [upstream #76](https://github.com/google-antigravity/antigravity-cli/issues/76) — silently drops stdout in non-TTY contexts. Hive worker mode (PTY) is unaffected.

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
- **antigravity (agy)**: Default for frontend role since PR #116. Indexes similar to gemini. Settings.json owns model + verbosity.
- **gemini**: Selectable peer until Google's 2026-06-18 deprecation. Indexes codebase before starting (~7-12 min). Reliable once indexing completes.
- **cursor**: Good for review/test tasks. WSL environment may lack Rust toolchain.
- **claude**: Most reliable for autonomous work. Starts producing output faster than codex/gemini. Best for complex multi-site refactors.
- **droid**: Fastest (~2min). Excellent for handler changes, validation, straightforward tasks. Minimal indexing overhead.
- **Strategy**: Wait at least **12-15 minutes** before declaring a worker stalled. Codex / Gemini / agy index extensively — no git diff for 8-12 min is normal startup behavior. Only Droid and Claude start producing output quickly. Check terminal/PTY activity if possible, not just git diff. Only flag as truly stalled if zero terminal activity AND >15 min elapsed.

### Port Conflict Diagnosis (Dev vs Installed)
- When the dev build's UI shows "Session Not Found" for a session ID that the backend just minted, suspect **two backends competing for port 18800** before suspecting an API bug
- Both the installed Hive Manager (`%LOCALAPPDATA%\Hive Manager\hive-manager.exe`) and the dev build (`target\debug\hive-manager.exe`) bind 18800 — whichever started first wins, and the new build silently falls through with the frontend talking to the WRONG backend
- Diagnose: `Get-NetTCPConnection -LocalPort 18800 | join Get-Process` — the smoking gun is the listening PID's `Path` (`AppData\Local` = old, `target\debug` = new)
- Fix: `Stop-Process` the old installed PID, kill the dev build too (so it rebinds cleanly), then restart `tauri dev`
- Generalizable: any time the symptom is "UI shows stale/missing data from API that should exist", **check port ownership before code paths**

## Model Performance Notes

### Multi-Agent Verification
- 3 agents per concern with different models provides reliable consensus
- All 7 concerns in PR #19 were validated VALID by 3/3 agents (high confidence)
- Different models catch different aspects - useful for comprehensive analysis

## Hot Files
Files frequently modified across sessions — pay extra attention. Sorted by curation-weighted touch count:
- `src-tauri/src/session/controller.rs` (16x recent + many historical) — 4200+ lines, prompt templates, state machine, CLI flag-construction blocks, doc tables. The "where everything meets" file.
- `src-tauri/src/storage/mod.rs` (12x recent) — Dual storage paths, role defaults, session persistence, normalize helpers. Touched for any new role, CLI, or migration.
- `src-tauri/src/http/tests.rs` (11x recent) — Integration test suite. Every new HTTP behavior gets a test here.
- `src/lib/config/clis.ts` (8x recent) — Frontend source-of-truth for CLI options + role defaults. Must lockstep with Rust storage.
- `src/lib/components/AgentConfigEditor.svelte` (8x recent) — Per-CLI preset arrays, model-on-CLI-switch logic, model-field-hidden UI.
- `src-tauri/src/templates/mod.rs` (7x recent) — Built-in session templates (shared-cell, feature-build-hive, fusion-compare). New CLIs touch these.
- `src/lib/components/LaunchDialog.svelte` (6x recent) — Launch payload assembly, initial role configs. Must source from `defaultRoles`, not literals.
- `src-tauri/src/cli/registry.rs` (5x recent) — `default_model()`, behavior profiles, test fixtures. CLI peers added here.
- `src-tauri/src/adapters/mod.rs` (5x recent) — `VALID_CLIS`, factory `get_adapter()`. Lockstep with `http/handlers/mod.rs::VALID_CLIS`.
- `src-tauri/src/http/handlers/evaluator.rs` (4x recent) — Evaluator launch + QA worker spawning.
- `src-tauri/src/http/handlers/sessions.rs` (4x recent) — Active session listing, heartbeat payload assembly.
- `src-tauri/src/coordination/injection.rs` — Authority enforcement, inject methods. Critical for security.
- `src/lib/stores/sessions.ts` — Frontend type unions must mirror Rust enums exactly.

## Keywords → Files Mapping
Quick lookup for common tasks:
- **CLI migration / new CLI**: `src-tauri/src/adapters/`, `cli/registry.rs`, `storage/mod.rs::default_config`, `templates/mod.rs`, `http/handlers/mod.rs::VALID_CLIS`, `src/lib/config/clis.ts`, `AgentConfigEditor.svelte`
- **Role default change**: All 5 default-mirroring files (see "Default-Mirroring Across 5 Files" pattern)
- **Prompt-injection / worker spawn**: `session/controller.rs::add_prompt_to_args` + `add_inline_task_to_args` + `build_command` + `build_solo_command`, plus the relevant `adapters/<cli>.rs`. **Always include manual E2E smoke as acceptance criterion.**
- **QA verdict / heartbeat / staleness**: `http/handlers/sessions.rs` (active session payload), `coordination/state.rs`, frontend `heartbeatStore.ts`
- **Evaluator config**: `http/handlers/evaluator.rs`, `controller.rs::launch_evaluator`, frontend `LaunchDialog.svelte` evaluator section
- **Worker prompt paths**: `controller.rs::write_prompt_file`, `<worktree>/.hive-manager/prompts/` for workers, `<project_root>/.hive-manager/{session_id}/prompts/` for Queen/planner/evaluator
- **Cursor / WSL**: `controller.rs::to_wsl_path`, `controller.rs::add_prompt_to_args` (must match both `cursor` and `wsl` identifiers)

---
*Curated from learnings.jsonl (59 entries: 23 from prior curation + 36 from this curation) + 1 hive session payload*
*2026-06-19: +9 entries from hive session bb8e9ce6 (Debate mode #120 + agent-native epic follow-ups #129 + multi-agent quality loop / PR #144). New patterns: Debate-as-Fusion-profile, ActionRegistry consolidation + render envelope, Windows conpty cargo-test fix via cfg(test) shims, multi-agent reconciliation discipline; anti-patterns: Option<Option<T>> null-vs-absent, codex repo-wide cargo fmt, Swarm queen-config-as-defaults.*
*Last updated: 2026-06-19*
