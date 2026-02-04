use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub short_hash: String,
    pub is_current: bool,
}

fn run_git_in_dir(args: &[&str], project_path: &str) -> Result<String, String> {
    let path = Path::new(project_path);
    if !path.exists() {
        return Err(format!("Project path does not exist: {}", project_path));
    }

    let output = Command::new("git")
        .args(args)
        .current_dir(path)
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
