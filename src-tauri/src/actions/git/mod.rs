//! Git actions: wrap the existing git free functions behind the unified
//! [`Action`] contract.
//!
//! The [`run_git_in_dir`] helper (and its load-bearing `#[cfg(windows)]`
//! `CREATE_NO_WINDOW` creation flag, which prevents a console window from
//! flashing on Windows) lives here as the single source of truth.
//! `commands/git_commands.rs` re-exports the types and helper from this module.

use std::path::Path;
use std::process::Command;

use async_trait::async_trait;
use schemars::schema::RootSchema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::ActionError;
use super::registry::{Action, ActionRegistry};
use super::ActionContext;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub short_hash: String,
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    pub head: String,
    pub is_bare: bool,
}

/// Run a git command in `project_path`, returning stdout on success or a
/// human-readable error string on failure.
///
/// IMPORTANT (load-bearing): the `#[cfg(windows)]` `CREATE_NO_WINDOW` creation
/// flag must remain — without it git spawns a flashing console window on Windows.
pub fn run_git_in_dir(args: &[&str], project_path: &str) -> Result<String, String> {
    let path = Path::new(project_path);
    if !path.exists() {
        return Err(format!("Project path does not exist: {}", project_path));
    }

    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(path);

    // Prevent console window from flashing on Windows
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        return Err(if message.is_empty() {
            "Git command failed".to_string()
        } else {
            message
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn parse_worktree_list(output: &str) -> Result<Vec<WorktreeInfo>, String> {
    let mut worktrees = Vec::new();
    let mut current: Option<WorktreeInfo> = None;

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            if let Some(info) = current.take() {
                if info.path.is_empty() {
                    return Err("Unexpected git worktree output: missing path".to_string());
                }
                worktrees.push(info);
            }
            continue;
        }

        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(info) = current.take() {
                if info.path.is_empty() {
                    return Err("Unexpected git worktree output: missing path".to_string());
                }
                worktrees.push(info);
            }

            current = Some(WorktreeInfo {
                path: path.to_string(),
                branch: String::new(),
                head: String::new(),
                is_bare: false,
            });
            continue;
        }

        let entry = current
            .as_mut()
            .ok_or_else(|| format!("Unexpected git worktree output: {}", line))?;

        if let Some(head) = line.strip_prefix("HEAD ") {
            entry.head = head.to_string();
        } else if let Some(branch) = line.strip_prefix("branch ") {
            entry.branch = branch
                .strip_prefix("refs/heads/")
                .unwrap_or(branch)
                .to_string();
        } else if line == "bare" {
            entry.is_bare = true;
        } else if line == "detached" && entry.branch.is_empty() {
            entry.branch = "detached".to_string();
        }
    }

    if let Some(info) = current.take() {
        if info.path.is_empty() {
            return Err("Unexpected git worktree output: missing path".to_string());
        }
        worktrees.push(info);
    }

    Ok(worktrees)
}

pub fn parse_branch_list(output: &str) -> Result<Vec<BranchInfo>, String> {
    let mut branches = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('|');
        let name = parts.next().unwrap_or("").trim();
        let short_hash = parts.next().unwrap_or("").trim();
        let head_marker = parts.next().unwrap_or("").trim();

        if name.is_empty() || short_hash.is_empty() {
            return Err(format!("Unexpected git branch output: {}", line));
        }

        branches.push(BranchInfo {
            name: name.to_string(),
            short_hash: short_hash.to_string(),
            is_current: head_marker == "*",
        });
    }
    Ok(branches)
}

