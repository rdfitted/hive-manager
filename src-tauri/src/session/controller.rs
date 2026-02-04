use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::pty::{AgentRole, AgentStatus, AgentConfig, PtyManager, WorkerRole};
use crate::storage::SessionStorage;
use crate::coordination::{StateManager, HierarchyNode, WorkerStateInfo};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionType {
    Hive { worker_count: u8 },
    Swarm { planner_count: u8 },
    Fusion { variants: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionState {
    Planning,      // Master Planner is running
    PlanReady,     // Plan generated, waiting for user to continue
    Starting,
    Running,
    Paused,
    Completed,
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub role: AgentRole,
    pub status: AgentStatus,
    pub config: AgentConfig,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiveLaunchConfig {
    pub project_path: String,
    pub queen_config: AgentConfig,
    pub workers: Vec<AgentConfig>,
    pub prompt: Option<String>,
    #[serde(default)]
    pub with_planning: bool,  // If true, spawn Master Planner first
    #[serde(default)]
    pub smoke_test: bool,     // If true, create a minimal test plan without real investigation
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmLaunchConfig {
    pub project_path: String,
    pub queen_config: AgentConfig,
    pub planner_count: u8,                    // How many planners
    pub planner_config: AgentConfig,          // Config shared by all planners
    pub workers_per_planner: Vec<AgentConfig>, // Workers shared config (applied to each planner)
    pub prompt: Option<String>,
    #[serde(default)]
    pub with_planning: bool,  // If true, spawn Master Planner first
    #[serde(default)]
    pub smoke_test: bool,     // If true, create a minimal test plan without real investigation

    // Legacy support - if planners vec is provided, use it instead
    #[serde(default)]
    pub planners: Vec<PlannerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerConfig {
    pub config: AgentConfig,
    pub domain: String,
    pub workers: Vec<AgentConfig>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub session_type: SessionType,
    pub project_path: PathBuf,
    pub state: SessionState,
    pub created_at: DateTime<Utc>,
    pub agents: Vec<AgentInfo>,
}

#[derive(Clone, Serialize)]
pub struct SessionUpdate {
    pub session: Session,
}

pub struct SessionController {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    pty_manager: Arc<RwLock<PtyManager>>,
    app_handle: Option<AppHandle>,
    storage: Option<Arc<SessionStorage>>,
}

// Explicitly implement Send + Sync
unsafe impl Send for SessionController {}
unsafe impl Sync for SessionController {}

impl SessionController {
    pub fn new(pty_manager: Arc<RwLock<PtyManager>>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            pty_manager,
            app_handle: None,
            storage: None,
        }
    }

    pub fn set_app_handle(&mut self, handle: AppHandle) {
        self.app_handle = Some(handle.clone());
        let mut pty_manager = self.pty_manager.write();
        pty_manager.set_app_handle(handle);
    }

    pub fn set_storage(&mut self, storage: Arc<SessionStorage>) {
        self.storage = Some(storage);
    }

    pub fn launch_hive(
        &self,
        project_path: PathBuf,
        worker_count: u8,
        command: &str,
        prompt: Option<String>,
    ) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();
        let mut agents = Vec::new();
        let prompt_str = prompt.unwrap_or_default();
        let cwd = project_path.to_str().unwrap_or(".");

        // Parse command - support "command arg1 arg2" format
        let parts: Vec<&str> = command.split_whitespace().collect();
        let (cmd, base_args) = if parts.is_empty() {
            ("cmd.exe", vec![])
        } else {
            (parts[0], parts[1..].to_vec())
        };

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent
            let queen_id = format!("{}-queen", session_id);
            let mut queen_args = base_args.clone();

            // Add prompt as positional argument if provided and command is claude
            if cmd == "claude" && !prompt_str.is_empty() {
                queen_args.push(&prompt_str);
            }

            tracing::info!("Launching Queen agent: {} {:?} in {:?}", cmd, queen_args, project_path);

            pty_manager
                .create_session(
                    queen_id.clone(),
                    AgentRole::Queen,
                    cmd,
                    &queen_args.iter().map(|s| *s).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| {
                    let err_msg = format!("Failed to spawn Queen: {}", e);
                    tracing::error!("{}", err_msg);
                    err_msg
                })?;

            let queen_config = AgentConfig {
                cli: cmd.to_string(),
                model: if cmd == "claude" { Some("opus".to_string()) } else { None },
                flags: base_args.iter().map(|s| s.to_string()).collect(),
                label: None,
                role: None,
                initial_prompt: None,
            };

            agents.push(AgentInfo {
                id: queen_id,
                role: AgentRole::Queen,
                status: AgentStatus::Running,
                config: queen_config,
                parent_id: None,
            });

            // Create Worker agents
            for i in 1..=worker_count {
                let worker_id = format!("{}-worker-{}", session_id, i);
                let worker_args = base_args.clone();

                tracing::info!("Launching Worker {} agent: {} {:?} in {:?}", i, cmd, worker_args, project_path);

                pty_manager
                    .create_session(
                        worker_id.clone(),
                        AgentRole::Worker { index: i, parent: None },
                        cmd,
                        &worker_args.iter().map(|s| *s).collect::<Vec<_>>(),
                        Some(cwd),
                        120,
                        30,
                    )
                    .map_err(|e| {
                        let err_msg = format!("Failed to spawn Worker {}: {}", i, e);
                        tracing::error!("{}", err_msg);
                        err_msg
                    })?;

                let worker_config = AgentConfig {
                    cli: cmd.to_string(),
                    model: if cmd == "claude" { Some("opus".to_string()) } else { None },
                    flags: worker_args.iter().map(|s| s.to_string()).collect(),
                    label: None,
                    role: None,
                    initial_prompt: None,
                };

                agents.push(AgentInfo {
                    id: worker_id.clone(),
                    role: AgentRole::Worker { index: i, parent: Some(format!("{}-queen", session_id)) },
                    status: AgentStatus::Running,
                    config: worker_config,
                    parent_id: Some(format!("{}-queen", session_id)),
                });
            }
        }

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Hive { worker_count },
            project_path,
            state: SessionState::Running,
            created_at: Utc::now(),
            agents,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        Ok(session)
    }

    pub fn get_session(&self, id: &str) -> Option<Session> {
        let sessions = self.sessions.read();
        sessions.get(id).cloned()
    }

    pub fn list_sessions(&self) -> Vec<Session> {
        let sessions = self.sessions.read();
        sessions.values().cloned().collect()
    }

    pub fn stop_session(&self, id: &str) -> Result<(), String> {
        let session = {
            let sessions = self.sessions.read();
            sessions.get(id).cloned()
        };

        if let Some(session) = session {
            let pty_manager = self.pty_manager.read();
            for agent in &session.agents {
                let _ = pty_manager.kill(&agent.id);
            }

            {
                let mut sessions = self.sessions.write();
                if let Some(s) = sessions.get_mut(id) {
                    s.state = SessionState::Completed;
                }
            }

            if let Some(ref app_handle) = self.app_handle {
                let sessions = self.sessions.read();
                if let Some(session) = sessions.get(id) {
                    let _ = app_handle.emit("session-update", SessionUpdate {
                        session: session.clone(),
                    });
                }
            }

            Ok(())
        } else {
            Err(format!("Session not found: {}", id))
        }
    }

    pub fn stop_agent(&self, session_id: &str, agent_id: &str) -> Result<(), String> {
        let pty_manager = self.pty_manager.read();
        pty_manager.kill(agent_id).map_err(|e| e.to_string())?;

        {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                if let Some(agent) = session.agents.iter_mut().find(|a| a.id == agent_id) {
                    agent.status = AgentStatus::Completed;
                }
            }
        }

        Ok(())
    }

    /// Build command and args from AgentConfig
    /// Returns (command, args) with CLI-specific flags already added
    fn build_command(config: &AgentConfig) -> (String, Vec<String>) {
        let mut args = Vec::new();

        // Add CLI-specific flags
        match config.cli.as_str() {
            "claude" => {
                // Claude CLI requires --dangerously-skip-permissions for automated use
                args.push("--dangerously-skip-permissions".to_string());
                if let Some(ref model) = config.model {
                    args.push("--model".to_string());
                    args.push(model.clone());
                }
            }
            "gemini" => {
                // Gemini CLI uses -y for auto-approve
                args.push("-y".to_string());
                if let Some(ref model) = config.model {
                    args.push("-m".to_string());
                    args.push(model.clone());
                }
            }
            "codex" => {
                // Codex CLI uses --dangerously-bypass-approvals-and-sandbox
                args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
                if let Some(ref model) = config.model {
                    args.push("-m".to_string());
                    args.push(model.clone());
                }
            }
            "opencode" => {
                // OpenCode relies on OPENCODE_YOLO=true env var (set in batch file)
                if let Some(ref model) = config.model {
                    args.push("-m".to_string());
                    args.push(model.clone());
                }
            }
            _ => {
                // For other CLIs, just add model flag if specified
                if let Some(ref model) = config.model {
                    args.push("--model".to_string());
                    args.push(model.clone());
                }
            }
        }

        // Add any extra flags from config
        args.extend(config.flags.clone());

        (config.cli.clone(), args)
    }

    /// Add prompt argument to args based on CLI type
    /// Each CLI has different syntax for accepting initial prompts
    fn add_prompt_to_args(cli: &str, args: &mut Vec<String>, prompt_path: &str) {
        let prompt_arg = format!("Read {} and execute.", prompt_path);
        match cli {
            "claude" | "codex" => {
                // Claude and Codex accept prompt as positional argument
                args.push(prompt_arg);
            }
            "gemini" => {
                // Gemini uses -i flag for initial prompt
                args.push("-i".to_string());
                args.push(prompt_arg);
            }
            "opencode" => {
                // OpenCode uses --prompt flag
                args.push("--prompt".to_string());
                args.push(prompt_arg);
            }
            _ => {
                // Default: try positional argument
                args.push(prompt_arg);
            }
        }
    }

    /// Build the Master Planner's prompt for initial planning phase
    fn build_master_planner_prompt(session_id: &str, user_prompt: &str) -> String {
        // Determine phase 0 based on whether a task was provided
        let phase0 = if user_prompt.trim().is_empty() {
            String::from(r#"## PHASE 0: Gather Task (FIRST STEP)

**No task was provided.** You must first ask the user what they want to work on.

Ask the user: "What would you like me to help you with today? You can:
- Provide a GitHub issue number (e.g., #42 or just 42)
- Describe a feature you want to implement
- Describe a bug you want to fix
- Describe code you want to refactor"

**If user provides a GitHub Issue number:**
1. Fetch issue details using: gh issue view <number> --json number,title,body,labels,state
2. Extract requirements and acceptance criteria from the issue body

**Once you have the task, proceed to PHASE 1.**

---

"#)
        } else if user_prompt.trim().starts_with('#') || user_prompt.trim().parse::<u32>().is_ok() {
            // Looks like a GitHub issue number
            let issue_num = user_prompt.trim().trim_start_matches('#');
            format!(r#"## PHASE 0: Fetch GitHub Issue

The user wants to work on GitHub issue: **#{}**

**Fetch the issue details now:**
```bash
gh issue view {} --json number,title,body,labels,state
```

Extract from the response:
- Issue title and full description
- Acceptance criteria (look for checkboxes in the body)
- Labels (bug, feature, enhancement, etc.)

**Once you have the full context, proceed to PHASE 1.**

---

"#, issue_num, issue_num)
        } else {
            format!(r#"## PHASE 0: Task Provided

The user wants to work on:

**{}**

**Proceed directly to PHASE 1.**

---

"#, user_prompt)
        };

        format!(
r#"# Master Planner - Multi-Agent Codebase Investigation

You are the **Master Planner** orchestrating a multi-agent investigation to create a detailed implementation plan.

## Session Info

- **Session ID**: {session_id}
- **Plan Output**: `.hive-manager/{session_id}/plan.md`

## Your Mission

1. **Gather Task**: Understand what the user wants (GitHub issue or custom task)
2. **Spawn Scout Agents**: Launch parallel investigation agents using external CLIs
3. **Synthesize Findings**: Merge and deduplicate file discoveries
4. **Create Plan**: Write comprehensive plan.md with clear tasks
5. **Wait for Approval**: User will review and may request refinements

---

{phase0}## PHASE 1: Multi-Agent Investigation (MANDATORY)

You MUST spawn Task agents that call external CLI tools via Bash. This provides diverse model perspectives and comprehensive coverage.

**Launch ALL scouts in PARALLEL (single message, multiple Task calls):**

### Scout 1 - OpenCode BigPickle (Deep Analysis)

Task(subagent_type="general-purpose", prompt="You are a codebase investigation agent. IMMEDIATELY run: OPENCODE_YOLO=true opencode run --format default -m opencode/big-pickle 'Investigate codebase for: [TASK]. Find relevant files, architecture patterns, entry points.' Return file paths with relevance notes.")

### Scout 2 - OpenCode GLM 4.7 (Pattern Recognition)

Task(subagent_type="general-purpose", prompt="You are a codebase investigation agent. IMMEDIATELY run: OPENCODE_YOLO=true opencode run --format default -m opencode/glm-4.7-free 'Analyze codebase for: [TASK]. Focus on code patterns, affected components, dependencies.' Return file paths with observations.")

### Scout 3 - OpenCode Grok Code (Quick Search)

Task(subagent_type="general-purpose", prompt="You are a codebase investigation agent. IMMEDIATELY run: OPENCODE_YOLO=true opencode run --format default -m opencode/grok-code 'Scout codebase for: [TASK]. Identify entry points, test files, implementation surface.' Return file paths with notes.")

**CRITICAL RULES:**
- Replace [TASK] with the actual task description from Phase 0
- Launch ALL 3 scouts in PARALLEL using a SINGLE message
- Wait for ALL scouts to complete before proceeding

---

## PHASE 2: Synthesize Findings

After all scouts return:
1. Deduplicate files - merge overlapping discoveries
2. Rank by consensus - files found by 2-3 scouts = higher priority
3. Categorize: core files, supporting files, test files, config files
4. Identify implementation approach and potential risks

---

## PHASE 3: Write Plan

Write your plan to `.hive-manager/{session_id}/plan.md` with this format:

# [Plan Title]

## Summary
[1-2 sentence overview]

## Investigation Results
- Scouts Used: 3 (BigPickle, GLM 4.7, Grok Code)
- Files Identified: [count]
- Consensus Level: HIGH/MEDIUM/LOW

## Tasks
- [ ] [HIGH] Task 1 -> Worker 1
- [ ] [MEDIUM] Task 2 -> Worker 2
(use checkboxes, priority tags, and worker assignments)

## Files to Modify
| File | Priority | Changes Needed |
|------|----------|----------------|

## Dependencies
[Task ordering]

## Risks
[Potential issues]

---

## PHASE 4: Await User Feedback

After writing plan.md, say: **"PLAN READY FOR REVIEW"**

The user may approve or request refinements. Stay ready to update the plan.

---

## Begin Now

1. Complete PHASE 0 (gather task if needed)
2. Launch ALL 3 scout agents in PARALLEL
3. Synthesize findings
4. Write plan to `.hive-manager/{session_id}/plan.md`
5. Say "PLAN READY FOR REVIEW""#,
            session_id = session_id,
            phase0 = phase0
        )
    }

    /// Build a minimal smoke test prompt that creates a simple plan without real investigation
    fn build_smoke_test_prompt(session_id: &str) -> String {
        format!(
r#"# Smoke Test - Quick Flow Validation

You are running a **SMOKE TEST** to validate the Hive Manager flow.

## Your Task

Create a minimal test plan immediately. Do NOT spawn any investigation agents.
Do NOT analyze the codebase. Just create a simple plan to test the flow.

## Write This Plan Now

Write the following to `.hive-manager/{session_id}/plan.md`:

```markdown
# Smoke Test Plan

## Summary
This is a smoke test to validate the planning flow works correctly.

## Investigation Results
- Scouts Used: 0 (smoke test - skipped)
- Files Identified: 0
- Consensus Level: N/A

## Tasks
- [ ] [HIGH] Smoke test task 1: Verify worker spawning -> Worker 1
- [ ] [MEDIUM] Smoke test task 2: Verify Queen coordination -> Worker 2

## Files to Modify
| File | Priority | Changes Needed |
|------|----------|----------------|
| (smoke test - no real files) | N/A | N/A |

## Dependencies
Task 2 depends on Task 1 completing.

## Risks
None - this is a smoke test.

## Notes
Smoke test completed successfully. The planning phase flow is working.
```

After writing the plan, say: **"PLAN READY FOR REVIEW"**

This tests that:
1. Master Planner can write to the plan file
2. User can see and approve the plan
3. Flow continues to spawn Queen and Workers"#,
            session_id = session_id
        )
    }

    /// Build the Queen's master prompt with worker information
    fn build_queen_master_prompt(session_id: &str, workers: &[AgentConfig], user_prompt: Option<&str>, has_plan: bool) -> String {
        let mut worker_list = String::new();
        for (i, worker_config) in workers.iter().enumerate() {
            let index = i + 1;
            let worker_id = format!("{}-worker-{}", session_id, index);
            let role_label = worker_config.role.as_ref()
                .map(|r| format!("Worker {} ({})", index, r.label))
                .unwrap_or_else(|| format!("Worker {}", index));
            worker_list.push_str(&format!("| {} | {} | {} |\n", worker_id, role_label, worker_config.cli));
        }

        let plan_section = if has_plan {
            format!(
r#"## Implementation Plan

**IMPORTANT**: A plan has been generated for this session. Read it first:
```
.hive-manager/{}/plan.md
```

Follow the plan's task breakdown when assigning work to workers."#,
                session_id
            )
        } else {
            String::new()
        };

        format!(
r#"# Queen Agent - Hive Manager Session

You are the **Queen** orchestrating a multi-agent Hive session. You have full Claude Code capabilities plus coordination tools.

## Session Info

- **Session ID**: {session_id}
- **Prompts Directory**: `.hive-manager/{session_id}/prompts/`
- **Tasks Directory**: `.hive-manager/{session_id}/tasks/`

{plan_section}

## Your Workers

| ID | Role | CLI |
|----|------|-----|
{worker_list}
## Your Tools

### Claude Code Tools (Native)
You have full access to all Claude Code tools:
- **Read/Write/Edit** - File operations
- **Bash** - Run shell commands, git operations
- **Glob/Grep** - Search files and content
- **Task** - Spawn subagents for complex tasks
- **WebFetch/WebSearch** - Access web resources

### Claude Code Commands
You can use any /commands in `~/.claude/commands/`

### Hive Coordination
To assign tasks to workers, update their task files:

**Update task file status from STANDBY to ACTIVE:**
```
Edit: .hive-manager/{session_id}/tasks/worker-N-task.md
Change Status: STANDBY -> ACTIVE
Add task instructions
```

Workers are polling their task files and will start working when they see ACTIVE status.

## Coordination Protocol

1. **Read the plan** - Check `.hive-manager/{session_id}/plan.md` if it exists
2. **Assign tasks** - Update worker task files with specific assignments
3. **Monitor progress** - Watch for workers to mark tasks COMPLETED
4. **Review & integrate** - Review worker output and coordinate integration
5. **Commit & push** - You handle final commits (workers don't push)

## Your Task

{task}"#,
            session_id = session_id,
            plan_section = plan_section,
            worker_list = worker_list,
            task = user_prompt.unwrap_or("Read the plan and begin coordinating workers.")
        )
    }

    /// Build a worker's role prompt
    fn build_worker_prompt(index: u8, config: &AgentConfig, queen_id: &str, session_id: &str) -> String {
        let role_name = config.role.as_ref()
            .map(|r| r.label.clone())
            .unwrap_or_else(|| format!("Worker {}", index));

        let role_description = config.role.as_ref()
            .map(|r| match r.role_type.to_lowercase().as_str() {
                "backend" => "Server-side logic, APIs, databases, and backend infrastructure.",
                "frontend" => "UI components, state management, styling, and user experience.",
                "coherence" => "Code consistency, API contracts, and cross-component integration.",
                "simplify" => "Code simplification, refactoring, and reducing complexity.",
                "reviewer" => "Deep code review: edge cases, security, performance, architecture, breaking changes.",
                "reviewer-quick" => "Quick code review: obvious bugs, code style, simple improvements.",
                "resolver" => "Address all reviewer findings: fix HIGH/MEDIUM issues, document skipped items with rationale.",
                "tester" => "Run test suite, fix failures, document difficulties that couldn't be resolved.",
                "code-quality" => "Resolve PR comments from external reviewers, ensure code meets quality standards.",
                _ => "General development tasks as assigned.",
            })
            .unwrap_or("General development tasks as assigned.");

        let task_file = format!(".hive-manager/{}/tasks/worker-{}-task.md", session_id, index);

        // Issue #3: Codex and OpenCode CLIs do not automatically poll for task file
        // changes like Claude and Gemini do. We must include explicit bash polling
        // instructions in their worker prompts to ensure they detect ACTIVE status.
        let polling_instructions = match config.cli.as_str() {
            "codex" | "opencode" => format!(r#"

## Polling Protocol (CRITICAL - You MUST follow this)

Your CLI requires explicit polling. Run this bash loop to wait for activation:

```bash
while true; do
  STATUS=$(grep "^Status:" "{task_file}" | head -1)
  echo "[Worker {index}] Checking status: $STATUS"
  if [[ "$STATUS" == *"ACTIVE"* ]]; then
    echo "[Worker {index}] Task is ACTIVE - beginning work"
    break
  fi
  if [[ "$STATUS" == *"COMPLETED"* ]] || [[ "$STATUS" == *"BLOCKED"* ]]; then
    echo "[Worker {index}] Task already finished"
    break
  fi
  sleep 30
done
```

Do NOT simply "wait" conceptually. You MUST run the actual bash loop above."#,
                task_file = task_file, index = index),
            _ => String::new() // Claude and Gemini handle polling implicitly
        };

        format!(
r#"# Worker {index} ({role_name}) - Hive Session

You are a **Worker** in a multi-agent Hive session, coordinated by the Queen.

## Your Specialization

{role_description}

## Your Tools

You have full access to Claude Code tools:
- **Read/Write/Edit** - File operations
- **Bash** - Run shell commands
- **Glob/Grep** - Search files and content
- **Task** - Spawn subagents if needed

## Task File (File-Based Coordination)

Your task assignments are in: `{task_file}`

**Workflow:**
1. Read your task file to check your current status
2. If Status is `STANDBY` - wait and periodically re-check the file
3. If Status is `ACTIVE` - execute the task described in the file
4. When done, update the task file: change Status to `COMPLETED` and add your results
5. If blocked, change Status to `BLOCKED` and describe the issue

## Important Rules

1. **Stay in your lane** - Focus on your specialization ({role_name})
2. **Don't push to git** - Only the Queen commits and pushes
3. **Update your task file** - Always update status when done or blocked
4. **Ask for clarification** - If task is unclear, note it in the task file

## Coordinator

- **Queen**: {queen_id}

## Initial Action

Read your task file now: `{task_file}`

If the status is STANDBY, wait for the Queen to assign you a task by updating that file.{polling_instructions}"#,
            index = index,
            role_name = role_name,
            role_description = role_description,
            queen_id = queen_id,
            task_file = task_file,
            polling_instructions = polling_instructions
        )
    }

    /// Build a planner's role prompt
    fn build_planner_prompt(index: u8, config: &PlannerConfig, queen_id: &str) -> String {
        let mut worker_list = String::new();
        for (i, worker_config) in config.workers.iter().enumerate() {
            let worker_index = i + 1;
            let role_label = worker_config.role.as_ref()
                .map(|r| r.label.clone())
                .unwrap_or_else(|| format!("Worker {}", worker_index));
            worker_list.push_str(&format!("| Worker {} | {} | {} |\n", worker_index, role_label, worker_config.cli));
        }

        format!(
r#"# Planner {index} - {domain} Domain

You are a **Planner** in a multi-agent Swarm session, managing the {domain} domain.

## Your Domain

{domain}

## Your Workers

| ID | Role | CLI |
|----|------|-----|
{worker_list}
## Your Tools

You have full access to Claude Code tools:
- **Read/Write/Edit** - File operations
- **Bash** - Run shell commands
- **Glob/Grep** - Search files and content
- **Task** - Spawn subagents for complex tasks

## Coordination

- **Queen**: {queen_id}
- Break down domain tasks into worker assignments
- Write task files for your workers
- Monitor worker progress via [COMPLETED] / [BLOCKED] markers
- Report to Queen when domain work is complete

## Protocol

1. Receive domain task from Queen
2. Break into worker subtasks
3. Write task files to assign work
4. Monitor and coordinate
5. Report `[DOMAIN_COMPLETE]` when done

## Your Current Task

Awaiting task assignment from the Queen."#,
            index = index,
            domain = config.domain,
            worker_list = worker_list,
            queen_id = queen_id
        )
    }

    /// Build the Queen's master prompt for Swarm mode with planner information
    fn build_swarm_queen_prompt(session_id: &str, planners: &[PlannerConfig], user_prompt: Option<&str>) -> String {
        let mut planner_list = String::new();
        for (i, planner_config) in planners.iter().enumerate() {
            let index = i + 1;
            let planner_id = format!("{}-planner-{}", session_id, index);
            planner_list.push_str(&format!("| {} | {} | {} | {} workers |\n",
                planner_id, index, planner_config.domain, planner_config.workers.len()));
        }

        format!(
r#"# Queen Agent - Swarm Session

You are the **Queen** orchestrating a multi-agent Swarm session. You coordinate Planners who each manage their own domain.

## Session Info

- **Session ID**: {session_id}
- **Mode**: Swarm (hierarchical)
- **Prompts Directory**: `.hive-manager/{session_id}/prompts/`

## Your Planners

| ID | # | Domain | Workers |
|----|---|--------|---------|
{planner_list}
## Your Tools

### Claude Code Tools (Native)
You have full access to all Claude Code tools:
- **Read/Write/Edit** - File operations
- **Bash** - Run shell commands, git operations
- **Glob/Grep** - Search files and content
- **Task** - Spawn subagents for complex tasks
- **WebFetch/WebSearch** - Access web resources

### Claude Code Commands
You can use any /commands in `~/.claude/commands/`

### Swarm Coordination
Assign domain-level tasks to Planners. Each Planner will break down the task and coordinate their workers.

**Task Assignment:**
Write domain tasks to planner prompt files or tell the operator:
```
@Operator: Tell Planner 1 to handle the backend API implementation
```

## Swarm Protocol

1. **Analyze task** - Identify which domains are involved
2. **Assign to Planners** - Give each Planner their domain scope
3. **Monitor progress** - Watch for [DOMAIN_COMPLETE] from Planners
4. **Integration** - Coordinate cross-domain integration
5. **Commit & push** - You handle final commits (only you push)

## Your Task

{task}"#,
            session_id = session_id,
            planner_list = planner_list,
            task = user_prompt.unwrap_or("Awaiting instructions from the operator.")
        )
    }

    /// Write a prompt file to the session's prompts directory
    fn write_prompt_file(project_path: &PathBuf, session_id: &str, filename: &str, content: &str) -> Result<PathBuf, String> {
        let prompts_dir = project_path.join(".hive-manager").join(session_id).join("prompts");
        std::fs::create_dir_all(&prompts_dir)
            .map_err(|e| format!("Failed to create prompts directory: {}", e))?;

        let file_path = prompts_dir.join(filename);
        std::fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write prompt file: {}", e))?;

        Ok(file_path)
    }

    /// Write a task file for a worker (STANDBY by default)
    fn write_task_file(project_path: &PathBuf, session_id: &str, worker_index: u8, initial_task: Option<&str>) -> Result<PathBuf, String> {
        let tasks_dir = project_path.join(".hive-manager").join(session_id).join("tasks");
        std::fs::create_dir_all(&tasks_dir)
            .map_err(|e| format!("Failed to create tasks directory: {}", e))?;

        let filename = format!("worker-{}-task.md", worker_index);
        let file_path = tasks_dir.join(&filename);

        let (status, task_content) = if let Some(task) = initial_task {
            ("ACTIVE", task.to_string())
        } else {
            ("STANDBY", "Awaiting task assignment. Monitor this file for updates.".to_string())
        };

        let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let content = format!(
"# Task Assignment - Worker {worker_index}

## Status: {status}

## Instructions

{task_content}

## Completion Protocol

When task is complete, update this file:
1. Change Status to: COMPLETED
2. Add a summary under a new Result section

If blocked, change Status to: BLOCKED and describe the issue.

---
Last updated: {timestamp}
",
            worker_index = worker_index,
            status = status,
            task_content = task_content,
            timestamp = timestamp
        );

        std::fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write task file: {}", e))?;

        Ok(file_path)
    }
    pub fn launch_hive_v2(&self, config: HiveLaunchConfig) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();
        let mut agents = Vec::new();
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();

        // If with_planning is true, spawn Master Planner first
        if config.with_planning {
            return self.launch_planning_phase(session_id, config);
        }

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent
            let queen_id = format!("{}-queen", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config);

            // Check if plan.md exists (from previous planning phase)
            let plan_path = project_path.join(".hive-manager").join(&session_id).join("plan.md");
            let has_plan = plan_path.exists();

            // Write Queen prompt to file and pass to CLI
            let master_prompt = Self::build_queen_master_prompt(&session_id, &config.workers, config.prompt.as_deref(), has_plan);
            let prompt_file = Self::write_prompt_file(&project_path, &session_id, "queen-prompt.md", &master_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!("Launching Queen agent (v2): {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    queen_id.clone(),
                    AgentRole::Queen,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Queen: {}", e))?;

            agents.push(AgentInfo {
                id: queen_id.clone(),
                role: AgentRole::Queen,
                status: AgentStatus::Running,
                config: config.queen_config.clone(),
                parent_id: None,
            });

            // Create Worker agents
            for (i, worker_config) in config.workers.iter().enumerate() {
                let index = (i + 1) as u8;
                let worker_id = format!("{}-worker-{}", session_id, index);
                let (cmd, mut args) = Self::build_command(worker_config);

                // Write task file for this worker (STANDBY or with initial task)
                Self::write_task_file(&project_path, &session_id, index, worker_config.initial_prompt.as_deref())?;

                // Write worker prompt to file and pass to CLI
                let worker_prompt = Self::build_worker_prompt(index, worker_config, &queen_id, &session_id);
                let filename = format!("worker-{}-prompt.md", index);
                let prompt_file = Self::write_prompt_file(&project_path, &session_id, &filename, &worker_prompt)?;
                let prompt_path = prompt_file.to_string_lossy().to_string();
                Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

                tracing::info!("Launching Worker {} agent (v2): {} {:?} in {:?}", index, cmd, args, cwd);

                pty_manager
                    .create_session(
                        worker_id.clone(),
                        AgentRole::Worker { index, parent: Some(queen_id.clone()) },
                        &cmd,
                        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                        Some(cwd),
                        120,
                        30,
                    )
                    .map_err(|e| format!("Failed to spawn Worker {}: {}", index, e))?;

                agents.push(AgentInfo {
                    id: worker_id,
                    role: AgentRole::Worker { index, parent: Some(queen_id.clone()) },
                    status: AgentStatus::Running,
                    config: worker_config.clone(),
                    parent_id: Some(queen_id.clone()),
                });
            }
        }

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Hive { worker_count: config.workers.len() as u8 },
            project_path,
            state: SessionState::Running,
            created_at: Utc::now(),
            agents,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        // Initialize session storage
        self.init_session_storage(&session);

        Ok(session)
    }

    /// Launch the planning phase - spawns Master Planner only
    fn launch_planning_phase(&self, session_id: String, config: HiveLaunchConfig) -> Result<Session, String> {
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();
        let mut agents = Vec::new();

        // Build the appropriate prompt based on mode
        let planner_prompt = if config.smoke_test {
            tracing::info!("Running in SMOKE TEST mode - skipping real investigation");
            Self::build_smoke_test_prompt(&session_id)
        } else {
            // Empty string means Master Planner will ask user what task they want
            let prompt = config.prompt.as_deref().unwrap_or("");
            Self::build_master_planner_prompt(&session_id, prompt)
        };

        {
            let pty_manager = self.pty_manager.read();

            // Create Master Planner agent
            let planner_id = format!("{}-master-planner", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config); // Use queen config for planner

            // Write Master Planner prompt to file
            let prompt_file = Self::write_prompt_file(&project_path, &session_id, "master-planner-prompt.md", &planner_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!("Launching Master Planner: {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    planner_id.clone(),
                    AgentRole::MasterPlanner,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Master Planner: {}", e))?;

            agents.push(AgentInfo {
                id: planner_id,
                role: AgentRole::MasterPlanner,
                status: AgentStatus::Running,
                config: config.queen_config.clone(),
                parent_id: None,
            });
        }

        // Store the pending config for later continuation
        let pending_config_path = project_path.join(".hive-manager").join(&session_id).join("pending-config.json");
        std::fs::create_dir_all(pending_config_path.parent().unwrap())
            .map_err(|e| format!("Failed to create session directory: {}", e))?;
        let config_json = serde_json::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        std::fs::write(&pending_config_path, config_json)
            .map_err(|e| format!("Failed to write pending config: {}", e))?;

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Hive { worker_count: config.workers.len() as u8 },
            project_path,
            state: SessionState::Planning,
            created_at: Utc::now(),
            agents,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        self.init_session_storage(&session);

        Ok(session)
    }

    /// Launch the planning phase for Swarm - spawns Master Planner only
    fn launch_swarm_planning_phase(&self, session_id: String, config: SwarmLaunchConfig) -> Result<Session, String> {
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();
        let mut agents = Vec::new();

        // Build the appropriate prompt based on mode
        let planner_prompt = if config.smoke_test {
            tracing::info!("Running in SMOKE TEST mode (swarm) - skipping real investigation");
            Self::build_smoke_test_prompt(&session_id)
        } else {
            // Empty string means Master Planner will ask user what task they want
            let prompt = config.prompt.as_deref().unwrap_or("");
            Self::build_master_planner_prompt(&session_id, prompt)
        };

        {
            let pty_manager = self.pty_manager.read();

            // Create Master Planner agent
            let planner_id = format!("{}-master-planner", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config); // Use queen config for planner

            // Write Master Planner prompt to file
            let prompt_file = Self::write_prompt_file(&project_path, &session_id, "master-planner-prompt.md", &planner_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!("Launching Master Planner (swarm): {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    planner_id.clone(),
                    AgentRole::MasterPlanner,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Master Planner: {}", e))?;

            agents.push(AgentInfo {
                id: planner_id,
                role: AgentRole::MasterPlanner,
                status: AgentStatus::Running,
                config: config.queen_config.clone(),
                parent_id: None,
            });
        }

        // Store the pending Swarm config for later continuation
        let pending_config_path = project_path.join(".hive-manager").join(&session_id).join("pending-swarm-config.json");
        std::fs::create_dir_all(pending_config_path.parent().unwrap())
            .map_err(|e| format!("Failed to create session directory: {}", e))?;
        let config_json = serde_json::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        std::fs::write(&pending_config_path, config_json)
            .map_err(|e| format!("Failed to write pending config: {}", e))?;

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Swarm { planner_count: if config.planners.is_empty() { config.planner_count } else { config.planners.len() as u8 } },
            project_path,
            state: SessionState::Planning,
            created_at: Utc::now(),
            agents,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        self.init_session_storage(&session);

        Ok(session)
    }

    /// Continue a session after planning phase - spawns Queen + Workers/Planners
    pub fn continue_after_planning(&self, session_id: &str) -> Result<Session, String> {
        // Get the session
        let session = {
            let sessions = self.sessions.read();
            sessions.get(session_id).cloned()
        }.ok_or_else(|| format!("Session not found: {}", session_id))?;

        // Verify session is in Planning or PlanReady state
        if session.state != SessionState::Planning && session.state != SessionState::PlanReady {
            return Err(format!("Session is not in planning phase: {:?}", session.state));
        }

        // Dispatch based on session type
        match &session.session_type {
            SessionType::Swarm { .. } => {
                return self.continue_swarm_after_planning(session_id, &session);
            }
            _ => {} // Continue with Hive logic below
        }

        let cwd = session.project_path.to_str().unwrap_or(".");

        // Load the pending config
        let pending_config_path = session.project_path.join(".hive-manager").join(session_id).join("pending-config.json");
        let config_json = std::fs::read_to_string(&pending_config_path)
            .map_err(|e| format!("Failed to read pending config: {}", e))?;
        let config: HiveLaunchConfig = serde_json::from_str(&config_json)
            .map_err(|e| format!("Failed to parse pending config: {}", e))?;

        // Clean up Master Planner PTY before spawning Queen (fixes terminal corruption)
        let planner_id = format!("{}-master-planner", session_id);
        if let Err(e) = self.stop_agent(session_id, &planner_id) {
            tracing::warn!("Failed to stop Master Planner {}: {}", planner_id, e);
        } else {
            tracing::info!("Stopped Master Planner {} before spawning Queen", planner_id);
            // Remove Master Planner from agents list to prevent resource leaks
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.agents.retain(|a| a.id != planner_id);
            }
        }

        let mut new_agents = Vec::new();

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent
            let queen_id = format!("{}-queen", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config);

            // Plan should exist now
            let has_plan = session.project_path.join(".hive-manager").join(session_id).join("plan.md").exists();

            // Write Queen prompt with plan reference
            let master_prompt = Self::build_queen_master_prompt(session_id, &config.workers, config.prompt.as_deref(), has_plan);
            let prompt_file = Self::write_prompt_file(&session.project_path, session_id, "queen-prompt.md", &master_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!("Launching Queen agent (after planning): {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    queen_id.clone(),
                    AgentRole::Queen,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Queen: {}", e))?;

            new_agents.push(AgentInfo {
                id: queen_id.clone(),
                role: AgentRole::Queen,
                status: AgentStatus::Running,
                config: config.queen_config.clone(),
                parent_id: None,
            });

            // Create Worker agents
            for (i, worker_config) in config.workers.iter().enumerate() {
                let index = (i + 1) as u8;
                let worker_id = format!("{}-worker-{}", session_id, index);
                let (cmd, mut args) = Self::build_command(worker_config);

                // Write task file for this worker
                Self::write_task_file(&session.project_path, session_id, index, worker_config.initial_prompt.as_deref())?;

                // Write worker prompt
                let worker_prompt = Self::build_worker_prompt(index, worker_config, &queen_id, session_id);
                let filename = format!("worker-{}-prompt.md", index);
                let prompt_file = Self::write_prompt_file(&session.project_path, session_id, &filename, &worker_prompt)?;
                let prompt_path = prompt_file.to_string_lossy().to_string();
                Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

                tracing::info!("Launching Worker {} (after planning): {} {:?} in {:?}", index, cmd, args, cwd);

                pty_manager
                    .create_session(
                        worker_id.clone(),
                        AgentRole::Worker { index, parent: Some(queen_id.clone()) },
                        &cmd,
                        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                        Some(cwd),
                        120,
                        30,
                    )
                    .map_err(|e| format!("Failed to spawn Worker {}: {}", index, e))?;

                new_agents.push(AgentInfo {
                    id: worker_id,
                    role: AgentRole::Worker { index, parent: Some(queen_id.clone()) },
                    status: AgentStatus::Running,
                    config: worker_config.clone(),
                    parent_id: Some(queen_id.clone()),
                });
            }
        }

        // Update session with new agents and Running state
        let updated_session = {
            let mut sessions = self.sessions.write();
            if let Some(s) = sessions.get_mut(session_id) {
                s.agents.extend(new_agents);
                s.state = SessionState::Running;
                s.clone()
            } else {
                return Err("Session disappeared".to_string());
            }
        };

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: updated_session.clone(),
            });
        }

        // Update storage
        self.update_session_storage(session_id);

        // Clean up pending config file
        let _ = std::fs::remove_file(&pending_config_path);

        Ok(updated_session)
    }

    /// Mark a planning session as ready (plan generated)
    pub fn mark_plan_ready(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.write();
        if let Some(session) = sessions.get_mut(session_id) {
            if session.state == SessionState::Planning {
                session.state = SessionState::PlanReady;

                if let Some(ref app_handle) = self.app_handle {
                    let _ = app_handle.emit("session-update", SessionUpdate {
                        session: session.clone(),
                    });
                }
                Ok(())
            } else {
                Err(format!("Session is not in planning state: {:?}", session.state))
            }
        } else {
            Err(format!("Session not found: {}", session_id))
        }
    }

    /// Continue a Swarm session after planning phase
    fn continue_swarm_after_planning(&self, session_id: &str, session: &Session) -> Result<Session, String> {
        let cwd = session.project_path.to_str().unwrap_or(".");

        // Load the pending Swarm config
        let pending_config_path = session.project_path.join(".hive-manager").join(session_id).join("pending-swarm-config.json");
        let config_json = std::fs::read_to_string(&pending_config_path)
            .map_err(|e| format!("Failed to read pending swarm config: {}", e))?;
        let config: SwarmLaunchConfig = serde_json::from_str(&config_json)
            .map_err(|e| format!("Failed to parse pending swarm config: {}", e))?;

        // Generate planners from simplified config (or use legacy planners if provided)
        let planners: Vec<PlannerConfig> = if !config.planners.is_empty() {
            config.planners.clone()
        } else {
            (0..config.planner_count)
                .map(|i| PlannerConfig {
                    config: config.planner_config.clone(),
                    domain: format!("domain-{}", i + 1),
                    workers: config.workers_per_planner.clone(),
                })
                .collect()
        };

        let mut new_agents = Vec::new();

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent
            let queen_id = format!("{}-queen", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config);

            // Write Queen prompt with plan reference
            let master_prompt = Self::build_swarm_queen_prompt(session_id, &planners, config.prompt.as_deref());
            let prompt_file = Self::write_prompt_file(&session.project_path, session_id, "queen-prompt.md", &master_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!("Launching Queen agent (swarm, after planning): {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    queen_id.clone(),
                    AgentRole::Queen,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Queen: {}", e))?;

            new_agents.push(AgentInfo {
                id: queen_id.clone(),
                role: AgentRole::Queen,
                status: AgentStatus::Running,
                config: config.queen_config.clone(),
                parent_id: None,
            });

            // Create Planner agents with their Workers
            for (pi, planner_config) in planners.iter().enumerate() {
                let planner_index = (pi + 1) as u8;
                let planner_id = format!("{}-planner-{}", session_id, planner_index);
                let (cmd, mut args) = Self::build_command(&planner_config.config);

                // Build planner prompt
                let planner_prompt = Self::build_planner_prompt(
                    planner_index,
                    planner_config,
                    &queen_id,
                );
                let filename = format!("planner-{}-prompt.md", planner_index);
                let prompt_file = Self::write_prompt_file(&session.project_path, session_id, &filename, &planner_prompt)?;
                let prompt_path = prompt_file.to_string_lossy().to_string();
                Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

                tracing::info!("Launching Planner {} (swarm, after planning): {} {:?}", planner_index, cmd, args);

                pty_manager
                    .create_session(
                        planner_id.clone(),
                        AgentRole::Planner { index: planner_index },
                        &cmd,
                        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                        Some(cwd),
                        120,
                        30,
                    )
                    .map_err(|e| format!("Failed to spawn Planner {}: {}", planner_index, e))?;

                new_agents.push(AgentInfo {
                    id: planner_id.clone(),
                    role: AgentRole::Planner { index: planner_index },
                    status: AgentStatus::Running,
                    config: planner_config.config.clone(),
                    parent_id: Some(queen_id.clone()),
                });

                // Create Workers for this Planner
                for (wi, worker_config) in planner_config.workers.iter().enumerate() {
                    let worker_index = (wi + 1) as u8;
                    let worker_id = format!("{}-planner-{}-worker-{}", session_id, planner_index, worker_index);
                    let (cmd, mut args) = Self::build_command(worker_config);

                    let worker_prompt = Self::build_worker_prompt(
                        worker_index,
                        worker_config,
                        &planner_id,
                        session_id,
                    );
                    let filename = format!("planner-{}-worker-{}-prompt.md", planner_index, worker_index);
                    let prompt_file = Self::write_prompt_file(&session.project_path, session_id, &filename, &worker_prompt)?;
                    let prompt_path = prompt_file.to_string_lossy().to_string();
                    Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

                    pty_manager
                        .create_session(
                            worker_id.clone(),
                            AgentRole::Worker { index: worker_index, parent: Some(planner_id.clone()) },
                            &cmd,
                            &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                            Some(cwd),
                            120,
                            30,
                        )
                        .map_err(|e| format!("Failed to spawn Worker {}: {}", worker_index, e))?;

                    new_agents.push(AgentInfo {
                        id: worker_id,
                        role: AgentRole::Worker { index: worker_index, parent: Some(planner_id.clone()) },
                        status: AgentStatus::Running,
                        config: worker_config.clone(),
                        parent_id: Some(planner_id.clone()),
                    });
                }
            }
        }

        // Update session with new agents and Running state
        let updated_session = {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                session.agents.extend(new_agents);
                session.state = SessionState::Running;
                session.clone()
            } else {
                return Err("Session disappeared".to_string());
            }
        };

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: updated_session.clone(),
            });
        }

        // Update storage
        self.update_session_storage(session_id);

        // Clean up pending config file
        let _ = std::fs::remove_file(&pending_config_path);

        Ok(updated_session)
    }

    pub fn launch_swarm(&self, config: SwarmLaunchConfig) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();

        // If with_planning is true, spawn Master Planner first
        if config.with_planning {
            return self.launch_swarm_planning_phase(session_id, config);
        }

        // Generate planners from simplified config (or use legacy planners if provided)
        let planners: Vec<PlannerConfig> = if !config.planners.is_empty() {
            config.planners.clone()
        } else {
            (0..config.planner_count)
                .map(|i| PlannerConfig {
                    config: config.planner_config.clone(),
                    domain: format!("domain-{}", i + 1),
                    workers: config.workers_per_planner.clone(),
                })
                .collect()
        };

        let mut agents = Vec::new();
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent
            let queen_id = format!("{}-queen", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config);

            // Write Queen prompt to file and pass to CLI
            let master_prompt = Self::build_swarm_queen_prompt(&session_id, &planners, config.prompt.as_deref());
            let prompt_file = Self::write_prompt_file(&project_path, &session_id, "queen-prompt.md", &master_prompt)?;
            let prompt_path = prompt_file.to_string_lossy().to_string();
            Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

            tracing::info!("Launching Queen agent (swarm): {} {:?} in {:?}", cmd, args, cwd);

            pty_manager
                .create_session(
                    queen_id.clone(),
                    AgentRole::Queen,
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Queen: {}", e))?;

            agents.push(AgentInfo {
                id: queen_id.clone(),
                role: AgentRole::Queen,
                status: AgentStatus::Running,
                config: config.queen_config.clone(),
                parent_id: None,
            });

            // Create Planner agents and their Workers
            for (planner_idx, planner_config) in planners.iter().enumerate() {
                let planner_index = (planner_idx + 1) as u8;
                let planner_id = format!("{}-planner-{}", session_id, planner_index);
                let (cmd, mut args) = Self::build_command(&planner_config.config);

                // Write planner prompt to file and pass to CLI
                let planner_prompt = Self::build_planner_prompt(planner_index, planner_config, &queen_id);
                let filename = format!("planner-{}-prompt.md", planner_index);
                let prompt_file = Self::write_prompt_file(&project_path, &session_id, &filename, &planner_prompt)?;
                let prompt_path = prompt_file.to_string_lossy().to_string();
                Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

                tracing::info!("Launching Planner {} ({}) agent: {} {:?} in {:?}",
                    planner_index, planner_config.domain, cmd, args, cwd);

                pty_manager
                    .create_session(
                        planner_id.clone(),
                        AgentRole::Planner { index: planner_index },
                        &cmd,
                        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                        Some(cwd),
                        120,
                        30,
                    )
                    .map_err(|e| format!("Failed to spawn Planner {}: {}", planner_index, e))?;

                agents.push(AgentInfo {
                    id: planner_id.clone(),
                    role: AgentRole::Planner { index: planner_index },
                    status: AgentStatus::Running,
                    config: planner_config.config.clone(),
                    parent_id: Some(queen_id.clone()),
                });

                // Create Workers for this Planner
                for (worker_idx, worker_config) in planner_config.workers.iter().enumerate() {
                    let worker_index = (worker_idx + 1) as u8;
                    // For swarm, use combined index for unique task file naming
                    let combined_index = planner_index * 10 + worker_index;
                    let worker_id = format!("{}-planner-{}-worker-{}", session_id, planner_index, worker_index);
                    let (cmd, mut args) = Self::build_command(worker_config);

                    // Write task file for this worker (STANDBY or with initial task)
                    Self::write_task_file(&project_path, &session_id, combined_index, worker_config.initial_prompt.as_deref())?;

                    // Write worker prompt to file and pass to CLI (use combined_index for task file reference)
                    let worker_prompt = Self::build_worker_prompt(combined_index, worker_config, &planner_id, &session_id);
                    let filename = format!("planner-{}-worker-{}-prompt.md", planner_index, worker_index);
                    let prompt_file = Self::write_prompt_file(&project_path, &session_id, &filename, &worker_prompt)?;
                    let prompt_path = prompt_file.to_string_lossy().to_string();
                    Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

                    tracing::info!("Launching Worker {}.{} agent: {} {:?} in {:?}",
                        planner_index, worker_index, cmd, args, cwd);

                    pty_manager
                        .create_session(
                            worker_id.clone(),
                            AgentRole::Worker { index: worker_index, parent: Some(planner_id.clone()) },
                            &cmd,
                            &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                            Some(cwd),
                            120,
                            30,
                        )
                        .map_err(|e| format!("Failed to spawn Worker {}.{}: {}", planner_index, worker_index, e))?;

                    agents.push(AgentInfo {
                        id: worker_id,
                        role: AgentRole::Worker { index: worker_index, parent: Some(planner_id.clone()) },
                        status: AgentStatus::Running,
                        config: worker_config.clone(),
                        parent_id: Some(planner_id.clone()),
                    });
                }
            }
        }

        let session = Session {
            id: session_id.clone(),
            session_type: SessionType::Swarm { planner_count: planners.len() as u8 },
            project_path,
            state: SessionState::Running,
            created_at: Utc::now(),
            agents,
        };

        {
            let mut sessions = self.sessions.write();
            sessions.insert(session_id.clone(), session.clone());
        }

        if let Some(ref app_handle) = self.app_handle {
            let _ = app_handle.emit("session-update", SessionUpdate {
                session: session.clone(),
            });
        }

        // Initialize session storage if available
        self.init_session_storage(&session);

        Ok(session)
    }

    /// Add a worker to an existing session
    pub fn add_worker(
        &self,
        session_id: &str,
        config: AgentConfig,
        role: WorkerRole,
        parent_id: Option<String>,
    ) -> Result<AgentInfo, String> {
        // Get session and validate
        let session = {
            let sessions = self.sessions.read();
            sessions.get(session_id).cloned()
        }.ok_or_else(|| format!("Session not found: {}", session_id))?;

        if session.state != SessionState::Running {
            return Err("Cannot add worker to non-running session".to_string());
        }

        // Determine worker index
        let existing_workers = session.agents.iter()
            .filter(|a| matches!(a.role, AgentRole::Worker { .. }))
            .count();
        let worker_index = (existing_workers + 1) as u8;

        // Determine parent (default to Queen)
        let actual_parent_id = parent_id.unwrap_or_else(|| format!("{}-queen", session_id));

        // Generate worker ID
        let worker_id = format!("{}-worker-{}", session_id, worker_index);

        // Build command
        let (cmd, mut args) = Self::build_command(&config);

        // Get project path
        let cwd = session.project_path.to_str().unwrap_or(".");

        // Create a temporary config with role for prompt generation
        let mut config_with_role = config.clone();
        config_with_role.role = Some(role.clone());

        // Write task file for this worker (STANDBY or with initial task)
        Self::write_task_file(&session.project_path, session_id, worker_index, config_with_role.initial_prompt.as_deref())?;

        // Write worker prompt to file and add to args
        let worker_prompt = Self::build_worker_prompt(worker_index, &config_with_role, &actual_parent_id, session_id);
        let filename = format!("worker-{}-prompt.md", worker_index);
        let prompt_file = Self::write_prompt_file(&session.project_path, session_id, &filename, &worker_prompt)?;
        let prompt_path = prompt_file.to_string_lossy().to_string();
        Self::add_prompt_to_args(&cmd, &mut args, &prompt_path);

        tracing::info!(
            "Adding Worker {} ({}) to session {}: {} {:?}",
            worker_index,
            role.label,
            session_id,
            cmd,
            args
        );

        // Spawn PTY
        {
            let pty_manager = self.pty_manager.read();
            pty_manager
                .create_session(
                    worker_id.clone(),
                    AgentRole::Worker { index: worker_index, parent: Some(actual_parent_id.clone()) },
                    &cmd,
                    &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    Some(cwd),
                    120,
                    30,
                )
                .map_err(|e| format!("Failed to spawn Worker {}: {}", worker_index, e))?;
        }

        // Create agent info with role
        let mut agent_config = config;
        agent_config.role = Some(role);

        let agent_info = AgentInfo {
            id: worker_id.clone(),
            role: AgentRole::Worker { index: worker_index, parent: Some(actual_parent_id.clone()) },
            status: AgentStatus::Running,
            config: agent_config,
            parent_id: Some(actual_parent_id),
        };

        // Update session
        {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(session_id) {
                session.agents.push(agent_info.clone());
            }
        }

        // Emit session update
        if let Some(ref app_handle) = self.app_handle {
            let sessions = self.sessions.read();
            if let Some(session) = sessions.get(session_id) {
                let _ = app_handle.emit("session-update", SessionUpdate {
                    session: session.clone(),
                });
            }
        }

        // Update session storage
        self.update_session_storage(session_id);

        Ok(agent_info)
    }

    /// Initialize session storage for a new session
    fn init_session_storage(&self, session: &Session) {
        if let Some(ref storage) = self.storage {
            // Create session directory
            if let Err(e) = storage.create_session_dir(&session.id) {
                tracing::warn!("Failed to create session directory: {}", e);
                return;
            }

            // Build hierarchy nodes
            let hierarchy: Vec<HierarchyNode> = session.agents.iter().map(|agent| {
                let role_str = match &agent.role {
                    AgentRole::MasterPlanner => "MasterPlanner".to_string(),
                    AgentRole::Queen => "Queen".to_string(),
                    AgentRole::Planner { index } => format!("Planner-{}", index),
                    AgentRole::Worker { index, .. } => format!("Worker-{}", index),
                    AgentRole::Fusion { variant } => format!("Fusion-{}", variant),
                };

                let children: Vec<String> = session.agents.iter()
                    .filter(|a| a.parent_id.as_ref() == Some(&agent.id))
                    .map(|a| a.id.clone())
                    .collect();

                HierarchyNode {
                    id: agent.id.clone(),
                    role: role_str,
                    parent_id: agent.parent_id.clone(),
                    children,
                }
            }).collect();

            // Build worker state info
            let workers: Vec<WorkerStateInfo> = session.agents.iter()
                .filter(|a| !matches!(a.role, AgentRole::Queen))
                .map(|a| WorkerStateInfo {
                    id: a.id.clone(),
                    role: a.config.role.clone().unwrap_or_default(),
                    cli: a.config.cli.clone(),
                    status: format!("{:?}", a.status),
                    current_task: None,
                    last_update: Utc::now(),
                })
                .collect();

            // Update state files
            let state_manager = StateManager::new(storage.session_dir(&session.id));
            if let Err(e) = state_manager.update_hierarchy(&hierarchy) {
                tracing::warn!("Failed to update hierarchy: {}", e);
            }
            if let Err(e) = state_manager.update_workers_file(&workers) {
                tracing::warn!("Failed to update workers file: {}", e);
            }
        }
    }

    /// Update session storage after changes
    fn update_session_storage(&self, session_id: &str) {
        if let Some(ref storage) = self.storage {
            let sessions = self.sessions.read();
            if let Some(session) = sessions.get(session_id) {
                // Build hierarchy nodes
                let hierarchy: Vec<HierarchyNode> = session.agents.iter().map(|agent| {
                    let role_str = match &agent.role {
                        AgentRole::MasterPlanner => "MasterPlanner".to_string(),
                        AgentRole::Queen => "Queen".to_string(),
                        AgentRole::Planner { index } => format!("Planner-{}", index),
                        AgentRole::Worker { index, .. } => format!("Worker-{}", index),
                        AgentRole::Fusion { variant } => format!("Fusion-{}", variant),
                    };

                    let children: Vec<String> = session.agents.iter()
                        .filter(|a| a.parent_id.as_ref() == Some(&agent.id))
                        .map(|a| a.id.clone())
                        .collect();

                    HierarchyNode {
                        id: agent.id.clone(),
                        role: role_str,
                        parent_id: agent.parent_id.clone(),
                        children,
                    }
                }).collect();

                // Build worker state info
                let workers: Vec<WorkerStateInfo> = session.agents.iter()
                    .filter(|a| !matches!(a.role, AgentRole::Queen))
                    .map(|a| WorkerStateInfo {
                        id: a.id.clone(),
                        role: a.config.role.clone().unwrap_or_default(),
                        cli: a.config.cli.clone(),
                        status: format!("{:?}", a.status),
                        current_task: None,
                        last_update: Utc::now(),
                    })
                    .collect();

                // Update state files
                let state_manager = StateManager::new(storage.session_dir(session_id));
                if let Err(e) = state_manager.update_hierarchy(&hierarchy) {
                    tracing::warn!("Failed to update hierarchy: {}", e);
                }
                if let Err(e) = state_manager.update_workers_file(&workers) {
                    tracing::warn!("Failed to update workers file: {}", e);
                }
            }
        }
    }
}

impl Default for SessionController {
    fn default() -> Self {
        Self::new(Arc::new(RwLock::new(PtyManager::new())))
    }
}
