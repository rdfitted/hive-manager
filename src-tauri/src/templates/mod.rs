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
            description: "Queen-led bug fix session with backend and frontend implementers.".to_string(),
            mode: SessionMode::Hive,
            cells: vec![
                CellTemplate {
                    role: "queen".to_string(),
                    cli: "claude".to_string(),
                    model: Some("opus-4-6".to_string()),
                    prompt_template: "queen-hive".to_string(),
                },
                CellTemplate {
                    role: "backend".to_string(),
                    cli: "codex".to_string(),
                    model: Some("gpt-5.4".to_string()),
                    prompt_template: "roles/backend".to_string(),
                },
                CellTemplate {
                    role: "frontend".to_string(),
                    cli: "gemini".to_string(),
                    model: Some("gemini-2.5-pro".to_string()),
                    prompt_template: "roles/frontend".to_string(),
                },
            ],
            workspace_strategy: WorkspaceStrategy::SharedCell,
            is_builtin: true,
        },
        SessionTemplate {
            id: "feature-build-hive".to_string(),
            name: "Feature-build Hive".to_string(),
            description: "Queen plus backend, frontend, and coherence workers for feature delivery.".to_string(),
            mode: SessionMode::Hive,
            cells: vec![
                CellTemplate {
                    role: "queen".to_string(),
                    cli: "claude".to_string(),
                    model: Some("opus-4-6".to_string()),
                    prompt_template: "queen-hive".to_string(),
                },
                CellTemplate {
                    role: "backend".to_string(),
                    cli: "codex".to_string(),
                    model: Some("gpt-5.4".to_string()),
                    prompt_template: "roles/backend".to_string(),
                },
                CellTemplate {
                    role: "frontend".to_string(),
                    cli: "gemini".to_string(),
                    model: Some("gemini-2.5-pro".to_string()),
                    prompt_template: "roles/frontend".to_string(),
                },
                CellTemplate {
                    role: "coherence".to_string(),
                    cli: "droid".to_string(),
                    model: Some("glm-4.7".to_string()),
                    prompt_template: "roles/coherence".to_string(),
                },
            ],
            workspace_strategy: WorkspaceStrategy::SharedCell,
            is_builtin: true,
        },
        SessionTemplate {
            id: "fusion-compare".to_string(),
            name: "Fusion Compare".to_string(),
            description: "Two candidate implementation cells plus a resolver recommendation pass.".to_string(),
            mode: SessionMode::Fusion,
            cells: vec![
                CellTemplate {
                    role: "candidate-a".to_string(),
                    cli: "codex".to_string(),
                    model: Some("gpt-5.4".to_string()),
                    prompt_template: "fusion-worker".to_string(),
                },
                CellTemplate {
                    role: "candidate-b".to_string(),
                    cli: "gemini".to_string(),
                    model: Some("gemini-2.5-pro".to_string()),
                    prompt_template: "fusion-worker".to_string(),
                },
                CellTemplate {
                    role: "resolver".to_string(),
                    cli: "claude".to_string(),
                    model: Some("opus-4-6".to_string()),
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
                model: Some("opus-4-6".to_string()),
                prompt_template: "queen-hive".to_string(),
            }],
        },
        RolePack {
            id: "implementer".to_string(),
            name: "Implementer".to_string(),
            roles: vec![CellTemplate {
                role: "backend".to_string(),
                cli: "codex".to_string(),
                model: Some("gpt-5.4".to_string()),
                prompt_template: "roles/backend".to_string(),
            }],
        },
        RolePack {
            id: "reviewer".to_string(),
            name: "Reviewer".to_string(),
            roles: vec![CellTemplate {
                role: "coherence".to_string(),
                cli: "droid".to_string(),
                model: Some("glm-4.7".to_string()),
                prompt_template: "roles/coherence".to_string(),
            }],
        },
        RolePack {
            id: "resolver".to_string(),
            name: "Resolver".to_string(),
            roles: vec![CellTemplate {
                role: "resolver".to_string(),
                cli: "claude".to_string(),
                model: Some("opus-4-6".to_string()),
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
        self.builtin_templates.insert("roles/backend".to_string(), r#"# Backend Worker Role

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

## Current Assignment
{{task}}
"#.to_string());

        // Frontend worker role template
        self.builtin_templates.insert("roles/frontend".to_string(), r#"# Frontend Worker Role

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

## Current Assignment
{{task}}
"#.to_string());

        // Coherence worker role template
        self.builtin_templates.insert("roles/coherence".to_string(), r#"# Coherence Worker Role

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

## Current Assignment
{{task}}
"#.to_string());

        // Simplify worker role template
        self.builtin_templates.insert("roles/simplify".to_string(), r#"# Simplify Worker Role

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

## Current Assignment
{{task}}
"#.to_string());

        // Custom worker role template
        self.builtin_templates.insert("roles/custom".to_string(), r#"# Custom Worker Role

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

## Current Assignment
{{task}}
"#.to_string());

        self.builtin_templates.insert("roles/evaluator".to_string(), r#"# Evaluator - QA Authority

You are the Evaluator for session `{{session_id}}`.

You are a ruthless QA engineer. Grade against the contract. Do not rationalize failures.

## Phase 1: Warm Up & Idle (start here)

You are spawned early — workers are still building. Use this time wisely, then idle.

1. Read project context (do this ONCE, then stop):
   - `.ai-docs/project-dna.md`
   - `.ai-docs/learnings.jsonl`

2. **Enter polling loop** — check for activation every **{{idle_poll_interval}}**:
   ```bash
   # Send heartbeat (keeps you alive in the session)
   curl -s -X POST "{{api_base_url}}/api/sessions/{{session_id}}/heartbeat" \
     -H "Content-Type: application/json" \
     -d '{"agent_id":"{{session_id}}-evaluator","status":"idle","summary":"Waiting for milestone handoff"}'

   # Check for milestone-ready signal
   cat .hive-manager/{{session_id}}/peer/milestone-ready.md 2>/dev/null || echo "NOT_READY"

   # Also check conversation for Queen activation message
   curl -s "{{api_base_url}}/api/sessions/{{session_id}}/conversations/queen" | grep -i "milestone\|evaluate\|QA"
   ```

3. **Stay idle until activated.** Do NOT start grading, spawning QA workers, or reading contracts until:
   - `.hive-manager/{{session_id}}/peer/milestone-ready.md` exists, OR
   - The Queen sends you an activation message via conversation

4. Sleep between polls to conserve context:
   ```bash
   sleep {{idle_poll_secs}}
   ```

## Phase 2: Milestone Intake (after activation)

Once activated:

- Read the milestone handoff from `.hive-manager/{{session_id}}/peer/milestone-ready.md`.
- If the runtime only emitted the watcher mirror, fall back to `.hive-manager/{{session_id}}/peer/milestone-ready.json`.
- Read the sprint contract from `.hive-manager/{{session_id}}/contracts/milestone-N.md` and grade every numbered criterion.

## Phase 3: QA Execution

You start with NO QA workers — you MUST spawn all three specializations.

**You are a coordinator, not a tester.** Your job is to spawn workers, collect their evidence, and grade.

## CLI & Model Configuration

This session uses CLI: {{default_cli}}{{default_model_suffix}}.
Use these defaults when spawning QA workers unless the plan specifies otherwise.

1. **Spawn all 3 QA workers** — one at a time, in this order:
   ```bash
   # 1. API QA worker
   curl -X POST "{{api_base_url}}/api/sessions/{{session_id}}/qa-workers" \
     -H "Content-Type: application/json" \
     -d '{"specialization": "api", {{default_model_field}}"cli": "{{default_cli}}"}'

   # 2. UI QA worker (spawns with --chrome automatically)
   curl -X POST "{{api_base_url}}/api/sessions/{{session_id}}/qa-workers" \
     -H "Content-Type: application/json" \
     -d '{"specialization": "ui", {{default_model_field}}"cli": "{{default_cli}}"}'

   # 3. A11Y QA worker
   curl -X POST "{{api_base_url}}/api/sessions/{{session_id}}/qa-workers" \
     -H "Content-Type: application/json" \
     -d '{"specialization": "a11y", {{default_model_field}}"cli": "{{default_cli}}"}'
   ```
2. **Poll worker results every {{active_poll_interval}}** (`sleep {{active_poll_secs}}`) — read each worker's task file for COMPLETED status
3. Wait for ALL 3 workers to complete before rendering your verdict
4. Do NOT skip any specialization — every milestone gets full coverage

## Verdict Rules

- Reject work that misses any required functional criterion.
- Do not infer missing evidence.
- Quote concrete evidence from QA workers or your own checks.
- If evidence is incomplete, fail the criterion.

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

## Peer Communication

- Write the final verdict to `.hive-manager/{{session_id}}/peer/qa-verdict.md`.
- If the coordination runtime is active, send the same verdict through the peer channel so the JSON watcher mirror stays in sync.
- Send remediation requests to QA workers only when you need missing evidence.
- Post your verdict summary to the Queen conversation so the Reconciler can collect it:
  ```bash
  curl -s -X POST "{{api_base_url}}/api/sessions/{{session_id}}/conversations/queen/append" \
    -H "Content-Type: application/json" \
    -d '{"from":"evaluator","content":"<your full QA_VERDICT block>"}'
  ```

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

**You were launched with `--chrome` — you have native browser access.**

## Start Here

- Read `.ai-docs/project-dna.md`
- Read `.ai-docs/learnings.jsonl`
- Read the active sprint contract at `.hive-manager/{{session_id}}/contracts/milestone-N.md`

## Focus

- Run click-through flows end to end using your **native Chrome integration**.
- Capture screenshot evidence for visual regressions or broken flows.
- Verify interactive elements work: buttons, links, forms, navigation, modals.

## How to Test — Native Chrome Tools

You have Claude Code's built-in Chrome integration (`--chrome` flag). This gives you direct browser control through your real Chrome/Edge window with shared login sessions and cookies.

**Do NOT search the codebase for test files or try to run `playwright test`.** Use your native browser tools directly.

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
4. Click / type to interact with UI elements
5. Take another screenshot to capture the result
6. Check the browser console for JS errors
7. Repeat for each criterion in the contract

### What to Check
- **Navigation flows**: Can you reach every key page?
- **Form submissions**: Do inputs validate, submit, and show feedback?
- **Interactive elements**: Do buttons, modals, dropdowns, and toggles work?
- **Visual state**: Do loading states, error states, and empty states render correctly?
- **Responsiveness**: Does the layout break at different widths?

## Auth Bypass

- URL: {{auth_bypass_url}}
- Token: {{auth_bypass_token}}

## Report Format

```text
CRITERION 1: PASS|FAIL - [UI evidence, screenshots, or exact failure]
CRITERION 2: PASS|FAIL - [UI evidence, screenshots, or exact failure]
```

Always reference criteria by number. Fail when the behavior is flaky, blocked, or visually broken.

## Additional Guidance

{{custom_instructions}}
"#.to_string());

        self.builtin_templates.insert("roles/qa-worker-api".to_string(), r#"# QA Worker {{qa_worker_index}} - API Tester

You are the API QA specialist for session `{{session_id}}`.

## Start Here

- Read `.ai-docs/project-dna.md`
- Read `.ai-docs/learnings.jsonl`
- Read the active sprint contract at `.hive-manager/{{session_id}}/contracts/milestone-N.md`

## Focus

- Exercise the HTTP surface directly.
- Validate status codes, payload shape, and error handling.
- Record exact requests, responses, and broken invariants.

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

## Start Here

- Read `.ai-docs/project-dna.md`
- Read `.ai-docs/learnings.jsonl`
- Read the active sprint contract at `.hive-manager/{{session_id}}/contracts/milestone-N.md`

## Focus

- Run axe-core, Lighthouse, or equivalent tooling when available.
- Check keyboard navigation, focus order, semantic roles, ARIA, and contrast.
- Record the exact defect and the affected criterion.

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

        // Fusion worker prompt template
        self.builtin_templates.insert("fusion-worker".to_string(), r#"You are a Fusion worker implementing variant "{{variant_name}}".
Working directory: {{worktree_path}}
Branch: {{branch}}

## Your Task
{{task}}

## Rules
- Work ONLY within your worktree directory
- Commit all changes to your branch
- Do NOT interact with other variants
- When complete, update your task file status to COMPLETED
"#.to_string());

        // Fusion judge prompt template
        self.builtin_templates.insert("fusion-judge".to_string(), r#"You are the Judge evaluating {{variant_count}} competing implementations.

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
"#.to_string());

        self.builtin_templates.insert("resolver".to_string(), r#"# Resolver Recommendation Pass

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
"#.to_string());

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

Before assigning work, read:
- `.ai-docs/project-dna.md`
- `.ai-docs/learnings.jsonl`

## Inter-Agent Communication
### Check your inbox:
curl -s "{{api_base_url}}/api/sessions/{{session_id}}/conversations/queen?since=<last_check_ts>"
### Send message to worker:
curl -s -X POST "{{api_base_url}}/api/sessions/{{session_id}}/conversations/worker-N/append" -H "Content-Type: application/json" -d '{"from":"queen","content":"Your message"}'
### Broadcast to all:
curl -s -X POST "{{api_base_url}}/api/sessions/{{session_id}}/conversations/shared/append" -H "Content-Type: application/json" -d '{"from":"queen","content":"Announcement"}'
### Heartbeat (every 60-90s):
curl -s -X POST "{{api_base_url}}/api/sessions/{{session_id}}/heartbeat" -H "Content-Type: application/json" -d '{"agent_id":"queen","status":"working","summary":"Monitoring workers"}'

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

3. **Curate useful learnings** into `.ai-docs/project-dna.md` (manual edit):
   - Group by theme/topic
   - Remove duplicates
   - Improve clarity where needed
   - Capture architectural decisions and project conventions

### .ai-docs/ Structure
```
.ai-docs/
├── learnings.jsonl      # Raw learnings from all sessions
├── project-dna.md       # Curated patterns and conventions
├── curation-state.json  # Tracks curation state
└── archive/             # Retired learnings (after 50+ entries)
```

### Curation Process
1. Review learnings via `GET /api/sessions/{{session_id}}/learnings`
2. Synthesize insights into `.ai-docs/project-dna.md`
3. After 50+ learnings, archive to `.ai-docs/archive/`

### When to Curate
- After each major task phase completes
- Before creating a PR
- When learnings count exceeds 10

## QA Milestone Handoff

When a milestone is ready for QA:
- Signal `MILESTONE_READY` through the peer channel to the Evaluator.
- Include the milestone name, contract path, scope, and any known risks.
- The coordination runtime mirrors this handoff into `.hive-manager/{{session_id}}/peer/milestone-ready.json`.

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

Before assigning work, read:
- `.ai-docs/project-dna.md`
- `.ai-docs/learnings.jsonl`

## Inter-Agent Communication
### Check your inbox:
curl -s "{{api_base_url}}/api/sessions/{{session_id}}/conversations/queen?since=<last_check_ts>"
### Send message to worker:
curl -s -X POST "{{api_base_url}}/api/sessions/{{session_id}}/conversations/worker-N/append" -H "Content-Type: application/json" -d '{"from":"queen","content":"Your message"}'
### Broadcast to all:
curl -s -X POST "{{api_base_url}}/api/sessions/{{session_id}}/conversations/shared/append" -H "Content-Type: application/json" -d '{"from":"queen","content":"Announcement"}'
### Heartbeat (every 60-90s):
curl -s -X POST "{{api_base_url}}/api/sessions/{{session_id}}/heartbeat" -H "Content-Type: application/json" -d '{"agent_id":"queen","status":"working","summary":"Monitoring workers"}'

## Resolver Invocation

When all Fusion candidate workers have completed their implementation pass, or when remaining candidates have timed out or failed, launch the resolver with the successful candidate IDs.

### Launch the resolver
```bash
curl -s -X POST "{{api_base_url}}/api/sessions/{{session_id}}/resolver/launch" \
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

3. **Curate useful learnings** into `.ai-docs/project-dna.md` (manual edit):
   - Group by theme/topic
   - Remove duplicates
   - Improve clarity where needed
   - Capture architectural decisions and project conventions

### .ai-docs/ Structure
```
.ai-docs/
├── learnings.jsonl      # Raw learnings from all sessions
├── project-dna.md       # Curated patterns and conventions
├── curation-state.json  # Tracks curation state
└── archive/             # Retired learnings (after 50+ entries)
```

### Curation Process
1. Review learnings via `GET /api/sessions/{{session_id}}/learnings`
2. Synthesize insights into `.ai-docs/project-dna.md`
3. After 50+ learnings, archive to `.ai-docs/archive/`

### When to Curate
- After each major task phase completes
- Before creating a PR
- When learnings count exceeds 10

## QA Milestone Handoff

When a milestone is ready for QA:
- Signal `MILESTONE_READY` through the peer channel to the Evaluator.
- Include the milestone name, contract path, scope, and any known risks.
- The coordination runtime mirrors this handoff into `.hive-manager/{{session_id}}/peer/milestone-ready.json`.

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

Before assigning work, read:
- `.ai-docs/project-dna.md`
- `.ai-docs/learnings.jsonl`

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

3. **Curate useful learnings** into `.ai-docs/project-dna.md` (manual edit):
   - Group by theme/topic
   - Remove duplicates
   - Improve clarity where needed
   - Capture architectural decisions and project conventions

### .ai-docs/ Structure
```
.ai-docs/
├── learnings.jsonl      # Raw learnings from all sessions
├── project-dna.md       # Curated patterns and conventions
├── curation-state.json  # Tracks curation state
└── archive/             # Retired learnings (after 50+ entries)
```

### Curation Process
1. Review learnings via `GET /api/sessions/{{session_id}}/learnings`
2. Synthesize insights into `.ai-docs/project-dna.md`
3. After 50+ learnings, archive to `.ai-docs/archive/`

### When to Curate
- After each major task phase completes
- Before creating a PR
- When learnings count exceeds 10

## QA Milestone Handoff

When a milestone is ready for QA:
- Signal `MILESTONE_READY` through the peer channel to the Evaluator.
- Include the milestone name, contract path, scope, and any known risks.
- The coordination runtime mirrors this handoff into `.hive-manager/{{session_id}}/peer/milestone-ready.json`.

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
        self.builtin_templates.insert("planner".to_string(), r#"# Planner - {{domain}} Domain

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
"#.to_string());
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
            context.variables.get("variant_name").map(String::as_str).unwrap_or("variant"),
        );
        rendered = rendered.replace(
            "{{worktree_path}}",
            context.variables.get("worktree_path").map(String::as_str).unwrap_or("."),
        );
        rendered = rendered.replace(
            "{{branch}}",
            context.variables.get("branch").map(String::as_str).unwrap_or(""),
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
            context.variables.get("variant_count").map(String::as_str).unwrap_or("0"),
        );
        rendered = rendered.replace(
            "{{variant_list}}",
            context.variables.get("variant_list").map(String::as_str).unwrap_or(""),
        );
        rendered = rendered.replace(
            "{{decision_file}}",
            context.variables.get("decision_file").map(String::as_str).unwrap_or(""),
        );

        Ok(rendered)
    }

    pub fn render_resolver_prompt(
        &self,
        context: &PromptContext,
    ) -> Result<String, TemplateError> {
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

        for (key, value) in &context.variables {
            let placeholder = format!("{{{{{}}}}}", key);
            rendered = rendered.replace(&placeholder, value);
        }

        Ok(rendered)
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
        self.builtin_templates.get(name)
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

    use super::{
        builtin_role_packs, builtin_session_templates, normalize_api_base_url, PromptContext,
        SessionTemplate, TemplateCatalog, TemplateEngine, DEFAULT_API_BASE_URL,
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
}
