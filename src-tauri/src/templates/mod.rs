// Template engine module - infrastructure for future prompt template features
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{SessionMode, WorkspaceStrategy};
use crate::pty::WorkerRole;
use crate::session::SessionType;

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Template not found: {0}")]
    NotFound(String),
    #[error("Invalid template: {0}")]
    Invalid(String),
}

/// Context for rendering prompts
#[derive(Debug, Clone)]
pub struct PromptContext {
    pub session_id: String,
    pub project_path: String,
    pub task: Option<String>,
    pub variables: HashMap<String, String>,
}

impl Default for PromptContext {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            project_path: String::new(),
            task: None,
            variables: HashMap::new(),
        }
    }
}

const DEFAULT_API_BASE_URL: &str = "http://localhost:18800";

fn normalize_api_base_url(raw: Option<&String>) -> String {
    let trimmed = raw.map(|value| value.trim()).unwrap_or_default();
    if trimmed.is_empty() {
        return DEFAULT_API_BASE_URL.to_string();
    }

    trimmed.trim_end_matches('/').to_string()
}

pub fn heartbeat_snippet(
    api_base_url: &str,
    session_id: &str,
    agent_id: &str,
    status: &str,
    summary: &str,
) -> String {
    let body = serde_json::json!({
        "agent_id": agent_id,
        "status": status,
        "summary": summary,
    })
    .to_string();

    format!(
        r#"cat <<'JSON' | curl -fsS -X POST "{api_base_url}/api/sessions/{session_id}/heartbeat" \
  -H "Content-Type: application/json" \
  --data-binary @-
{body}
JSON"#
    )
}

fn required_variable<'a>(
    variables: &'a HashMap<String, String>,
    key: &str,
) -> Result<&'a str, TemplateError> {
    let value = variables
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            TemplateError::Invalid(format!(
                "generic_heartbeat_snippet requires non-empty variable `{}`",
                key
            ))
        })?;

    Ok(value)
}

