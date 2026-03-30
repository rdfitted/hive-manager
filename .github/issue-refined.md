## Description

Add conversation-file-based inter-agent communication alongside existing task files for Hive mode. Task files remain the source of truth for role assignment and responsibilities. Conversation files enable agents to coordinate when tasks overlap, ask questions, and report progress via heartbeat.

**Scope**: Hive mode only (Swarm/Fusion in future PRs).
**Supersedes**: #9 (agent messaging), #12 (active sessions), #22 (coordination logging)

## Problem

- Workers with overlapping tasks can't coordinate — they work blind and create merge conflicts
- Queen polls task files for `COMPLETED` string — misses stalls, no partial progress visibility
- Stalled agents (Codex, Qwen) look identical to working ones — no self-reporting mechanism
- Workers can't ask the Queen or peers questions mid-task
- No discovery mechanism for external tools to find active sessions and agents

## Design: Task Files + Conversation Files

### Task Files (unchanged)
Define **what** each agent does — role, scope, files to touch, acceptance criteria:
```
.hive-manager/{session_id}/tasks/
  worker-1-task.md    # Role assignment, responsibilities, status
  worker-2-task.md
  worker-3-task.md
```
Status transitions remain: STANDBY → ACTIVE → COMPLETED/BLOCKED

### Conversation Files (new)
Enable **how** agents coordinate while doing their tasks:
```
.hive-manager/{session_id}/conversations/
  queen.md          # Queen's inbox — workers report here, ask questions
  worker-1.md       # Worker 1's inbox — queen and peers message here
  worker-2.md       # Worker 2's inbox
  shared.md         # Broadcast channel — announcements, all agents read
```

Append-only markdown format with timestamps:
```markdown
---
[2026-02-13T10:30:00Z] from @worker-1
Auth handler done. Touched controller.rs and routes.rs.
Worker-2: I added a `/auth` prefix — make sure your routes don't clash.

---
[2026-02-13T10:31:00Z] from @queen
Good work. Move on to rate limiting (see task file for details).
```

### When to Use Which

| Mechanism | Purpose | Example |
|-----------|---------|---------|
| **Task file** | Role assignment, scope, acceptance criteria | "Implement auth handler. Touch: controller.rs, routes.rs" |
| **Conversation file** | Coordination, questions, progress | "Hey worker-2, I'm using /auth prefix for routes" |
| **Heartbeat** | Liveness signal, stall detection | POST to `/api/sessions/{id}/heartbeat` with status |

### Heartbeat + Stall Detection

**All supported CLIs have shell execution capability** (verified 2026-02-13):
- **claude**: Bash tool (native exec)
- **codex**: Shell exec with approval prompts
- **gemini**: `run_shell_command` tool
- **droid**: Tiered autonomy exec (medium+ autonomy for curl)
- **cursor**: Shell with Y/N approval
- **opencode**: Configurable bash tool
- **qwen**: Shell/Bash tool support

All agents use **dual heartbeat mechanism**:
1. **Structured heartbeat**: `POST /api/sessions/{id}/heartbeat` with JSON payload (`agent_id`, `status`, `summary`)
2. **Conversation writes**: Any append to conversation files updates `last_activity` timestamp

Prompt templates instruct agents to:
- POST heartbeat between subtasks or every 60-90 seconds
- Check own conversation file for new messages
- Append progress to `queen.md` when completing milestones
- Read `shared.md` for broadcasts

**Stall Detection**: Server tracks per-agent `last_activity`. No activity (heartbeat OR conversation write) for >3 minutes → agent flagged as potentially stalled, Tauri event emitted.

### Agent Persistence

Agents persist after task completion (no termination). When a worker completes its task:
- Reports completion to `queen.md`
- Transitions to IDLE state (not terminated)
- Continues checking conversation file on heartbeat cadence
- Queen can re-engage by writing new task to worker's conversation file

## API Endpoints