// ---------------------------------------------------------------------------
// Input DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct ProjectPathInput {
    project_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SwitchBranchInput {
    project_path: String,
    branch: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WorktreeAddInput {
    project_path: String,
    worktree_path: String,
    branch: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WorktreeRemoveInput {
    project_path: String,
    worktree_path: String,
}

fn deserialize_input<T: for<'de> Deserialize<'de>>(input: Value) -> Result<T, ActionError> {
    serde_json::from_value(input)
        .map_err(|e| ActionError::bad_request(format!("Invalid input: {}", e)))
}

/// Map the string error from `run_git_in_dir`/parsers into an `ActionError`.
/// A non-existent project path is a bad request; everything else is internal.
fn git_err(message: String) -> ActionError {
    if message.starts_with("Project path does not exist") {
        ActionError::bad_request(message)
    } else {
        ActionError::internal(message)
    }
}

// ---------------------------------------------------------------------------
// git.list_branches
// ---------------------------------------------------------------------------

struct ListBranches;

#[async_trait]
impl Action for ListBranches {
    fn name(&self) -> &'static str {
        "git.list_branches"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(ProjectPathInput)
    }

    async fn run(&self, _ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: ProjectPathInput = deserialize_input(input)?;
        let output = run_git_in_dir(
            &[
                "branch",
                "--list",
                "--format=%(refname:short)|%(objectname:short)|%(HEAD)",
            ],
            &parsed.project_path,
        )
        .map_err(git_err)?;
        let branches = parse_branch_list(&output).map_err(git_err)?;
        serde_json::to_value(branches)
            .map_err(|e| ActionError::internal(format!("Failed to serialize branches: {}", e)))
    }
}

// ---------------------------------------------------------------------------
// git.current_branch
// ---------------------------------------------------------------------------

struct CurrentBranch;

#[async_trait]
impl Action for CurrentBranch {
    fn name(&self) -> &'static str {
        "git.current_branch"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(ProjectPathInput)
    }

    async fn run(&self, _ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: ProjectPathInput = deserialize_input(input)?;
        let output =
            run_git_in_dir(&["rev-parse", "--abbrev-ref", "HEAD"], &parsed.project_path)
                .map_err(git_err)?;
        let branch = output.trim();
        if branch.is_empty() {
            return Err(ActionError::internal("Unable to determine current branch"));
        }
        Ok(Value::String(branch.to_string()))
    }
}

// ---------------------------------------------------------------------------
// git.switch_branch
// ---------------------------------------------------------------------------

struct SwitchBranch;

#[async_trait]
impl Action for SwitchBranch {
    fn name(&self) -> &'static str {
        "git.switch_branch"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(SwitchBranchInput)
    }

    async fn run(&self, _ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: SwitchBranchInput = deserialize_input(input)?;
        let branch = parsed.branch.trim();
        if branch.is_empty() {
            return Err(ActionError::bad_request("Branch name cannot be empty"));
        }

        let status =
            run_git_in_dir(&["status", "--porcelain"], &parsed.project_path).map_err(git_err)?;
        if !status.trim().is_empty() {
            return Err(ActionError::bad_request(
                "Uncommitted changes detected. Please commit or stash before switching branches.",
            ));
        }

        run_git_in_dir(&["switch", branch], &parsed.project_path).map_err(git_err)?;
        Ok(Value::Null)
    }
}

// ---------------------------------------------------------------------------
// git.pull
// ---------------------------------------------------------------------------

struct Pull;

#[async_trait]
impl Action for Pull {
    fn name(&self) -> &'static str {
        "git.pull"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(ProjectPathInput)
    }

    async fn run(&self, _ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: ProjectPathInput = deserialize_input(input)?;
        let status =
            run_git_in_dir(&["status", "--porcelain"], &parsed.project_path).map_err(git_err)?;
        if !status.trim().is_empty() {
            return Err(ActionError::bad_request(
                "Uncommitted changes detected. Please commit or stash before pulling.",
            ));
        }
        let output = run_git_in_dir(&["pull"], &parsed.project_path).map_err(git_err)?;
        Ok(Value::String(output.trim().to_string()))
    }
}

// ---------------------------------------------------------------------------
// git.push
// ---------------------------------------------------------------------------

struct Push;

#[async_trait]
impl Action for Push {
    fn name(&self) -> &'static str {
        "git.push"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(ProjectPathInput)
    }

    async fn run(&self, _ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: ProjectPathInput = deserialize_input(input)?;
        let output = run_git_in_dir(&["push"], &parsed.project_path).map_err(git_err)?;
        Ok(Value::String(output.trim().to_string()))
    }
}

// ---------------------------------------------------------------------------
// git.fetch
// ---------------------------------------------------------------------------

struct Fetch;

#[async_trait]
impl Action for Fetch {
    fn name(&self) -> &'static str {
        "git.fetch"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(ProjectPathInput)
    }

    async fn run(&self, _ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: ProjectPathInput = deserialize_input(input)?;
        let output = run_git_in_dir(&["fetch", "--all"], &parsed.project_path).map_err(git_err)?;
        Ok(Value::String(output.trim().to_string()))
    }
}

