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

### Prompt Template Structure in controller.rs
- Queen prompt (standard Hive): includes Learning Curation Protocol, tool table, sequential spawning
- Queen prompt (Swarm): similar but with planner-focused curation protocol
- Worker prompt: includes Learnings Protocol section with correct outcome values
- Tool files (`.hive-manager/{session_id}/tools/*.md`): generated by `write_tool_files()` method

### Tauri Command Pattern
- `resume_session` loads `PersistedSession` from storage, converts to active `Session`
- `PersistedAgentInfo` stores role as String (e.g., 'Queen', 'Worker(1)') requiring string parsing

## Model Performance Notes

### Multi-Agent Verification
- 3 agents per concern with different models provides reliable consensus
- All 7 concerns in PR #19 were validated VALID by 3/3 agents (high confidence)
- Different models catch different aspects - useful for comprehensive analysis

---
*Curated from learnings.jsonl (15 entries) by manual session*
*Last updated: 2026-02-05*