/// Information about a worker for prompt rendering
#[derive(Debug, Clone)]
pub struct WorkerInfo {
    pub id: String,
    pub role_label: String,
    pub role_type: String,
    pub cli: String,
    pub status: String,
    pub current_task: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub mode: SessionMode,
    pub cells: Vec<CellTemplate>,
    pub workspace_strategy: WorkspaceStrategy,
    pub is_builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CellTemplate {
    pub role: String,
    pub cli: String,
    pub model: Option<String>,
    pub prompt_template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RolePack {
    pub id: String,
    pub name: String,
    pub roles: Vec<CellTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateCatalog {
    pub templates: Vec<SessionTemplate>,
    pub role_packs: Vec<RolePack>,
}

pub fn builtin_session_templates() -> Vec<SessionTemplate> {
    vec![
        SessionTemplate {
            id: "bug-fix-hive".to_string(),
            name: "Bug-fix Hive".to_string(),
            description: "Opus Queen with GPT-5.6 backend and frontend coding principals."
                .to_string(),
            mode: SessionMode::Hive,
            cells: vec![
                CellTemplate {
                    role: "queen".to_string(),
                    cli: "claude".to_string(),
                    model: Some("opus".to_string()),
                    prompt_template: "queen-hive".to_string(),
                },
                CellTemplate {
                    role: "backend".to_string(),
                    cli: "codex".to_string(),
                    model: Some("gpt-5.6".to_string()),
                    prompt_template: "roles/backend".to_string(),
                },
                CellTemplate {
                    role: "frontend".to_string(),
                    cli: "codex".to_string(),
                    model: Some("gpt-5.6".to_string()),
                    prompt_template: "roles/frontend".to_string(),
                },
            ],
            workspace_strategy: WorkspaceStrategy::SharedCell,
            is_builtin: true,
        },
        // NOTE: Research mode is intentionally NOT exposed as a builtin SessionTemplate.
        // The template picker routes purely on SessionTemplate.mode, and the Rust
        // SessionMode enum only has Hive/Fusion — so a "research" template would be
        // forced to mode: Hive and launch through launch_hive_v2 (worktrees + queen-hive),
        // bypassing the no-git launch_research path. Research is launched from its own
        // dedicated tab in LaunchDialog (exactly like Solo mode, which is also not a
        // builtin template). See PR #121 review.
        SessionTemplate {
            id: "feature-build-hive".to_string(),
            name: "Feature-build Hive".to_string(),
            description:
                "Opus Queen plus GPT-5.6 backend/frontend coding principals and a Droid coherence specialist."
                    .to_string(),
            mode: SessionMode::Hive,
            cells: vec![
                CellTemplate {
                    role: "queen".to_string(),
                    cli: "claude".to_string(),
                    model: Some("opus".to_string()),
                    prompt_template: "queen-hive".to_string(),
                },
                CellTemplate {
                    role: "backend".to_string(),
                    cli: "codex".to_string(),
                    model: Some("gpt-5.6".to_string()),
                    prompt_template: "roles/backend".to_string(),
                },
                CellTemplate {
                    role: "frontend".to_string(),
                    cli: "codex".to_string(),
                    model: Some("gpt-5.6".to_string()),
                    prompt_template: "roles/frontend".to_string(),
                },
                CellTemplate {
                    role: "coherence".to_string(),
                    cli: "droid".to_string(),
                    model: Some("glm-5.1".to_string()),
                    prompt_template: "roles/coherence".to_string(),
                },
            ],
            workspace_strategy: WorkspaceStrategy::SharedCell,
            is_builtin: true,
        },
        SessionTemplate {
            id: "fusion-compare".to_string(),
            name: "Fusion Compare".to_string(),
            description: "Two candidate implementation cells plus a resolver recommendation pass."
                .to_string(),
            mode: SessionMode::Fusion,
            cells: vec![
                CellTemplate {
                    role: "candidate-a".to_string(),
                    cli: "codex".to_string(),
                    model: Some("gpt-5.6".to_string()),
                    prompt_template: "fusion-worker".to_string(),
                },
                CellTemplate {
                    role: "candidate-b".to_string(),
                    cli: "antigravity".to_string(),
                    model: None,
                    prompt_template: "fusion-worker".to_string(),
                },
                CellTemplate {
                    role: "resolver".to_string(),
                    cli: "claude".to_string(),
                    model: Some("opus".to_string()),
                    prompt_template: "resolver".to_string(),
                },
            ],
            workspace_strategy: WorkspaceStrategy::IsolatedCell,
            is_builtin: true,
        },
    ]
}

pub fn builtin_role_packs() -> Vec<RolePack> {
    vec![
        RolePack {
            id: "queen".to_string(),
            name: "Queen".to_string(),
            roles: vec![CellTemplate {
                role: "queen".to_string(),
                cli: "claude".to_string(),
                model: Some("opus".to_string()),
                prompt_template: "queen-hive".to_string(),
            }],
        },
        RolePack {
            id: "implementer".to_string(),
            name: "Coding Principal".to_string(),
            roles: vec![CellTemplate {
                role: "backend".to_string(),
                cli: "codex".to_string(),
                model: Some("gpt-5.6".to_string()),
                prompt_template: "roles/backend".to_string(),
            }],
        },
        RolePack {
            id: "reviewer".to_string(),
            name: "Reviewer".to_string(),
            roles: vec![CellTemplate {
                role: "coherence".to_string(),
                cli: "droid".to_string(),
                model: Some("glm-5.1".to_string()),
                prompt_template: "roles/coherence".to_string(),
            }],
        },
        RolePack {
            id: "resolver".to_string(),
            name: "Resolver".to_string(),
            roles: vec![CellTemplate {
                role: "resolver".to_string(),
                cli: "claude".to_string(),
                model: Some("opus".to_string()),
                prompt_template: "resolver".to_string(),
            }],
        },
    ]
}

/// Template engine for rendering role and queen prompts
pub struct TemplateEngine {
    templates_dir: PathBuf,
    builtin_templates: HashMap<String, String>,
}

impl TemplateEngine {
    /// Create a new template engine with the given templates directory
    pub fn new(templates_dir: PathBuf) -> Self {
        let mut engine = Self {
            templates_dir,
            builtin_templates: HashMap::new(),
        };
        engine.load_builtin_templates();
        engine
    }

    /// Load built-in templates
    fn load_builtin_templates(&mut self) {
        // Backend worker role template
        self.builtin_templates.insert(
            "roles/backend".to_string(),
            r#"# Backend Worker Role

You are a Backend Worker in a multi-agent coding session.

## Your Responsibilities
- Implement server-side logic, APIs, and data models
- Work with databases, authentication, and business logic
- Coordinate with Frontend workers on API contracts

## Communication Protocol
- Check your task assignments in the coordination system
- Report progress and completion via coordination.log
- Flag blockers immediately to your coordinator
- Check your conversation file between subtasks
- Report progress to `queen.md` after milestones
- Read `shared.md` for broadcasts

## Heartbeat (every 60-90s — REQUIRED)
```bash
{{generic_heartbeat_snippet}}
```

## Current Assignment
{{task}}
"#
            .to_string(),
        );

        // NOTE: No `roles/researcher` builtin template. Research workers are launched
        // via launch_research, which (like all v2 Hive launches) builds worker prompts
        // through controller::build_worker_prompt — that function special-cases the
        // `researcher` role (no EXECUTOR framing, no Learnings POST, no commits, report
        // findings to the Queen via the conversation API). A standalone role template
        // here would be dead code and was removed in PR #121 review.

        // Frontend worker role template
        self.builtin_templates.insert(
            "roles/frontend".to_string(),
            r#"# Frontend Worker Role

You are a Frontend Worker in a multi-agent coding session.

## Your Responsibilities
- Implement UI components and user interactions
- Manage client-side state and data flow
- Coordinate with Backend workers on API contracts

## Communication Protocol
- Check your task assignments in the coordination system
- Report progress and completion via coordination.log
- Flag blockers immediately to your coordinator
- Check your conversation file between subtasks
- Report progress to `queen.md` after milestones
- Read `shared.md` for broadcasts

## Heartbeat (every 60-90s — REQUIRED)
```bash
{{generic_heartbeat_snippet}}
```

## Current Assignment
{{task}}
"#
            .to_string(),
        );

        // Coherence worker role template
        self.builtin_templates.insert(
            "roles/coherence".to_string(),
            r#"# Coherence Worker Role

You are a Coherence Worker in a multi-agent coding session.

## Your Responsibilities
- Review code across all workers for consistency
- Ensure API contracts are properly implemented on both sides
- Check for integration issues and type mismatches
- Verify naming conventions and code style consistency

## Communication Protocol
- Review changes from other workers
- Report inconsistencies via coordination.log
- Suggest fixes to maintain coherence
- Check your conversation file between subtasks
- Report progress to `queen.md` after milestones
- Read `shared.md` for broadcasts

## Heartbeat (every 60-90s — REQUIRED)
```bash
{{generic_heartbeat_snippet}}
```

## Current Assignment
{{task}}
"#
            .to_string(),
        );

        // Simplify worker role template
        self.builtin_templates.insert(
            "roles/simplify".to_string(),
            r#"# Simplify Worker Role

You are a Simplify Worker in a multi-agent coding session.

## Your Responsibilities
- Review code for unnecessary complexity
- Suggest simplifications and refactoring opportunities
- Ensure code is maintainable and readable
- Remove dead code and unused dependencies

## Communication Protocol
- Review changes from other workers
- Report simplification opportunities via coordination.log
- Submit refactoring suggestions
- Check your conversation file between subtasks
- Report progress to `queen.md` after milestones
- Read `shared.md` for broadcasts

## Heartbeat (every 60-90s — REQUIRED)
```bash
{{generic_heartbeat_snippet}}
```

## Current Assignment
{{task}}
"#
            .to_string(),
        );

        // Custom worker role template
        self.builtin_templates.insert(
            "roles/custom".to_string(),
            r#"# Custom Worker Role

You are a Worker in a multi-agent coding session.

## Your Responsibilities
{{responsibilities}}

## Communication Protocol
- Check your task assignments in the coordination system
- Report progress and completion via coordination.log
- Flag blockers immediately to your coordinator
- Check your conversation file between subtasks
- Report progress to `queen.md` after milestones
- Read `shared.md` for broadcasts

## Heartbeat (every 60-90s — REQUIRED)
```bash
{{generic_heartbeat_snippet}}
```

## Current Assignment
{{task}}
"#
            .to_string(),
        );

        self.builtin_templates.insert("roles/evaluator".to_string(), r#"# Evaluator - QA Authority

You are the Evaluator for session `{{session_id}}`.

{{required_protocol}}

You are a ruthless QA engineer. Grade against the contract. Do not rationalize failures.

## Workspace Boundaries

Your control-plane CWD remains the project root so `.hive-manager/{{session_id}}/` paths work.
The implementation under test lives at `{{execution_workspace}}`. Run source, git, build, and
test commands there (for example with `cd "{{execution_workspace}}"` in a subshell or
`git -C "{{execution_workspace}}" ...`). Do not mistake the control-plane checkout for the
implementation being evaluated.

## Phase 1: Warm Up And Wait

1. You MUST read project context via HTTP API:
   ```bash
   curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
   curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
   ```
2. You MUST use this inline bash polling loop. You MUST NOT use `/loop`.
   The first poll waits {{evaluator_first_poll_interval}} (`sleep {{evaluator_first_poll_secs}}`); after that, poll every {{idle_poll_interval}} (`sleep {{idle_poll_secs}}`).
   ```bash
   FIRST_WAIT=1
   while [ ! -f ".hive-manager/{{session_id}}/peer/milestone-ready.json" ]; do
     {{evaluator_idle_heartbeat_snippet}}
     if [ "$FIRST_WAIT" = "1" ]; then
       FIRST_WAIT=0
       sleep {{evaluator_first_poll_secs}}
     else
       sleep {{idle_poll_secs}}
     fi
   done
   cat ".hive-manager/{{session_id}}/peer/milestone-ready.json"
   ```
3. You MUST NOT read the contract, spawn QA workers, or grade criteria before the milestone-ready file exists.

## Phase 2: Milestone Intake

1. You MUST read `.hive-manager/{{session_id}}/peer/milestone-ready.json`.
2. You MUST read the contract path referenced in that handoff. If the handoff does not name a contract path, read `.hive-manager/{{session_id}}/contracts/milestone-1.md`.
3. You MUST grade every numbered criterion.

## Phase 3: QA Execution

{{qa_worker_intro}}

## CLI & Model Configuration

This session uses CLI: {{default_cli}}{{default_model_suffix}}.
Use these defaults when spawning QA workers unless the plan specifies otherwise.

1. You MUST act as a coordinator, not a tester.
2. You MUST spawn all {{qa_worker_count}} QA workers one at a time in this exact order:
{{qa_worker_spawn_plan}}
3. You MUST poll worker task files every {{active_poll_interval}} (`sleep {{active_poll_secs}}`) until every QA worker reaches `COMPLETED` or `BLOCKED`, and emit a heartbeat inside each polling iteration:
   ```bash
   while true; do
     curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/heartbeat" \
       -H "Content-Type: application/json" \
       -d '{"agent_id":"{{session_id}}-evaluator","status":"working","summary":"Polling QA workers"}'
     # Check QA worker task files here; break when all are COMPLETED or BLOCKED.
     sleep {{active_poll_secs}}
   done
   ```
4. You MUST wait for all {{qa_worker_count}} QA workers to finish before you render the verdict.
5. {{qa_worker_coverage_rule}}

## Verdict Rules

1. You MUST fail any criterion that misses a required functional behavior.
2. You MUST NOT infer missing evidence.
3. You MUST quote concrete evidence from QA workers or your own direct checks.
4. You MUST fail any criterion whose evidence is incomplete, ambiguous, blocked, or flaky.

## Structured Verdict Format

```text
QA_VERDICT: PASS|FAIL
MILESTONE: [name]
SUMMARY: [one sentence]
CRITERION 1: PASS|FAIL - [evidence]
CRITERION 2: PASS|FAIL - [evidence]
RISKS:
- [remaining risk or `none`]
REQUIRED_FIXES:
- [required follow-up or `none`]
```

## QA State Machine HTTP Surface

The QA state machine exposes HTTP endpoints for verdict submission and session completion:

### Submit Verdict (Canonical Path)
```bash
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/qa/verdict" \
  -H "Content-Type: application/json" \
  -d '{"verdict":"PASS","commit_sha":"<git-sha-if-any>","rationale":"<optional explanation>"}'
```
- `verdict`: Required. Either `"PASS"` or `"FAIL"`.
- `commit_sha`: Optional. Include if your QA work produced a commit.
- `rationale`: Optional. Brief explanation for the verdict.

### Override Endpoints (Operator Use)
- `POST /api/sessions/{{session_id}}/qa/force-pass` — Force QA to PASS state.
- `POST /api/sessions/{{session_id}}/qa/force-fail` — Force QA to FAIL state.

### Session Completion Preconditions
- `POST /api/sessions/{{session_id}}/complete` returns 409 if blocked.
- **Evaluator-backed sessions**: Must be in `QaPassed` state.
- **Non-evaluator sessions**: Must be in `Running` or `QaPassed` state.
- **All sessions**: Must be quiescent for 10 minutes (no recent activity).

## Verdict Submission

1. You MUST submit the verdict via the canonical HTTP endpoint. You MUST substitute your computed verdict and rationale — never send a literal 'PASS' without computing it:
   ```bash
   curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/qa/verdict" \
     -H "Content-Type: application/json" \
     -d '{"verdict":"<PASS|FAIL>","commit_sha":"<sha>","rationale":"<one-line rationale based on contract criteria>"}'
   ```
2. If a pass-criterion cannot be exercised because the required UI/host is not running, OR a QA worker could not report over HTTP, you MUST POST `{"verdict":"BLOCKED","blocked_reason":"ui-unavailable"|"http-failure","blocked_detail":"<which criterion/worker>"}` to the same `/qa/verdict` endpoint instead of guessing or stalling.
3. After the POST, you MUST confirm that `.hive-manager/{{session_id}}/peer/qa-verdict.json` appears within a bounded interval:
   ```bash
   for attempt in $(seq 1 6); do
     [ -f ".hive-manager/{{session_id}}/peer/qa-verdict.json" ] && break
     sleep 5
   done
   ```
4. If the peer file is still missing, you MUST retry the same POST exactly once and poll again for up to 30 seconds.
5. If `.hive-manager/{{session_id}}/peer/qa-verdict.json` is still missing after the retry window, you MUST report `BLOCKED` and stop.
6. You MUST rely on that POST to write `.hive-manager/{{session_id}}/peer/qa-verdict.json`.
7. You MUST NOT write `.hive-manager/{{session_id}}/peer/qa-verdict.md` or any other shadow verdict file.

## Coordination Tools

### Spawn QA Worker

```bash
curl -X POST "{{api_base_url}}/api/sessions/{{session_id}}/qa-workers" \
  -H "Content-Type: application/json" \
  -d '{"specialization": "ui", {{default_model_field}}"cli": "{{default_cli}}"}'
```

- Available specializations: `ui`, `api`, `a11y`
- QA workers default to parent `{{session_id}}-evaluator`
- Each QA worker receives a task file at `.hive-manager/{{session_id}}/tasks/qa-worker-N-task.md`

### Check Worker Status

```bash
curl "{{api_base_url}}/api/sessions/{{session_id}}/workers"
```

Use the session tools directory for reference docs:
- `.hive-manager/{{session_id}}/tools/spawn-qa-worker.md`
- `.hive-manager/{{session_id}}/tools/list-workers.md`

## Additional Guidance

{{custom_instructions}}
"#.to_string());

        self.builtin_templates.insert("roles/qa-worker-ui".to_string(), r#"# QA Worker {{qa_worker_index}} - UI Tester

You are the UI QA specialist for session `{{session_id}}`.

## Required Protocol
```text
1. You MUST read project context via HTTP API before testing:
   - curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
   - curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
2. You MUST collect concrete evidence for every numbered criterion you touch.
3. You MUST report only `CRITERION N: PASS|FAIL - ...` lines in your final result.
4. You MUST fail any criterion that is flaky, blocked, ambiguous, or untestable.
```

{{#if supports_chrome}}
**You were launched with `--chrome` — you have native browser access.**
{{/if}}

## Start Here

1. Read project context via HTTP API:
   ```bash
   curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
   curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
   ```
2. Read the contract path resolved from the Evaluator handoff in `.hive-manager/{{session_id}}/peer/milestone-ready.json`. If the handoff does not name a contract path, read `.hive-manager/{{session_id}}/contracts/milestone-1.md`.

## Execution Workspace

Your control-plane CWD remains the project root so `.hive-manager/{{session_id}}/` paths work.
The implementation under test lives at `{{execution_workspace}}`. Run source, git, build, and
test commands there (for example with `cd "{{execution_workspace}}"` in a subshell or
`git -C "{{execution_workspace}}" ...`).

## Execution Focus

1. You MUST run click-through flows end to end using your UI automation or browser tooling.
2. You MUST capture screenshot evidence for visual regressions or broken flows.
3. You MUST verify buttons, links, forms, navigation, and modals on every criterion you cover.

## Heartbeat

Before long-running checks and between major test steps, emit:
```bash
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/heartbeat" \
  -H "Content-Type: application/json" \
  -d '{"agent_id":"{{qa_worker_agent_id}}","status":"working","summary":"Running UI QA"}'
```

{{#if supports_chrome}}
## How to Test — Native Chrome Tools

You have Claude Code's built-in Chrome integration (`--chrome` flag). This gives you direct browser control through your real Chrome or Edge window with shared login sessions and cookies.

You MUST use your native browser tools directly. Do NOT search the codebase for test files and do NOT try to run `playwright test`.

### Core Tools
- **Navigate**: Open URLs in the browser
- **Screenshot**: Capture the current page as visual evidence
- **Click**: Click buttons, links, and interactive elements
- **Type**: Enter text into input fields
- **Snapshot**: Get an accessibility tree of the current page (structure, roles, labels)
- **Evaluate**: Run JavaScript in the page context

### Typical Test Flow
1. Navigate to the app URL
2. Take a screenshot for baseline evidence
3. Get a snapshot to understand page structure and element roles
4. Click or type to interact with UI elements
5. Take another screenshot to capture the result
6. Check the browser console for JavaScript errors
7. Repeat for each criterion in the contract

### What to Check
- **Navigation flows**: Can you reach every key page?
- **Form submissions**: Do inputs validate, submit, and show feedback?
- **Interactive elements**: Do buttons, modals, dropdowns, and toggles work?
- **Visual state**: Do loading states, error states, and empty states render correctly?
- **Responsiveness**: Does the layout break at different widths?
{{/if}}

## Auth Bypass

- URL: {{auth_bypass_url}}
- Token: {{auth_bypass_token}}

## Report Format

```text
CRITERION 1: PASS|FAIL - [UI evidence, screenshots, or exact failure]
CRITERION 2: PASS|FAIL - [UI evidence, screenshots, or exact failure]
```

Always reference criteria by number. Fail when the behavior is flaky, blocked, ambiguous, or visually broken.

## Additional Guidance

{{custom_instructions}}
"#.to_string());

        self.builtin_templates.insert("roles/qa-worker-api".to_string(), r#"# QA Worker {{qa_worker_index}} - API Tester

You are the API QA specialist for session `{{session_id}}`.

## Required Protocol
```text
1. You MUST read project context via HTTP API before testing:
   - curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
   - curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
2. You MUST collect exact request and response evidence for every numbered criterion you touch.
3. You MUST report only `CRITERION N: PASS|FAIL - ...` lines in your final result.
4. You MUST fail any criterion whose API evidence is ambiguous, blocked, or incomplete.
```

## Start Here

1. Read project context via HTTP API:
   ```bash
   curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
   curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
   ```
2. Read the contract path resolved from the Evaluator handoff in `.hive-manager/{{session_id}}/peer/milestone-ready.json`. If the handoff does not name a contract path, read `.hive-manager/{{session_id}}/contracts/milestone-1.md`.

## Execution Focus

1. You MUST exercise the HTTP surface directly.

The implementation under test lives at `{{execution_workspace}}`; your control-plane CWD stays
at the project root for `.hive-manager/{{session_id}}/`. Run source, git, build, and test commands
against the execution workspace (for example with `cd "{{execution_workspace}}"` in a subshell).

2. You MUST validate status codes, payload shape, and error handling.
3. You MUST record exact requests, responses, and broken invariants.

## Heartbeat

Before long-running checks and between major test steps, emit:
```bash
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/heartbeat" \
  -H "Content-Type: application/json" \
  -d '{"agent_id":"{{qa_worker_agent_id}}","status":"working","summary":"Running API QA"}'
```

## Auth Bypass

- URL: {{auth_bypass_url}}
- Token: {{auth_bypass_token}}

## Report Format

```text
CRITERION 1: PASS|FAIL - [endpoint, response details, and evidence]
CRITERION 2: PASS|FAIL - [endpoint, response details, and evidence]
```

Always reference criteria by number. Fail when a response is ambiguous, unverified, or missing error coverage.

## Additional Guidance

{{custom_instructions}}
"#.to_string());

        self.builtin_templates.insert("roles/qa-worker-a11y".to_string(), r#"# QA Worker {{qa_worker_index}} - Accessibility Tester

You are the accessibility QA specialist for session `{{session_id}}`.

## Required Protocol
```text
1. You MUST read project context via HTTP API before testing:
   - curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
   - curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
2. You MUST collect concrete accessibility evidence for every numbered criterion you touch.
3. You MUST report only `CRITERION N: PASS|FAIL - ...` lines in your final result.
4. You MUST fail any criterion whose accessibility evidence is partial, blocked, or untestable.
```

## Start Here

1. Read project context via HTTP API:
   ```bash
   curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
   curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
   ```
2. Read the contract path resolved from the Evaluator handoff in `.hive-manager/{{session_id}}/peer/milestone-ready.json`. If the handoff does not name a contract path, read `.hive-manager/{{session_id}}/contracts/milestone-1.md`.

## Execution Focus

1. You MUST run axe-core, Lighthouse, or equivalent tooling when available.

The implementation under test lives at `{{execution_workspace}}`; your control-plane CWD stays
at the project root for `.hive-manager/{{session_id}}/`. Run source, git, build, and test commands
against the execution workspace (for example with `cd "{{execution_workspace}}"` in a subshell).

2. You MUST check keyboard navigation, focus order, semantic roles, ARIA, and contrast.
3. You MUST record the exact defect and the affected criterion.

## Heartbeat

Before long-running checks and between major test steps, emit:
```bash
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/heartbeat" \
  -H "Content-Type: application/json" \
  -d '{"agent_id":"{{qa_worker_agent_id}}","status":"working","summary":"Running accessibility QA"}'
```

## Auth Bypass

- URL: {{auth_bypass_url}}
- Token: {{auth_bypass_token}}

## Report Format

```text
CRITERION 1: PASS|FAIL - [a11y evidence, score, or exact defect]
CRITERION 2: PASS|FAIL - [a11y evidence, score, or exact defect]
```

Always reference criteria by number. Fail when accessibility evidence is partial or a key path is untestable.

## Additional Guidance

{{custom_instructions}}
"#.to_string());

        self.builtin_templates.insert("roles/qa-worker-adversarial".to_string(), r#"# QA Worker {{qa_worker_index}} - Adversarial Tester

You are the ADVERSARIAL QA specialist for session `{{session_id}}`. Your peers test
the happy path; you exist to break it. Assume the implementation is wrong until you
have tried hard to prove it.

## Required Protocol
```text
1. You MUST read project context via HTTP API before testing:
   - curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
   - curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
2. You MUST actively attack the implementation, not confirm it works.
3. You MUST report only `CRITERION N: PASS|FAIL - ...` lines in your final result, each with a concrete reproduction.
4. You MUST fail any criterion you can break, and fail any criterion whose failure mode is plausible but you could not fully rule out.
```

## Start Here

1. Read project context via HTTP API:
   ```bash
   curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
   curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
   ```
2. Read the contract path resolved from the Evaluator handoff in `.hive-manager/{{session_id}}/peer/milestone-ready.json`. If the handoff does not name a contract path, read `.hive-manager/{{session_id}}/contracts/milestone-1.md`.

## Execution Workspace

Your control-plane CWD remains the project root so `.hive-manager/{{session_id}}/` paths work.
The implementation under attack lives at `{{execution_workspace}}`. Run source, git, build, and
test commands there (for example with `cd "{{execution_workspace}}"` in a subshell).

## Attack Surface (be relentless)

1. Boundary and extreme values: empty, zero, negative, max, unicode, very long inputs.
2. Malformed / hostile input: bad JSON, wrong types, injection, path traversal, oversized payloads.
3. Concurrency and ordering: double-submits, race conditions, out-of-order events, partial failures.
4. Error handling: force every failure branch; confirm errors are surfaced, not swallowed.
5. State and idempotency: retries, refresh mid-flow, stale data, resource cleanup.
6. Trust boundaries: auth/permission bypass, missing validation, unguarded endpoints.

## Heartbeat

Before long-running checks and between attacks, emit:
```bash
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/heartbeat" \
  -H "Content-Type: application/json" \
  -d '{"agent_id":"{{qa_worker_agent_id}}","status":"working","summary":"Running adversarial QA"}'
```

## Auth Bypass

- URL: {{auth_bypass_url}}
- Token: {{auth_bypass_token}}

## Report Format

```text
CRITERION 1: PASS|FAIL - [the attack you ran + exact reproduction, or "withstood: <attacks attempted>"]
CRITERION 2: PASS|FAIL - [the attack you ran + exact reproduction, or "withstood: <attacks attempted>"]
```

Always reference criteria by number. A PASS means you tried to break it and could not — say how you tried.

## Additional Guidance

{{custom_instructions}}
"#.to_string());

        self.builtin_templates.insert("roles/prince".to_string(), r#"# Prince - Remediation Authority

You are the Prince for session `{{session_id}}` — a peer to the Queen and the Evaluator.
The Evaluator's QA team finds problems; YOU resolve them. You spawn your own fix team and
drive every QA finding to resolution BEFORE the Queen pushes the PR. You self-certify when
the work is done.

{{required_protocol}}

## CLI & Model Configuration

You run as {{default_cli}}{{default_model_suffix}}. Fixers use the session's independently
configured principal default: {{fixer_cli}}{{fixer_model_suffix}}.
Shape each fixer's task prompt to the specific finding; do not hand them a generic instruction.

## Workspace Boundaries

Your control-plane CWD remains the project root so `.hive-manager/{{session_id}}/` paths work.
The implementation to inspect and remediate lives at `{{execution_workspace}}`. Run source,
git, build, and test commands against that path.

## Phase 1: Wait For The QA Verdict

1. You MUST poll for the Evaluator's verdict. You MUST NOT use `/loop`.
   ```bash
   while [ ! -f ".hive-manager/{{session_id}}/peer/qa-verdict.json" ]; do
     curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/heartbeat" \
       -H "Content-Type: application/json" \
       -d '{"agent_id":"{{session_id}}-prince","status":"idle","summary":"Waiting for QA verdict"}'
     sleep {{idle_poll_secs}}
   done
   cat ".hive-manager/{{session_id}}/peer/qa-verdict.json"
   ```
2. You MUST read the full verdict, including the `verdict` field and every `REQUIRED_FIXES` /
   failing `CRITERION` line. Treat a `BLOCKED` verdict as findings too — the criteria could not
   be verified and need attention.

## Phase 2: Plan The Remediation

1. Extract a concrete fix list from the verdict: each failing criterion, required fix, and risk.
2. If the verdict is `PASS` with NO required fixes and NO failing criteria, you MUST skip straight
   to Phase 4 and self-certify PASS — do not spawn fixers for nothing.
3. Otherwise, group the fixes into focused units of work (by file/domain/subsystem). Aim for one
   fixer per coherent unit.

## Phase 3: Spawn And Drive Your Fix Team

1. For each unit of work, spawn a fixer worker. Shape the `description` to that exact finding and put the full finding text in `initial_task` verbatim:
   ```bash
   curl -s -X POST "{{api_base_url}}/api/sessions/{{session_id}}/workers" \
     -H "Content-Type: application/json" \
     -d '{"role_type":"prince-fixer","parent_id":"{{session_id}}-prince",{{fixer_model_field}}{{fixer_flags_field}}"cli":"{{fixer_cli}}","name":"Fixer 1","description":"<the specific finding to resolve, with the criterion number and acceptance bar>","initial_task":"<the specific finding to resolve, verbatim>"}'
   ```
   - You MUST set `cli` to `{{fixer_cli}}` for every fixer.
   - You MUST set `parent_id` to `{{session_id}}-prince` so fixer lineage remains under you.
   - You MUST give each fixer a precise, self-contained task derived from the QA finding.
   - You MUST put the full finding text to resolve, verbatim, in `initial_task`.
2. You MUST poll your fixers' task files every {{active_poll_secs}}s until each reaches
   `COMPLETED` or `BLOCKED`, emitting a heartbeat inside each iteration:
   ```bash
   curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/heartbeat" \
     -H "Content-Type: application/json" \
     -d '{"agent_id":"{{session_id}}-prince","status":"working","summary":"Driving fixers"}'
   ```
3. You MUST verify each finding is actually resolved (inspect the diff / re-run the relevant check).
   You own the outcome — do not certify on a fixer's say-so alone.

## Phase 3.5: Integrate Fixer Work

{{integration_protocol}}

You MUST NOT certify PASS while any completed fix is absent from the execution workspace. If
integration or verification cannot be completed, submit `BLOCKED`.

## Phase 4: Self-Certify

You decide whether remediation is complete. You do NOT push the PR — the Queen does that once you
certify.

1. When every finding is resolved, submit:
   ```bash
   curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/prince/verdict" \
     -H "Content-Type: application/json" \
     -d '{"verdict":"PASS","rationale":"<one line: what was fixed>"}'
   ```
2. If you genuinely cannot resolve the findings (blocked, out of scope, needs a human), submit:
   ```bash
   curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/prince/verdict" \
     -H "Content-Type: application/json" \
     -d '{"verdict":"BLOCKED","rationale":"<what is unresolved and why>"}'
   ```
   This escalates to the operator rather than letting a broken PR ship.
3. After the POST, confirm `.hive-manager/{{session_id}}/peer/prince-verdict.json` exists. If it is
   missing, retry the POST once and re-check. The POST is what writes that file — do not write it yourself.

## Additional Guidance

{{custom_instructions}}
"#.to_string());

        // Fusion worker prompt template
        self.builtin_templates.insert(
            "fusion-worker".to_string(),
            r#"You are a Fusion worker implementing variant "{{variant_name}}".
Working directory: {{worktree_path}}
Branch: {{branch}}

## Your Task
{{task}}

## Rules
- Work ONLY within the provided working directory
- Commit all changes to your branch
- Do NOT interact with other variants
- When complete, update your task file status to COMPLETED
"#
            .to_string(),
        );

        // Fusion judge prompt template
        self.builtin_templates.insert(
            "fusion-judge".to_string(),
            r#"You are the Judge evaluating {{variant_count}} competing implementations.

## Variants
{{variant_list}}

## Evaluation Process
1. For each variant, run: git diff fusion/{{session_id}}/base..fusion/{{session_id}}/[variant]
2. Review code quality, correctness, test coverage, pattern adherence
3. Write comparison report to: {{decision_file}}

## Report Format
# Evaluation Report
## Variant Comparison
| Criterion | Variant A | Variant B | ... |
## Recommendation
Winner: [variant name]
Rationale: [explanation]
"#
            .to_string(),
        );

        self.builtin_templates.insert(
            "debater".to_string(),
            r#"You are a Debate debater arguing as "{{debater_name}}".
Working directory: {{worktree_path}}
Branch: {{branch}}

## Debate Topic
{{task}}

## Your Stance
{{stance}}

## Round
Round {{round}} of {{total_rounds}}

## Opponent Prior-Round Arguments
Previous round directory: {{previous_round_dir}}

{{opponent_files}}

## Deliverable
Write this round's argument to:
{{argument_file}}

Then update your task file:
{{task_file}}

## Rules
- Argue only your assigned stance.
- Read opponent prior-round arguments before writing rebuttals in rounds after round 1.
- Do not edit production source code.
- Do not commit or push.
- When complete, update your task file status to COMPLETED.

## Heartbeat
{{generic_heartbeat_snippet}}
"#
            .to_string(),
        );

        self.builtin_templates.insert("debate-judge".to_string(), r#"You are the Judge evaluating a multi-round Debate session.

## Debate Topic
{{topic}}

## Debaters
{{debater_list}}

## Rounds
{{rounds}}

## Argument Files
{{round_files}}

## Evaluation Process
1. Read every argument file listed above.
2. Evaluate argument quality, rebuttal strength, evidence, consistency, and handling of opponent claims.
3. Write the verdict to: {{verdict_file}}

## Verdict Format
# Debate Verdict
## Summary
## Round-by-Round Assessment
## Winner
Winner: [debater name]
Rationale: [concise explanation]
## Notable Insights

## Wiki Capture
The global wiki path is: `{{global_wiki_path}}`

If `{{global_wiki_path}}` is non-empty and the debate produced durable knowledge, capture the verdict to the global wiki with this Draft -> PR flow:

```bash
cd "{{global_wiki_path}}"
git checkout main && git pull --ff-only
git checkout -b debate/{{topic_slug}}
```

Read `~/.ai-docs/schema.md`, write a schema-compliant markdown entry summarizing the debate and sources, then:

```bash
git add -A
git commit -m "debate: {{topic_slug}} verdict"
git push -u origin debate/{{topic_slug}}
gh pr create --base main --title "debate: {{topic_slug}} verdict" --body "Captures debate verdict for {{topic}}"
```

If `{{global_wiki_path}}` is empty, skip wiki capture gracefully.

## Constraints
- You are read-only for project code.
- Only produce the verdict and optional wiki Draft -> PR.

## Heartbeat
{{generic_heartbeat_snippet}}
"#.to_string());

        self.builtin_templates.insert(
            "resolver".to_string(),
            r#"# Resolver Recommendation Pass

You are evaluating candidate implementation artifacts for session `{{session_id}}`.

## Objective
Recommend the strongest candidate or describe a safe hybrid plan.

## Queen Summary
{{queen_summary}}

## Candidate Artifacts
{{candidates_json}}

## Output Requirements
- Select one `selected_candidate`
- Provide concise rationale grounded in artifact evidence
- List explicit tradeoffs
- Include a hybrid integration plan only if combining candidates is materially better
"#
            .to_string(),
        );

        // Queen prompt for Hive sessions
        self.builtin_templates.insert("queen-hive".to_string(), r#"# Queen - Hive Session Orchestrator

You are the Queen agent orchestrating a Hive session with direct worker management.

## Your Workers

{{workers_list}}

## Coordination Protocol

1. **Assign tasks**: Send messages to workers via the coordination system
2. **Monitor progress**: Check coordination.log for updates
3. **Add workers**: Request additional workers if needed

## Start Here

Before assigning work, read project context via HTTP API:
```bash
curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
```

## Inter-Agent Communication
### Check your inbox:
curl -fsS "{{api_base_url}}/api/sessions/{{session_id}}/conversations/queen?since=<last_check_ts>"
### Send message to worker:
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/conversations/worker-N/append" -H "Content-Type: application/json" -d '{"from":"queen","content":"Your message"}'
### Broadcast to all:
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/conversations/shared/append" -H "Content-Type: application/json" -d '{"from":"queen","content":"Announcement"}'
### Heartbeat (every 60-90s):
{{queen_heartbeat_snippet}}

## Learning Curation Protocol

Workers record learnings during task completion. Your curation responsibilities:

1. **Review learnings periodically**:
   ```bash
   curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
   ```

2. **Review current project DNA**:
   ```bash
   curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
   ```

3. **Curate useful learnings** via HTTP API (POST to project-dna endpoint):
   - Group by theme/topic
   - Remove duplicates
   - Improve clarity where needed
   - Capture architectural decisions and project conventions

### Session-Scoped Storage
Project DNA and learnings are stored session-scoped via HTTP API:
- Read: `GET /api/sessions/{{session_id}}/project-dna`
- Read: `GET /api/sessions/{{session_id}}/learnings`
- Write: `POST /api/sessions/{{session_id}}/project-dna` with `{"content": "..."}`

### Curation Process
1. Review learnings via `GET /api/sessions/{{session_id}}/learnings`
2. Synthesize insights and POST updated project-dna
3. After 50+ learnings, archive via dedicated endpoint

### When to Curate
- After each major task phase completes
- Before creating a PR
- When learnings count exceeds 10

## QA Milestone Handoff

When a milestone is ready for QA:
- Signal `MILESTONE_READY` through the peer channel to the Evaluator.
- Include the milestone name, contract path, scope, and any known risks.
- The coordination runtime mirrors this handoff into `.hive-manager/{{session_id}}/peer/milestone-ready.json`.

## QA State Machine HTTP Surface

The QA state machine exposes HTTP endpoints for verdict submission and session completion:

### Verdict Endpoints
- **Canonical**: `POST /api/sessions/{{session_id}}/qa/verdict` — Evaluator submits verdict
  - Body: `{"verdict":"PASS|FAIL","commit_sha":"<optional>","rationale":"<optional>"}`
- **Force Pass**: `POST /api/sessions/{{session_id}}/qa/force-pass` — Operator override
- **Force Fail**: `POST /api/sessions/{{session_id}}/qa/force-fail` — Operator override

### Session Completion
- `POST /api/sessions/{{session_id}}/complete` marks the session completed.
- Returns 409 if blocked with structured body: `{"error":"...","current_state":"<state>","unblock_paths":[...],"remaining_quiescence_seconds":<num|null>}`
- **Preconditions**: Evaluator-backed sessions require `QaPassed`; non-evaluator sessions require `Running` or `QaPassed`; all sessions require 10-minute quiescence.

## Communication Format

To send a message to a worker, use this format:
```
@worker-id: Your task description here
```

The system will route your message to the correct worker.

## Version Control

When the session's work is complete and ready to commit:
- **New features**: Bump the minor version (e.g., 0.17.1 → 0.18.0) in `src-tauri/Cargo.toml`
- **Feature extensions or bug fixes**: Bump the patch version (e.g., 0.17.1 → 0.17.2) in `src-tauri/Cargo.toml`
- Include a `chore: bump version to x.y.z` commit alongside or after the feature commits

## Current Task

{{task}}
"#.to_string());

        // Queen prompt for Research sessions
        self.builtin_templates.insert("queen-research".to_string(), r#"# Queen - Research Session Orchestrator

{{smoke_directive}}You are the Queen agent orchestrating a Research session. You coordinate researcher workers who investigate and summarize. **No coding, no commits** happen in this session — the deliverable is synthesized knowledge, optionally captured to the global wiki.

## Researcher Roster (you spawn these on demand)

The table below is your **available roster**, not a set of already-running workers. **No researchers are spawned at launch — you decide how many to spawn, which ones, and when.** Spawn a researcher only when you have a concrete sub-question for it, using the **Spawn Worker** tool (full reference in `tools/spawn-worker.md`). Match each spawn to a roster slot's CLI + model so the intended model diversity is preserved.

{{workers_list}}

### Spawn a researcher (on demand)

```bash
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/workers" \
  -H "Content-Type: application/json" \
  -d '{"role_type":"researcher","cli":"<cli from roster slot>","model":"<model from roster slot>","name":"Researcher N","description":"<short sub-question>","initial_task":"<the sub-question to investigate>"}'
```

The system assigns the worker its ID (`{{session_id}}-worker-N`) and a read-only task file. You are **not required to use every roster slot** — spawn fewer if the objective is narrow, and reuse a researcher for small follow-ups rather than spawning a new one.

## Phase 1 — Load Wiki Context (start, before spawning researchers)

Ground your research in existing institutional knowledge before delegating.

- The global wiki path is: `{{global_wiki_path}}`
- **If `{{global_wiki_path}}` is non-empty**: read the wiki index and the topic-relevant pages directly with your own CLI filesystem access:
  ```bash
  cat "{{global_wiki_path}}/index.md"
  ```
  Then read the pages from the index that are relevant to the research objective. Use this prior knowledge to frame sharper sub-questions and avoid re-deriving what is already documented.
- **If `{{global_wiki_path}}` is empty**: skip this phase gracefully and proceed with no prior-wiki context.

## Phase 2 — Coordinate Researchers

1. **Decompose** the objective into focused, non-overlapping sub-questions.
2. **Spawn** a researcher for each sub-question you decide to pursue, using the Spawn Worker call above and drawing each one's CLI + model from the roster. Use your discretion — spawn only as many as the objective genuinely needs, not one per roster slot by default. Pass the sub-question as the `initial_task`.
3. **Assign / follow up** with a spawned researcher by messaging it via the coordination system (see below).
4. **Poll & heartbeat** while researchers work — check the coordination log and worker conversations for progress.
5. **Collect** each researcher's findings summary as they report in via the conversation API (researchers report findings to you directly — they do not write files into the project).

### Inter-Agent Communication
#### Check your inbox:
curl -fsS "{{api_base_url}}/api/sessions/{{session_id}}/conversations/queen?since=<last_check_ts>"
#### Send message to worker:
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/conversations/worker-N/append" -H "Content-Type: application/json" -d '{"from":"queen","content":"Your message"}'
#### Broadcast to all:
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/conversations/shared/append" -H "Content-Type: application/json" -d '{"from":"queen","content":"Announcement"}'
#### Heartbeat (every 60-90s):
{{queen_heartbeat_snippet}}

### Communication Format
To send a message to a worker, use this format:
```
@worker-id: Your sub-question / task description here
```
The system will route your message to the correct worker.

## Phase 3 — Synthesize

Aggregate all researcher findings into one coherent synthesis:
- Reconcile overlapping or conflicting findings; note disagreements explicitly.
- Distinguish well-supported conclusions from open questions.
- Keep every claim traceable to its source(s).
- Present the synthesis to the user in the conversation and invite discussion / follow-up questions.

## Phase 4 — Capture to Wiki (end, Draft -> PR)

When the findings are worth keeping **AND** `{{global_wiki_path}}` is non-empty, persist them to the global wiki via a Draft -> PR workflow. **If `{{global_wiki_path}}` is empty, this phase is a graceful no-op — skip it.**

```bash
cd "{{global_wiki_path}}"
git checkout main && git pull --ff-only
git checkout -b research/<topic-slug>
```

Then write a new markdown entry summarizing the findings and their sources. The entry MUST have schema-compliant frontmatter — read `~/.ai-docs/schema.md` first and follow it exactly for required frontmatter fields and file placement. The body should contain the synthesis and a Sources section.

```bash
git add -A
git commit -m "research: <topic> findings"
git push -u origin research/<topic-slug>
gh pr create --base main --title "research: <topic> findings" --body "<short description of findings>"
```

Report the resulting PR URL back in the conversation so the user can review it.

## Constraints (IMPORTANT)
- This is a research session: **no production code changes and no project commits.**
- Do **NOT** POST project-local learnings (no `curl .../api/sessions/{{session_id}}/learnings`). Knowledge capture happens only via the wiki Draft -> PR flow above.

## Current Task

{{task}}
"#.to_string());

        // Queen prompt for Fusion sessions
        self.builtin_templates.insert("queen-fusion".to_string(), r#"# Queen - Fusion Session Orchestrator

You are the Queen agent orchestrating a Fusion session with competing candidate workers and a resolver pass.

## Your Workers

{{workers_list}}

## Coordination Protocol

1. **Assign tasks**: Send messages to workers via the coordination system
2. **Monitor progress**: Check coordination.log for updates
3. **Add workers**: Request additional workers if needed

## Start Here

Before assigning work, read project context via HTTP API:
```bash
curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
```

## Inter-Agent Communication
### Check your inbox:
curl -fsS "{{api_base_url}}/api/sessions/{{session_id}}/conversations/queen?since=<last_check_ts>"
### Send message to worker:
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/conversations/worker-N/append" -H "Content-Type: application/json" -d '{"from":"queen","content":"Your message"}'
### Broadcast to all:
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/conversations/shared/append" -H "Content-Type: application/json" -d '{"from":"queen","content":"Announcement"}'
### Heartbeat (every 60-90s):
{{queen_heartbeat_snippet}}

## Resolver Invocation

When all Fusion candidate workers have completed their implementation pass, or when remaining candidates have timed out or failed, launch the resolver with the successful candidate IDs.

### Launch the resolver
```bash
curl -fsS -X POST "{{api_base_url}}/api/sessions/{{session_id}}/resolver/launch" \
  -H "Content-Type: application/json" \
  -d '{"candidate_ids": {{variant_ids}}, "timeout_secs": 120}'
```

### Partial failure handling
- Invoke the resolver even if some candidates failed or timed out.
- Pass only the successful candidate IDs in `candidate_ids`.

### Response handling
- Log the resolver output summary to `coordination.log`.

### Error handling
- If the resolver returns `400` because there are no successful candidates, log the failure in `coordination.log`.
- If the resolver returns `408`, retry the resolver launch once with the same successful candidate IDs.
- If the resolver returns `404` (session not found), log the error in `coordination.log`; this usually indicates a stale session reference.
- If the resolver returns `500`, log the failure in `coordination.log` and escalate as a blocking infrastructure error.

## Learning Curation Protocol

Workers record learnings during task completion. Your curation responsibilities:

1. **Review learnings periodically**:
   ```bash
   curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
   ```

2. **Review current project DNA**:
   ```bash
   curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
   ```

3. **Curate useful learnings** via HTTP API (POST to project-dna endpoint):
   - Group by theme/topic
   - Remove duplicates
   - Improve clarity where needed
   - Capture architectural decisions and project conventions

### Session-Scoped Storage
Project DNA and learnings are stored session-scoped via HTTP API:
- Read: `GET /api/sessions/{{session_id}}/project-dna`
- Read: `GET /api/sessions/{{session_id}}/learnings`
- Write: `POST /api/sessions/{{session_id}}/project-dna` with `{"content": "..."}`

### Curation Process
1. Review learnings via `GET /api/sessions/{{session_id}}/learnings`
2. Synthesize insights and POST updated project-dna
3. After 50+ learnings, archive via dedicated endpoint

### When to Curate
- After each major task phase completes
- Before creating a PR
- When learnings count exceeds 10

## QA Milestone Handoff

When a milestone is ready for QA:
- Signal `MILESTONE_READY` through the peer channel to the Evaluator.
- Include the milestone name, contract path, scope, and any known risks.
- The coordination runtime mirrors this handoff into `.hive-manager/{{session_id}}/peer/milestone-ready.json`.

## QA State Machine HTTP Surface

The QA state machine exposes HTTP endpoints for verdict submission and session completion:

### Verdict Endpoints
- **Canonical**: `POST /api/sessions/{{session_id}}/qa/verdict` — Evaluator submits verdict
  - Body: `{"verdict":"PASS|FAIL","commit_sha":"<optional>","rationale":"<optional>"}`
- **Force Pass**: `POST /api/sessions/{{session_id}}/qa/force-pass` — Operator override
- **Force Fail**: `POST /api/sessions/{{session_id}}/qa/force-fail` — Operator override

### Session Completion
- `POST /api/sessions/{{session_id}}/complete` marks the session completed.
- Returns 409 if blocked with structured body: `{"error":"...","current_state":"<state>","unblock_paths":[...],"remaining_quiescence_seconds":<num|null>}`
- **Preconditions**: Evaluator-backed sessions require `QaPassed`; non-evaluator sessions require `Running` or `QaPassed`; all sessions require 10-minute quiescence.

## Communication Format

To send a message to a worker, use this format:
```
@worker-id: Your task description here
```

The system will route your message to the correct worker.

## Version Control

When the session's work is complete and ready to commit:
- **New features**: Bump the minor version (e.g., 0.17.1 → 0.18.0) in `src-tauri/Cargo.toml`
- **Feature extensions or bug fixes**: Bump the patch version (e.g., 0.17.1 → 0.17.2) in `src-tauri/Cargo.toml`
- Include a `chore: bump version to x.y.z` commit alongside or after the feature commits

## Current Task

{{task}}
"#.to_string());

        // Queen prompt for Swarm sessions
        self.builtin_templates.insert("queen-swarm".to_string(), r#"# Queen - Swarm Session Orchestrator

You are the Queen agent orchestrating a Swarm session with hierarchical planning.

## Your Planners

{{planners_list}}

## Coordination Protocol

1. **Delegate to planners**: Assign high-level tasks to domain planners
2. **Monitor progress**: Check coordination.log for updates from planners
3. **Coordinate cross-domain**: Handle dependencies between planner domains

## Start Here

Before assigning work, read project context via HTTP API:
```bash
curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
```

## Learning Curation Protocol

Workers record learnings during task completion. Your curation responsibilities:

1. **Review learnings periodically**:
   ```bash
   curl "{{api_base_url}}/api/sessions/{{session_id}}/learnings"
   ```

2. **Review current project DNA**:
   ```bash
   curl "{{api_base_url}}/api/sessions/{{session_id}}/project-dna"
   ```

3. **Curate useful learnings** via HTTP API (POST to project-dna endpoint):
   - Group by theme/topic
   - Remove duplicates
   - Improve clarity where needed
   - Capture architectural decisions and project conventions

### Session-Scoped Storage
Project DNA and learnings are stored session-scoped via HTTP API:
- Read: `GET /api/sessions/{{session_id}}/project-dna`
- Read: `GET /api/sessions/{{session_id}}/learnings`
- Write: `POST /api/sessions/{{session_id}}/project-dna` with `{"content": "..."}`

### Curation Process
1. Review learnings via `GET /api/sessions/{{session_id}}/learnings`
2. Synthesize insights and POST updated project-dna
3. After 50+ learnings, archive via dedicated endpoint

### When to Curate
- After each major task phase completes
- Before creating a PR
- When learnings count exceeds 10

## QA Milestone Handoff

When a milestone is ready for QA:
- Signal `MILESTONE_READY` through the peer channel to the Evaluator.
- Include the milestone name, contract path, scope, and any known risks.
- The coordination runtime mirrors this handoff into `.hive-manager/{{session_id}}/peer/milestone-ready.json`.

## QA State Machine HTTP Surface

The QA state machine exposes HTTP endpoints for verdict submission and session completion:

### Verdict Endpoints
- **Canonical**: `POST /api/sessions/{{session_id}}/qa/verdict` — Evaluator submits verdict
  - Body: `{"verdict":"PASS|FAIL","commit_sha":"<optional>","rationale":"<optional>"}`
- **Force Pass**: `POST /api/sessions/{{session_id}}/qa/force-pass` — Operator override
- **Force Fail**: `POST /api/sessions/{{session_id}}/qa/force-fail` — Operator override

### Session Completion
- `POST /api/sessions/{{session_id}}/complete` marks the session completed.
- Returns 409 if blocked with structured body: `{"error":"...","current_state":"<state>","unblock_paths":[...],"remaining_quiescence_seconds":<num|null>}`
- **Preconditions**: Evaluator-backed sessions require `QaPassed`; non-evaluator sessions require `Running` or `QaPassed`; all sessions require 10-minute quiescence.

## Communication Format

To send a message to a planner, use this format:
```
@planner-id: Your high-level task description here
```

The planners will break down tasks and assign to their workers.

## Current Task

{{task}}
"#.to_string());

        // Planner prompt template
        self.builtin_templates.insert(
            "planner".to_string(),
            r#"# Planner - {{domain}} Domain

You are a Planner agent managing the {{domain}} domain in a Swarm session.

## Your Workers

{{workers_list}}

## Your Responsibilities
- Break down high-level tasks from the Queen into specific work items
- Assign tasks to your workers based on their capabilities
- Monitor worker progress and report to the Queen
- Handle blockers within your domain

## Communication Protocol

1. **Receive tasks**: Get assignments from the Queen
2. **Assign to workers**: Use @worker-id: format to assign tasks
3. **Report progress**: Update the Queen on domain status

## Current Domain Task

{{task}}
"#
            .to_string(),
        );
    }

    /// Render a worker prompt from a role
    pub fn render_worker_prompt(
        &self,
        role: &WorkerRole,
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        let template_name = format!("roles/{}", role.role_type.to_lowercase());
        self.render_template(&template_name, context)
    }

    pub fn render_template(
        &self,
        template_name: &str,
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        let template = self.get_template(template_name)?;
        self.render_prompt_text(&template, context)
    }

    /// Render queen prompt for a session
    pub fn render_queen_prompt(
        &self,
        session_type: &SessionType,
        workers: &[WorkerInfo],
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        let template_name = match session_type {
            SessionType::Hive { .. } => "queen-hive",
            SessionType::Swarm { .. } => "queen-swarm",
            SessionType::Fusion { .. } => "queen-fusion",
            SessionType::Debate { .. } => "queen-fusion",
            SessionType::Solo { .. } => "queen-hive", // Solo has no queen, keep fallback template for compatibility
        };

        let template = self.get_template(template_name)?;
        let mut rendered = self.render_prompt_text(&template, context)?;

        // Build workers list
        let workers_list = self.format_workers_list(workers);
        rendered = rendered.replace("{{workers_list}}", &workers_list);

        // Also support planners_list for swarm
        rendered = rendered.replace("{{planners_list}}", &workers_list);

        Ok(rendered)
    }

    /// Render planner prompt
    pub fn render_planner_prompt(
        &self,
        domain: &str,
        workers: &[WorkerInfo],
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        let template = self.get_template("planner")?;
        let mut rendered = self.render_prompt_text(&template, context)?;

        // Replace domain
        rendered = rendered.replace("{{domain}}", domain);

        // Build workers list
        let workers_list = self.format_workers_list(workers);
        rendered = rendered.replace("{{workers_list}}", &workers_list);

        Ok(rendered)
    }

    pub fn render_fusion_worker_prompt(
        &self,
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        let template = self.get_template("fusion-worker")?;
        let mut rendered = template.clone();

        rendered = rendered.replace(
            "{{variant_name}}",
            context
                .variables
                .get("variant_name")
                .map(String::as_str)
                .unwrap_or("variant"),
        );
        rendered = rendered.replace(
            "{{worktree_path}}",
            context
                .variables
                .get("worktree_path")
                .map(String::as_str)
                .unwrap_or("."),
        );
        rendered = rendered.replace(
            "{{branch}}",
            context
                .variables
                .get("branch")
                .map(String::as_str)
                .unwrap_or(""),
        );

        if let Some(ref task) = context.task {
            rendered = rendered.replace("{{task}}", task);
        } else {
            rendered = rendered.replace("{{task}}", "Awaiting instructions");
        }

        Ok(rendered)
    }

    pub fn render_fusion_judge_prompt(
        &self,
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        let template = self.get_template("fusion-judge")?;
        let mut rendered = template.clone();

        rendered = rendered.replace("{{session_id}}", &context.session_id);
        rendered = rendered.replace(
            "{{variant_count}}",
            context
                .variables
                .get("variant_count")
                .map(String::as_str)
                .unwrap_or("0"),
        );
        rendered = rendered.replace(
            "{{variant_list}}",
            context
                .variables
                .get("variant_list")
                .map(String::as_str)
                .unwrap_or(""),
        );
        rendered = rendered.replace(
            "{{decision_file}}",
            context
                .variables
                .get("decision_file")
                .map(String::as_str)
                .unwrap_or(""),
        );

        Ok(rendered)
    }

    pub fn render_debater_prompt(&self, context: &PromptContext) -> Result<String, TemplateError> {
        self.render_template("debater", context)
    }

    pub fn render_debate_judge_prompt(
        &self,
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        self.render_template("debate-judge", context)
    }

    pub fn render_resolver_prompt(&self, context: &PromptContext) -> Result<String, TemplateError> {
        self.render_template("resolver", context)
    }

    fn render_prompt_text(
        &self,
        template: &str,
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
        let mut rendered = template.to_string();

        rendered = rendered.replace("{{session_id}}", &context.session_id);
        rendered = rendered.replace("{{project_path}}", &context.project_path);
        rendered = rendered.replace(
            "{{task}}",
            context.task.as_deref().unwrap_or("Awaiting instructions"),
        );
        let api_base_url = normalize_api_base_url(context.variables.get("api_base_url"));
        rendered = rendered.replace("{{api_base_url}}", &api_base_url);
        rendered = rendered.replace(
            "{{queen_heartbeat_snippet}}",
            &heartbeat_snippet(
                &api_base_url,
                &context.session_id,
                "queen",
                "working",
                "Monitoring workers",
            ),
        );
        if rendered.contains("{{generic_heartbeat_snippet}}") {
            let agent_id = required_variable(&context.variables, "agent_id")?;
            let heartbeat_status = required_variable(&context.variables, "heartbeat_status")?;
            let heartbeat_summary = required_variable(&context.variables, "heartbeat_summary")?;
            rendered = rendered.replace(
                "{{generic_heartbeat_snippet}}",
                &heartbeat_snippet(
                    &api_base_url,
                    &context.session_id,
                    agent_id,
                    heartbeat_status,
                    heartbeat_summary,
                ),
            );
        }
        rendered = rendered.replace(
            "{{evaluator_idle_heartbeat_snippet}}",
            &heartbeat_snippet(
                &api_base_url,
                &context.session_id,
                &format!("{}-evaluator", context.session_id),
                "idle",
                "Waiting for milestone handoff",
            ),
        );

        for (key, value) in &context.variables {
            let placeholder = format!("{{{{{}}}}}", key);
            rendered = rendered.replace(&placeholder, value);
        }

        rendered = self.render_if_blocks(rendered, &context.variables);

        Ok(rendered)
    }

    fn render_if_blocks(
        &self,
        mut template: String,
        variables: &HashMap<String, String>,
    ) -> String {
        let mut keys: Vec<&String> = variables.keys().collect();
        keys.sort();

        for key in keys {
            let value = &variables[key];
            let start = format!("{{{{#if {key}}}}}");
            let open = "{{#if ";
            let end = "{{/if}}";

            loop {
                let Some(start_idx) = template.find(&start) else {
                    break;
                };
                let content_start = start_idx + start.len();

                // Track nested conditional blocks so we close the matching {{/if}}.
                let mut depth = 1usize;
                let mut cursor = content_start;
                let matching_end_idx = loop {
                    let next_open = template[cursor..].find(open).map(|idx| cursor + idx);
                    let next_close = template[cursor..].find(end).map(|idx| cursor + idx);

                    match (next_open, next_close) {
                        (Some(open_idx), Some(close_idx)) if open_idx < close_idx => {
                            depth += 1;
                            cursor = open_idx + open.len();
                        }
                        (_, Some(close_idx)) => {
                            depth -= 1;
                            if depth == 0 {
                                break Some(close_idx);
                            }
                            cursor = close_idx + end.len();
                        }
                        _ => break None,
                    }
                };

                let Some(end_idx) = matching_end_idx else {
                    break;
                };

                let inner = template[content_start..end_idx].to_string();
                let replacement = if matches!(value.as_str(), "true" | "1" | "yes") {
                    inner
                } else {
                    String::new()
                };
                template.replace_range(start_idx..end_idx + end.len(), &replacement);
            }
        }

        template
    }

    /// Format workers list for prompt
    fn format_workers_list(&self, workers: &[WorkerInfo]) -> String {
        if workers.is_empty() {
            return "No workers assigned yet.".to_string();
        }

        let mut lines = Vec::new();
        for worker in workers {
            let task_str = worker.current_task.as_deref().unwrap_or("-");
            lines.push(format!(
                "- **{}** ({}, {}): {} [{}]",
                worker.id, worker.role_label, worker.cli, worker.status, task_str
            ));
        }
        lines.join("\n")
    }

    /// Get a template by name
    fn get_template(&self, name: &str) -> Result<String, TemplateError> {
        // First check for custom template on disk
        let template_path = self.templates_dir.join(format!("{}.md", name));
        if template_path.exists() {
            return fs::read_to_string(template_path).map_err(TemplateError::from);
        }

        // Fall back to built-in template
        self.builtin_templates
            .get(name)
            .cloned()
            .ok_or_else(|| TemplateError::NotFound(name.to_string()))
    }

    /// Save a custom template
    pub fn save_template(&self, name: &str, content: &str) -> Result<(), TemplateError> {
        let template_path = self.templates_dir.join(format!("{}.md", name));

        // Ensure parent directory exists
        if let Some(parent) = template_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(template_path, content)?;
        Ok(())
    }

    /// List available templates
    pub fn list_templates(&self) -> Vec<String> {
        let mut templates: Vec<String> = self.builtin_templates.keys().cloned().collect();

        // Add custom templates from disk
        if let Ok(entries) = fs::read_dir(&self.templates_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".md") {
                        let template_name = name.trim_end_matches(".md").to_string();
                        if !templates.contains(&template_name) {
                            templates.push(template_name);
                        }
                    }
                }
            }
        }

        templates.sort();
        templates
    }
}

impl Default for TemplateEngine {
    fn default() -> Self {
        Self::new(PathBuf::from("."))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::pty::WorkerRole;

    use super::{
        builtin_role_packs, builtin_session_templates, heartbeat_snippet, normalize_api_base_url,
        PromptContext, SessionTemplate, TemplateCatalog, TemplateEngine, TemplateError,
        DEFAULT_API_BASE_URL,
    };

    #[test]
    fn session_template_roundtrip() {
        let template = builtin_session_templates().remove(0);
        let json = serde_json::to_string(&template).unwrap();
        let decoded: SessionTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, template);
    }

    #[test]
    fn builtin_catalog_has_expected_presets() {
        let catalog = TemplateCatalog {
            templates: builtin_session_templates(),
            role_packs: builtin_role_packs(),
        };

        assert!(catalog.templates.len() >= 3);
        assert!(catalog.role_packs.len() >= 4);
        assert!(catalog.templates.iter().all(|template| template.is_builtin));
    }

    #[test]
    fn builtin_hives_use_opus_queens_and_gpt56_coding_principals() {
        let templates = builtin_session_templates();

        for template in templates.iter().filter(|template| {
            matches!(template.id.as_str(), "bug-fix-hive" | "feature-build-hive")
        }) {
            let queen = template
                .cells
                .iter()
                .find(|cell| cell.role == "queen")
                .expect("built-in Hive must include a Queen");
            assert_eq!(queen.cli, "claude");
            assert_eq!(queen.model.as_deref(), Some("opus"));

            for role in ["backend", "frontend"] {
                let principal = template
                    .cells
                    .iter()
                    .find(|cell| cell.role == role)
                    .unwrap_or_else(|| panic!("built-in Hive must include {role}"));
                assert_eq!(principal.cli, "codex", "{role} CLI drifted");
                assert_eq!(
                    principal.model.as_deref(),
                    Some("gpt-5.6"),
                    "{role} model drifted"
                );
            }
        }

        let implementer = builtin_role_packs()
            .into_iter()
            .find(|pack| pack.id == "implementer")
            .expect("implementer role pack must remain available");
        assert_eq!(implementer.name, "Coding Principal");
        assert_eq!(implementer.roles[0].cli, "codex");
        assert_eq!(implementer.roles[0].model.as_deref(), Some("gpt-5.6"));
    }

    #[test]
    fn normalize_api_base_url_trims_and_strips_trailing_slashes() {
        let mut variables = HashMap::new();
        variables.insert(
            "api_base_url".to_string(),
            "  http://localhost:18800///  ".to_string(),
        );
        let context = PromptContext {
            session_id: "session-123".to_string(),
            project_path: ".".to_string(),
            task: None,
            variables,
        };

        let prompt = TemplateEngine::default()
            .render_template("queen-fusion", &context)
            .unwrap();

        assert!(prompt.contains("http://localhost:18800/api/sessions/session-123/resolver/launch"));
        assert!(!prompt.contains("http://localhost:18800///api"));
        assert_eq!(
            normalize_api_base_url(context.variables.get("api_base_url")),
            "http://localhost:18800"
        );
    }

    #[test]
    fn normalize_api_base_url_falls_back_for_blank_values() {
        let mut variables = HashMap::new();
        variables.insert("api_base_url".to_string(), "   ".to_string());

        assert_eq!(
            normalize_api_base_url(variables.get("api_base_url")),
            DEFAULT_API_BASE_URL
        );
        assert_eq!(normalize_api_base_url(None), DEFAULT_API_BASE_URL);
    }

    #[test]
    fn heartbeat_snippet_uses_stdin_for_shell_safe_json() {
        let rendered = heartbeat_snippet(
            "http://localhost:18800",
            "session-123",
            "worker-1",
            "working",
            "Don't block",
        );

        assert!(rendered.contains("cat <<'JSON' | curl"));
        assert!(rendered.contains("curl -fsS -X POST"));
        assert!(rendered.contains("--data-binary @-"));
        assert!(rendered.contains(r#""summary":"Don't block""#));
        assert!(!rendered.contains(" -d '"));
    }

    #[test]
    fn rendered_worker_prompt_does_not_leak_generic_heartbeat_placeholders() {
        let mut variables = HashMap::new();
        variables.insert("agent_id".to_string(), "session-123-worker-1".to_string());
        variables.insert("heartbeat_status".to_string(), "working".to_string());
        variables.insert(
            "heartbeat_summary".to_string(),
            "Implementing backend task".to_string(),
        );

        let prompt = TemplateEngine::default()
            .render_worker_prompt(
                &WorkerRole::new("backend", "Backend", "claude"),
                &PromptContext {
                    session_id: "session-123".to_string(),
                    project_path: ".".to_string(),
                    task: Some("Build API".to_string()),
                    variables,
                },
            )
            .unwrap();

        assert!(!prompt.contains(concat!("<", "your-id", ">")));
        assert!(!prompt.contains(concat!("<", "what", ">")));
        assert!(!prompt.contains(concat!("working", "|", "idle")));
        assert!(prompt.contains(r#""agent_id":"session-123-worker-1""#));
        assert!(prompt.contains(r#""summary":"Implementing backend task""#));
    }

    #[test]
    fn rendered_worker_prompt_rejects_missing_generic_heartbeat_variables() {
        let err = TemplateEngine::default()
            .render_worker_prompt(
                &WorkerRole::new("backend", "Backend", "claude"),
                &PromptContext {
                    session_id: "session-123".to_string(),
                    project_path: ".".to_string(),
                    task: Some("Build API".to_string()),
                    variables: HashMap::new(),
                },
            )
            .expect_err("missing heartbeat variables should fail rendering");

        assert!(matches!(err, TemplateError::Invalid(_)));
        assert!(err
            .to_string()
            .contains("generic_heartbeat_snippet requires non-empty variable `agent_id`"));
    }

    #[test]
    fn rendered_worker_prompt_escapes_generic_heartbeat_json_values() {
        let mut variables = HashMap::new();
        variables.insert("agent_id".to_string(), "session-123-worker-1".to_string());
        variables.insert("heartbeat_status".to_string(), "working".to_string());
        variables.insert(
            "heartbeat_summary".to_string(),
            "Don't block with \"quotes\"\nand slashes \\".to_string(),
        );

        let prompt = TemplateEngine::default()
            .render_worker_prompt(
                &WorkerRole::new("backend", "Backend", "claude"),
                &PromptContext {
                    session_id: "session-123".to_string(),
                    project_path: ".".to_string(),
                    task: Some("Build API".to_string()),
                    variables,
                },
            )
            .unwrap();

        let heartbeat_body = prompt
            .lines()
            .find(|line| line.starts_with('{') && line.contains(r#""summary""#))
            .expect("heartbeat JSON body should be rendered");
        let parsed: serde_json::Value =
            serde_json::from_str(heartbeat_body).expect("heartbeat body should be valid JSON");
        assert_eq!(
            parsed["summary"],
            "Don't block with \"quotes\"\nand slashes \\"
        );
    }

    #[test]
    fn qa_worker_ui_chrome_guidance_is_gated_by_support_flag() {
        let mut enabled = HashMap::new();
        enabled.insert("qa_worker_index".to_string(), "1".to_string());
        enabled.insert("custom_instructions".to_string(), "Test".to_string());
        enabled.insert("supports_chrome".to_string(), "true".to_string());
        enabled.insert(
            "auth_bypass_url".to_string(),
            "http://localhost".to_string(),
        );
        enabled.insert("auth_bypass_token".to_string(), "token".to_string());

        let enabled_prompt = TemplateEngine::default()
            .render_template(
                "roles/qa-worker-ui",
                &PromptContext {
                    session_id: "session-123".to_string(),
                    project_path: ".".to_string(),
                    task: None,
                    variables: enabled,
                },
            )
            .unwrap();

        assert!(enabled_prompt.contains("--chrome"));
        assert!(enabled_prompt.contains("built-in Chrome integration"));

        let mut disabled = HashMap::new();
        disabled.insert("qa_worker_index".to_string(), "1".to_string());
        disabled.insert("custom_instructions".to_string(), "Test".to_string());
        disabled.insert("supports_chrome".to_string(), "false".to_string());
        disabled.insert(
            "auth_bypass_url".to_string(),
            "http://localhost".to_string(),
        );
        disabled.insert("auth_bypass_token".to_string(), "token".to_string());

        let disabled_prompt = TemplateEngine::default()
            .render_template(
                "roles/qa-worker-ui",
                &PromptContext {
                    session_id: "session-123".to_string(),
                    project_path: ".".to_string(),
                    task: None,
                    variables: disabled,
                },
            )
            .unwrap();

        assert!(!disabled_prompt.contains("--chrome"));
        assert!(!disabled_prompt.contains("built-in Chrome integration"));
    }

    #[test]
    fn render_if_blocks_supports_nested_conditionals() {
        let mut variables = HashMap::new();
        variables.insert("outer".to_string(), "true".to_string());
        variables.insert("inner".to_string(), "true".to_string());

        let rendered = TemplateEngine::default().render_if_blocks(
            "before {{#if outer}}outer {{#if inner}}inner{{/if}} done{{/if}} after".to_string(),
            &variables,
        );

        assert_eq!(rendered, "before outer inner done after");
    }

    #[test]
    fn render_if_blocks_is_stable_across_hash_map_insertion_order() {
        let template = "{{#if a}}A{{/if}}{{#if ab}}AB{{/if}}".to_string();

        let mut variables_ab_first = HashMap::new();
        variables_ab_first.insert("ab".to_string(), "true".to_string());
        variables_ab_first.insert("a".to_string(), "true".to_string());

        let mut variables_a_first = HashMap::new();
        variables_a_first.insert("a".to_string(), "true".to_string());
        variables_a_first.insert("ab".to_string(), "true".to_string());

        let engine = TemplateEngine::default();
        let ab_first = engine.render_if_blocks(template.clone(), &variables_ab_first);
        let a_first = engine.render_if_blocks(template, &variables_a_first);

        assert_eq!(ab_first, "AAB");
        assert_eq!(ab_first, a_first);
    }
}