// ---------------------------------------------------------------------------
// git.worktree_add
// ---------------------------------------------------------------------------

struct WorktreeAdd;

#[async_trait]
impl Action for WorktreeAdd {
    fn name(&self) -> &'static str {
        "git.worktree_add"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(WorktreeAddInput)
    }

    async fn run(&self, _ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: WorktreeAddInput = deserialize_input(input)?;
        let worktree_path = parsed.worktree_path.trim();
        if worktree_path.is_empty() {
            return Err(ActionError::bad_request("Worktree path cannot be empty"));
        }
        let branch = parsed.branch.trim();
        if branch.is_empty() {
            return Err(ActionError::bad_request("Branch name cannot be empty"));
        }

        run_git_in_dir(
            &["worktree", "add", worktree_path, "-b", branch],
            &parsed.project_path,
        )
        .map_err(git_err)?;
        Ok(Value::Null)
    }
}

// ---------------------------------------------------------------------------
// git.worktree_list
// ---------------------------------------------------------------------------

struct WorktreeList;

#[async_trait]
impl Action for WorktreeList {
    fn name(&self) -> &'static str {
        "git.worktree_list"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(ProjectPathInput)
    }

    async fn run(&self, _ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: ProjectPathInput = deserialize_input(input)?;
        let output = run_git_in_dir(
            &["worktree", "list", "--porcelain"],
            &parsed.project_path,
        )
        .map_err(git_err)?;
        let worktrees = parse_worktree_list(&output).map_err(git_err)?;
        serde_json::to_value(worktrees)
            .map_err(|e| ActionError::internal(format!("Failed to serialize worktrees: {}", e)))
    }
}

// ---------------------------------------------------------------------------
// git.worktree_remove
// ---------------------------------------------------------------------------

struct WorktreeRemove;

#[async_trait]
impl Action for WorktreeRemove {
    fn name(&self) -> &'static str {
        "git.worktree_remove"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(WorktreeRemoveInput)
    }

    async fn run(&self, _ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: WorktreeRemoveInput = deserialize_input(input)?;
        let worktree_path = parsed.worktree_path.trim();
        if worktree_path.is_empty() {
            return Err(ActionError::bad_request("Worktree path cannot be empty"));
        }

        match run_git_in_dir(
            &["worktree", "remove", worktree_path, "--force"],
            &parsed.project_path,
        ) {
            Ok(_) => Ok(Value::Null),
            Err(err) => {
                #[cfg(windows)]
                {
                    let lower = err.to_lowercase();
                    if lower.contains("in use")
                        || lower.contains("being used")
                        || lower.contains("permission denied")
                        || lower.contains("access is denied")
                    {
                        return Err(ActionError::internal(format!(
                            "Failed to remove worktree because files may still be open. Close terminals/editors using '{}' and retry. Git error: {}",
                            worktree_path, err
                        )));
                    }
                }

                Err(git_err(err))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// git.worktree_prune
// ---------------------------------------------------------------------------

struct WorktreePrune;

#[async_trait]
impl Action for WorktreePrune {
    fn name(&self) -> &'static str {
        "git.worktree_prune"
    }

    fn input_schema(&self) -> RootSchema {
        schemars::schema_for!(ProjectPathInput)
    }

    async fn run(&self, _ctx: &ActionContext, input: Value) -> Result<Value, ActionError> {
        let parsed: ProjectPathInput = deserialize_input(input)?;
        run_git_in_dir(&["worktree", "prune"], &parsed.project_path).map_err(git_err)?;
        Ok(Value::Null)
    }
}

/// Register every git action into the registry.
pub fn register(registry: &mut ActionRegistry) {
    registry.register(Box::new(ListBranches));
    registry.register(Box::new(CurrentBranch));
    registry.register(Box::new(SwitchBranch));
    registry.register(Box::new(Pull));
    registry.register(Box::new(Push));
    registry.register(Box::new(Fetch));
    registry.register(Box::new(WorktreeAdd));
    registry.register(Box::new(WorktreeList));
    registry.register(Box::new(WorktreeRemove));
    registry.register(Box::new(WorktreePrune));
}