```
POST /api/sessions/{id}/conversations/{agent}/append
Body: { "from": "worker-1", "content": "Auth handler complete. Ready for next task." }
→ Appends message to {agent}.md with fs2 exclusive file lock

GET /api/sessions/{id}/conversations/{agent}?since=<timestamp>
→ Returns messages from {agent}.md since timestamp (fs2 shared lock)

GET /api/sessions/active
→ Returns live sessions with agent list, roles, and last_activity timestamps

POST /api/sessions/{id}/heartbeat
Body: { "agent_id": "worker-1", "status": "working", "summary": "3/5 files done" }
→ Updates last_activity timestamp, emits Tauri event if status changed
```

## CLI Exec Capabilities (Verified 2026-02-13)

All 7 supported CLIs can execute shell commands (curl for HTTP API calls):

| CLI | Exec Mechanism | Approval | Sources |
|-----|---------------|----------|---------|
| **claude** | `Bash` tool | Auto (in dangerously-skip mode) | Native Claude Code tool |
| **codex** | Shell exec | Y/N prompt (may stall) | OpenAI Codex CLI docs |
| **gemini** | `run_shell_command` | Y/N prompt | [Gemini CLI Docs](https://geminicli.com/docs/tools/shell/) |
| **droid** | Tiered autonomy | Auto (medium+ autonomy) | [Factory CLI Docs](https://docs.factory.ai/reference/cli-reference) |
| **cursor** | Shell | Y/N prompt | [Cursor CLI Docs](https://www.codecademy.com/article/getting-started-with-cursor-cli) |
| **opencode** | `bash` tool | Configurable | [OpenCode Docs](https://opencode.ai/docs/cli/) |
| **qwen** | Shell/Bash | Approval | [Qwen Code Docs](https://qwenlm.github.io/qwen-code-docs/) |

**Implication**: All agents can POST heartbeats and read conversation files via curl. No need for separate "file-only" fallback mechanism.

## Implementation Phases

### Phase 1: Conversation File Infrastructure + File Locking
- **Add `fs2 = "0.4"` to `src-tauri/Cargo.toml`** (NOT present — PR #23 was closed without merging)
- Create `conversations/` directory on hive session launch (`storage/mod.rs::init_session_structure()`)
- Create `{agent}.md` and `shared.md` files when agents spawn
- `POST /api/sessions/{id}/conversations/{agent}/append` with **fs2 exclusive locking**
- `GET /api/sessions/{id}/conversations/{agent}?since=` endpoint with **fs2 shared locking**
- Content sanitization (reuse patterns from `coordination/injection.rs`)
- Route registration in `http/routes.rs`
- Integration tests following `http/tests.rs` patterns

### Phase 2: Heartbeat + Active Sessions
- `POST /api/sessions/{id}/heartbeat` endpoint
- Per-agent `last_activity: DateTime<Utc>` tracking in `SessionController`
- `GET /api/sessions/active` endpoint returning sessions with agent metadata
- Stall detection: background task checking `last_activity` every 60s, flag if >3min
- Tauri event emission on state change (`agent-stalled`, `agent-recovered`)
- Frontend status indicators per agent

### Phase 3: Prompt Template Integration
- Update `build_worker_prompt()` in `session/controller.rs`:
  - Add conversation file path as environment variable
  - Add curl command template for heartbeat POST
  - Add instructions to check `{agent}.md` between subtasks
  - Add instructions to write to `queen.md` for progress reports
  - Add peer messaging pattern with curl to other agent conversation files
- Update `build_queen_prompt()`:
  - Add instructions to check `queen.md` inbox periodically
  - Add instructions to write to worker conversation files for assignments
  - Add instructions to use `shared.md` for broadcasts
- Add CLI-specific examples (handle approval prompts for codex/gemini/cursor)

### Phase 4: Frontend
- Conversation file viewer component (per-agent tabs or integrated into CoordinationPanel)
- Agent status indicators (active/idle/stalled) with color coding
- Role-colored messages (Queen=purple, Worker=cyan, Planner=yellow)
- Real-time updates via Tauri events
- Agent re-engagement UI (write to idle agent's conversation file from UI)

## Relevant Files

| File | Lines | Relevance | Notes |
|------|-------|-----------|-------|
| `src-tauri/Cargo.toml` | - | **CRITICAL** | Add `fs2 = "0.4"` dependency (currently missing) |
| `src-tauri/src/storage/mod.rs` | 186-208, 406-467 | **HIGH** | Add conversations dir init, implement locked append/read |
| `src-tauri/src/http/handlers/conversations.rs` | - | **HIGH** | NEW FILE - conversation endpoints |
| `src-tauri/src/http/routes.rs` | 16-46 | **HIGH** | Add conversation + heartbeat routes |
| `src-tauri/src/session/controller.rs` | 2079-2920, 280-412 | **HIGH** | Update prompt templates, add `last_activity` tracking |
| `src-tauri/src/http/handlers/mod.rs` | 13-19 | **MEDIUM** | Reuse `validate_session_id()`, add `validate_agent_id()` |
| `src-tauri/src/coordination/injection.rs` | 50-200 | **MEDIUM** | Reference patterns for PTY injection |
| `src-tauri/src/coordination/mod.rs` | 1-55 | **MEDIUM** | Potentially extend `CoordinationMessage` types |
| `src-tauri/src/http/tests.rs` | 1663+ | **MEDIUM** | Test patterns for new endpoints |
| `src/lib/stores/coordination.ts` | 225+ | **MEDIUM** | Extend for conversation UI updates |

## Acceptance Criteria

- [ ] `fs2` dependency added to Cargo.toml
- [ ] Conversation directory + files created on hive session launch
- [ ] Agents can append to any agent's conversation file via API with fs2 exclusive lock
- [ ] Agents can read their inbox with `since` filter via API with fs2 shared lock
- [ ] `POST /api/sessions/{id}/heartbeat` endpoint tracks per-agent activity
- [ ] `GET /api/sessions/active` returns live sessions with agent list and activity timestamps
- [ ] Queen prompt includes conversation file checking instructions with curl examples
- [ ] Worker prompt includes peer messaging + heartbeat instructions with curl examples
- [ ] All 7 CLI types (claude, codex, gemini, droid, cursor, opencode, qwen) have working curl examples
- [ ] Stall detection flags agents with no activity (heartbeat OR conversation write) >3min
- [ ] Agents persist after completion — re-engageable via conversation file
- [ ] Task files continue to work unchanged for role assignment
- [ ] Integration tests for conversation endpoints (traversal attacks, concurrent writes, authorization)
- [ ] `cargo check --tests` passes

## Security

- **Path traversal**: Reuse `validate_session_id()` from `http/handlers/mod.rs` for session_id validation
- **Agent validation**: New `validate_agent_id()` function validates agent name format (alphanumeric + hyphens only, max 64 chars, no `/`, `\`, `..`)
- **Agent authorization**: Verify agent_id exists in session's agent registry before allowing conversation writes
- **Content sanitization**: Apply patterns from `coordination/injection.rs` (newline handling, length limits)
- **No cross-session conversation access**: Validate session ownership before any conversation operation
- **fs2 file locking**: Exclusive lock for writes, shared lock for reads (prevents concurrent write corruption)
- **CORS hardening**: Current CORS allows all origins (`src-tauri/src/http/routes.rs:11`) — consider tightening for local-only access

## Testing Requirements

### Integration Tests (in `src-tauri/src/http/tests.rs`)
- [ ] Append message to conversation file → verify file content and fs2 lock acquisition
- [ ] Concurrent appends from 5 simulated agents → verify no interleaved messages
- [ ] Read conversation file with `?since=` timestamp filter → verify correct message filtering
- [ ] POST heartbeat → verify `last_activity` timestamp updated
- [ ] Stall detection → verify agent flagged after 3min silence
- [ ] GET /api/sessions/active → verify returns only live sessions with agent metadata
- [ ] Path traversal attempt on session_id → verify 400 error
- [ ] Path traversal attempt on agent_id → verify 400 error
- [ ] Cross-session conversation access attempt → verify authorization failure
- [ ] Invalid agent_id format (spaces, slashes) → verify validation error

### Known Testing Constraint
Per `MEMORY.md`: `cargo test` has Windows DLL issue (`STATUS_ENTRYPOINT_NOT_FOUND`). Use `cargo check --tests` for compilation verification. Integration tests may need manual execution or CI environment.

## Additional Context

### PR #23 Status (CRITICAL)
**Issue originally claimed "Builds on PR #23 (file locking, sanitization)" but PR #23 was CLOSED on 2026-02-12 WITHOUT MERGING.** The `fs2` dependency and file locking infrastructure from PR #23 do NOT exist in the current codebase. Issue #27 must implement these from scratch.

Existing security patterns (from separate commit 36a4a91):
- Path traversal validation: `validate_session_id()` in `http/handlers/mod.rs`
- Content sanitization: Basic sanitization in `coordination/injection.rs`
- These are SEPARATE from PR #23 and should be reused

### Current vs Proposed Architecture

**Current**:
- Queen writes task files → Workers poll task files → Watcher detects COMPLETED
- One-way communication (Queen → Workers via PTY injection)
- Shared coordination.log for audit trail

**Proposed**:
- Task files remain for role assignment
- Conversation files enable bidirectional peer communication
- Heartbeat enables proactive stall detection (vs reactive polling)
- Agents persist in IDLE state (vs terminate on completion)

### File Locking Strategy

Use `fs2` crate advisory locking:
- **Writes**: `file.try_lock_exclusive()` with exponential backoff (max 3 retries)
- **Reads**: `file.lock_shared()` allows multiple concurrent readers
- **Best practice**: Always call lock operations inside `tokio::task::spawn_blocking` to avoid blocking async executor

### Alternative Queue Patterns (from research)

For high-throughput scenarios (>100 messages/sec per file), consider:
- **`walrus` crate**: Lock-free WAL achieving 1M ops/sec
- **`aora` crate**: Append-only random-accessed persistence
- Current markdown append with fs2 locking is sufficient for typical hive sessions (<10 agents, <10 msg/min/agent)

### Stall Detection Algorithm

Recommended: **Phi Accrual Failure Detector** (used in Cassandra/Kubernetes)
- Adaptive probabilistic detection based on heartbeat interval distribution
- Avoids false positives from network jitter or temporary slowdowns
- Returns suspicion level (0.0-1.0) instead of binary alive/dead
- Threshold: >0.9 = considered stalled

Simple initial implementation (can upgrade to Phi Accrual later):
- Fixed 3-minute timeout (3x the 60-90 second heartbeat interval)
- Binary alive/stalled state

### Agent Persistence Model

Current agent lifecycle: `Starting → Running → Completed` (then terminate)

Proposed agent lifecycle: `Starting → Running → Idle ⇄ Running` (persist)

Changes needed:
- Update `AgentInfo` struct with `last_activity: DateTime<Utc>` field
- Add `Idle` status to agent state enum
- Update prompt templates to return to idle polling state after task completion
- Modify `stop_session()` to preserve agent PTYs in idle state (vs terminate)

---

*Refined with multi-agent reassessment (7 agents: gemini-flash, codex-planner, opencode-bigpickle, opencode-glm, claude-haiku, 2 web search) + CLI exec capability verification (all 7 CLIs support shell exec). PR #23 file locking infrastructure does NOT exist — must implement from scratch.*

**Reassessment Date**: 2026-02-13
**Files Verified**: 12 core files
**Confidence**: HIGH

🤖 Generated with [Claude Code](https://claude.com/claude-code)
