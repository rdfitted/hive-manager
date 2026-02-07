use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub short_hash: String,
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    pub head: String,
    pub is_bare: bool,
}

fn run_git_in_dir(args: &[&str], project_path: &str) -> Result<String, String> {
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

fn parse_worktree_list(output: &str) -> Result<Vec<WorktreeInfo>, String> {
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

#[tauri::command]
pub async fn list_branches(project_path: String) -> Result<Vec<BranchInfo>, String> {
    let output = run_git_in_dir(
        &[
            "branch",
            "--list",
            "--format=%(refname:short)|%(objectname:short)|%(HEAD)",
        ],
        &project_path,
    )?;

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

#[tauri::command]
pub async fn get_current_branch(project_path: String) -> Result<String, String> {
    let output = run_git_in_dir(&["rev-parse", "--abbrev-ref", "HEAD"], &project_path)?;
    let branch = output.trim();
    if branch.is_empty() {
        return Err("Unable to determine current branch".to_string());
    }
    Ok(branch.to_string())
}

#[tauri::command]
pub async fn switch_branch(project_path: String, branch: String) -> Result<(), String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("Branch name cannot be empty".to_string());
    }

    let status = run_git_in_dir(&["status", "--porcelain"], &project_path)?;
    if !status.trim().is_empty() {
        return Err(
            "Uncommitted changes detected. Please commit or stash before switching branches."
                .to_string(),
        );
    }

    run_git_in_dir(&["switch", branch], &project_path)?;
    Ok(())
}

#[tauri::command]
pub async fn git_pull(project_path: String) -> Result<String, String> {
    // Check for uncommitted changes first
    let status = run_git_in_dir(&["status", "--porcelain"], &project_path)?;
    if !status.trim().is_empty() {
        return Err("Uncommitted changes detected. Please commit or stash before pulling.".to_string());
    }

    let output = run_git_in_dir(&["pull"], &project_path)?;
    Ok(output.trim().to_string())
}

#[tauri::command]
pub async fn git_push(project_path: String) -> Result<String, String> {
    let output = run_git_in_dir(&["push"], &project_path)?;
    Ok(output.trim().to_string())
}

#[tauri::command]
pub async fn git_fetch(project_path: String) -> Result<String, String> {
    let output = run_git_in_dir(&["fetch", "--all"], &project_path)?;
    Ok(output.trim().to_string())
}

#[tauri::command]
pub async fn git_worktree_add(
    project_path: String,
    worktree_path: String,
    branch: String,
) -> Result<(), String> {
    let worktree_path = worktree_path.trim();
    if worktree_path.is_empty() {
        return Err("Worktree path cannot be empty".to_string());
    }

    let branch = branch.trim();
    if branch.is_empty() {
        return Err("Branch name cannot be empty".to_string());
    }

    run_git_in_dir(
        &["worktree", "add", worktree_path, "-b", branch],
        &project_path,
    )?;
    Ok(())
}

#[tauri::command]
pub async fn git_worktree_list(project_path: String) -> Result<Vec<WorktreeInfo>, String> {
    let output = run_git_in_dir(&["worktree", "list", "--porcelain"], &project_path)?;
    parse_worktree_list(&output)
}

#[tauri::command]
pub async fn git_worktree_remove(project_path: String, worktree_path: String) -> Result<(), String> {
    let worktree_path = worktree_path.trim();
    if worktree_path.is_empty() {
        return Err("Worktree path cannot be empty".to_string());
    }

    match run_git_in_dir(
        &["worktree", "remove", worktree_path, "--force"],
        &project_path,
    ) {
        Ok(_) => Ok(()),
        Err(err) => {
            #[cfg(windows)]
            {
                let lower = err.to_lowercase();
                if lower.contains("in use")
                    || lower.contains("being used")
                    || lower.contains("permission denied")
                    || lower.contains("access is denied")
                {
                    return Err(format!(
                        "Failed to remove worktree because files may still be open. Close terminals/editors using '{}' and retry. Git error: {}",
                        worktree_path, err
                    ));
                }
            }

            Err(err)
        }
    }
}

#[tauri::command]
pub async fn git_worktree_prune(project_path: String) -> Result<(), String> {
    run_git_in_dir(&["worktree", "prune"], &project_path)?;
    Ok(())
}
