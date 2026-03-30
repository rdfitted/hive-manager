# improve: Enable multi-codebase support by making learnings API session-scoped

## Description

The recent PR that added session persistence and learnings functionality broke the ability to work on multiple codebases simultaneously, which is a core value proposition of this tool. The learnings API endpoints (`/api/learnings`, `/api/project-dna`) currently use an ambient context anti-pattern where they infer the project path from all active sessions, explicitly failing when multiple sessions have different project paths.

The error message `"Multiple active sessions with different project paths"` prevents legitimate multi-codebase workflows. This is not a fundamental architectural limitation - the session controller, storage layer, and persistence mechanisms all correctly support multiple projects. The issue is isolated to the HTTP handler layer's inference mechanism.

## Context

**Type**: enhancement
**Scope**: medium (3 core files + prompt templates)
**Complexity**: medium (API contract changes, backward compatibility, prompt updates)
**Priority**: high (blocks core multi-codebase use case, regression from previous capability)

## Relevant Files

Files identified by multi-agent investigation (ranked by relevance):

| File | Lines | Relevance | Notes |
|------|-------|-----------|-------|
| `src-tauri/src/http/handlers/learnings.rs` | 28-42 | **HIGH** | Contains `resolve_project_path()` that explicitly errors with "Multiple active sessions with different project paths" - **root cause** |
| `src-tauri/src/session/controller.rs` | 115-127, 1304, 1309, 1436 | **HIGH** | Session struct with `project_path` field, SessionController with `HashMap<String, Session>` supporting multiple projects, agent prompt templates with curl commands needing updates |
| `src-tauri/src/storage/mod.rs` | 442-513 | **HIGH** | Storage layer correctly supports multi-project (append_learning, read_learnings accept `project_path` parameter) - proves architecture is sound |
| `src-tauri/src/http/routes.rs` | 18-39 | **MEDIUM** | HTTP route definitions - need to add session-scoped endpoints: `/api/sessions/{id}/learnings`, `/api/sessions/{id}/project-dna` |
| `src-tauri/src/templates/mod.rs` | 26-27, 189, 194, 257, 262 | **MEDIUM** | PromptContext includes `session_id` and `project_path`, Queen prompt templates need curl command updates |
| `src-tauri/src/http/tests.rs` | 1-81 | **MEDIUM** | Test infrastructure - needs multi-project test scenarios |
| `src/lib/stores/sessions.ts` | 94+ | **LOW** | Frontend session store - may need API call updates |
| `src/lib/components/SessionSidebar.svelte` | 115+ | **LOW** | UI already filters sessions by current directory (PR #5) - no changes needed |

## Analysis

### Root Cause

The `resolve_project_path()` function in `learnings.rs` (lines 28-42) was designed as a convenience shortcut assuming a single-project mental model. It iterates all active sessions, grabs the first session's `project_path`, and fails if any other session has a different path. This is an **ambient context anti-pattern** where the API infers project context from global state rather than from the request itself.

The critical insight: **this is a single API design flaw, not a systemic architecture problem**. The session controller stores sessions in a `HashMap<String, Session>` with per-session `project_path` values. The storage layer's `append_learning()` and `read_learnings()` methods already accept `project_path` parameters. The frontend filters sessions by directory correctly. Only the HTTP handler layer assumes a single project.

### Impact Assessment

**What breaks:**
- `POST /api/learnings` - Workers cannot submit learnings when multiple projects are active
- `GET /api/learnings` - Queen cannot review learnings for curation
- `GET /api/project-dna` - Queen cannot review project DNA

**What works:**
- All session-scoped endpoints (`/api/sessions/{id}/workers`, `/api/sessions/{id}/planners`) work perfectly with multiple projects
- Session management, persistence, and resumption are unaffected
- Frontend session filtering by `currentDirectory` works correctly

### Technical Approach

Three implementation approaches were evaluated:

1. **Query parameter** (`?session_id=abc`) - Backward compatible but inconsistent with existing API patterns
2. **Project path parameter** (`?project_path=/path`) - **Security risk**: allows arbitrary filesystem writes
3. **RESTful nesting** (`/api/sessions/{id}/learnings`) - **RECOMMENDED**: Consistent with existing patterns, secure, self-documenting

**Recommended: Approach 3 (RESTful Nesting)**

**Advantages:**
- Perfectly consistent with existing patterns: `/api/sessions/{id}/workers`, `/api/sessions/{id}/planners`
- Server-side path resolution prevents path traversal attacks
- `session_id` already exists in agent prompt context
- Clean REST semantics
- Self-documenting API structure

**Implementation Plan:**
1. Add three new routes in `routes.rs` following the `{id}/workers` pattern
2. Create handler functions in `learnings.rs` that accept `Path(session_id): Path<String>` and look up the session to get `project_path`
3. Keep old endpoints with fallback logic (single project = works, multiple projects = helpful error pointing to new endpoints)
4. Update all curl commands in agent prompt templates (`controller.rs` lines 1304, 1309, 1436)
5. Update built-in templates in `templates/mod.rs` (lines 189, 194, 257, 262)
6. Add integration tests for multi-project scenarios

## Acceptance Criteria

**Functional:**
- [ ] User can run two Hive sessions against different projects simultaneously
- [ ] Workers in Session A submit learnings to Project A's `.ai-docs/learnings.jsonl`
- [ ] Workers in Session B submit learnings to Project B's `.ai-docs/learnings.jsonl`
- [ ] Queen in Session A reviews only Project A's learnings
- [ ] Queen in Session B reviews only Project B's learnings
- [ ] Project DNA retrieval is session-scoped

**Backward Compatibility:**
- [ ] Old `/api/learnings` endpoints work when only one project is active
- [ ] Old endpoints return helpful error when multiple projects are active, suggesting new endpoints

**Security:**
- [ ] No agent can write learnings to arbitrary filesystem paths
- [ ] Session_id validation returns 404 for non-existent sessions
- [ ] File path validation in `files_touched` is preserved

**Edge Cases:**
- [ ] Completed sessions don't interfere with active session path resolution
- [ ] Session resumption doesn't break learning endpoints for other active sessions
- [ ] Concurrent learning submissions from multiple workers in same session work correctly

## Testing Requirements

**Multi-Project Test Scenario:**
1. Launch Hive on `/project-a`, submit learning via session-scoped endpoint
   - Verify learning appears in `/project-a/.ai-docs/learnings.jsonl`
2. Launch Hive on `/project-b` while `/project-a` session still active
   - Submit learnings to both sessions, verify isolation
3. Stop `/project-a` session, verify `/project-b` learnings still work
4. Resume persisted session for `/project-a`, verify learnings work for both
5. Call old `/api/learnings` with both sessions active, verify helpful error

**Edge Case Tests:**
- Multiple sessions on same project (same `.ai-docs/` directory)
- Zero active sessions (completed sessions in HashMap)
- Session with invalid/missing session_id
- Concurrent submissions from multiple workers

## Additional Considerations

### Security

The session_id-based approach prevents path traversal attacks. If we used `project_path` as a parameter, agents could craft arbitrary filesystem paths. With session_id, the server looks up `project_path` from its own trusted state.

### Performance

Current approach: `resolve_project_path()` acquires read lock, clones all sessions, iterates - O(N) per request.

Session-scoped approach: Single `HashMap::get()` lookup - O(1) amortized. Strictly better performance.

### User Experience

**Current broken state:** Users working on Project A and Project B simultaneously cannot have active Hive sessions for both. Error message gives no guidance.

**After fix:** Natural multi-project workflow restored. Agents already receive `session_id` in prompts (used in task file paths, worker spawns), so session-scoped URLs are a natural extension.

### Critical Implementation Details

**HIGHEST RISK**: Agent prompt template updates in `controller.rs` and `templates/mod.rs`. These contain hardcoded curl commands. If any prompt isn't updated, that agent will fail when multiple projects are active. All curl examples must change from:
```bash
curl "http://localhost:18800/api/learnings"
```
to:
```bash
curl "http://localhost:18800/api/sessions/{session_id}/learnings"
```

The `{session_id}` placeholder is already used in other curl commands agents receive (worker spawn, planner spawn), so this is consistent.

## Documentation Needs

- Internal: Update system prompts in `controller.rs` and `templates/mod.rs` (primary agent documentation)
- API: Document the new session-scoped endpoints (if external API docs exist)
- Migration: Note that old endpoints remain for backward compatibility with deprecation warnings

---

**Investigation Summary**:
- Agents used: 9 (3 Gemini models, 3 OpenCode scouts, Codex, Opus Plan, Opus analysis)
- Files identified: 8 core files
- Confidence: **HIGH** (consistent findings across all agents, root cause clearly identified, implementation path validated)

*Issue created with multi-agent investigation using Claude Code - Scale 3*
