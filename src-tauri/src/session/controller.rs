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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmLaunchConfig {
    pub project_path: String,
    pub queen_config: AgentConfig,
    pub planners: Vec<PlannerConfig>,
    pub prompt: Option<String>,
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

    /// Build the Queen's master prompt with worker information
    fn build_queen_master_prompt(session_id: &str, workers: &[AgentConfig], user_prompt: Option<&str>) -> String {
        let mut worker_list = String::new();
        for (i, worker_config) in workers.iter().enumerate() {
            let index = i + 1;
            let worker_id = format!("{}-worker-{}", session_id, index);
            let role_label = worker_config.role.as_ref()
                .map(|r| format!("Worker {} ({})", index, r.label))
                .unwrap_or_else(|| format!("Worker {}", index));
            worker_list.push_str(&format!("| {} | {} | {} |\n", worker_id, role_label, worker_config.cli));
        }

        format!(
r#"# Queen Agent - Hive Manager Session

You are the **Queen** orchestrating a multi-agent Hive session. You have full Claude Code capabilities plus coordination tools.

## Session Info

- **Session ID**: {session_id}
- **Prompts Directory**: `.hive-manager/{session_id}/prompts/`

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
You can use any /commands in `~/.claude/commands/`, including:
- `/scout` - Search codebase for relevant files
- `/plan` - Generate implementation plans
- `/commit` - Create git commits
- And any custom commands available

### Hive Coordination
To assign tasks to workers, you have two options:

**Option 1: File-Based Tasks (Recommended)**
Write task files that workers will receive:
```
Write to: .hive-manager/{session_id}/prompts/worker-N-task.md
```
The operator will inject the task into the worker's terminal.

**Option 2: Direct Request**
Tell the operator what to inject:
```
@Operator: Please tell Worker 1 to implement the user auth API
```

## Coordination Protocol

1. **Plan the work** - Break down the task into worker assignments
2. **Write task files** - Create detailed task files for each worker
3. **Monitor progress** - Workers will update their status
4. **Review & integrate** - Review worker output and coordinate integration
5. **Commit & push** - You handle final commits (workers don't push)

## Your Task

{task}"#,
            session_id = session_id,
            worker_list = worker_list,
            task = user_prompt.unwrap_or("Awaiting instructions from the operator.")
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

If the status is STANDBY, wait for the Queen to assign you a task by updating that file."#,
            index = index,
            role_name = role_name,
            role_description = role_description,
            queen_id = queen_id,
            task_file = task_file
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

    /// Get the task file path for a worker
    fn get_task_file_path(project_path: &PathBuf, session_id: &str, worker_index: u8) -> PathBuf {
        project_path
            .join(".hive-manager")
            .join(session_id)
            .join("tasks")
            .join(format!("worker-{}-task.md", worker_index))
    }

    pub fn launch_hive_v2(&self, config: HiveLaunchConfig) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();
        let mut agents = Vec::new();
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent
            let queen_id = format!("{}-queen", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config);

            // Write Queen prompt to file and pass to CLI
            let master_prompt = Self::build_queen_master_prompt(&session_id, &config.workers, config.prompt.as_deref());
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

    pub fn launch_swarm(&self, config: SwarmLaunchConfig) -> Result<Session, String> {
        let session_id = Uuid::new_v4().to_string();
        let mut agents = Vec::new();
        let project_path = PathBuf::from(&config.project_path);
        let cwd = config.project_path.as_str();

        {
            let pty_manager = self.pty_manager.read();

            // Create Queen agent
            let queen_id = format!("{}-queen", session_id);
            let (cmd, mut args) = Self::build_command(&config.queen_config);

            // Write Queen prompt to file and pass to CLI
            let master_prompt = Self::build_swarm_queen_prompt(&session_id, &config.planners, config.prompt.as_deref());
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
            for (planner_idx, planner_config) in config.planners.iter().enumerate() {
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
            session_type: SessionType::Swarm { planner_count: config.planners.len() as u8 },
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
