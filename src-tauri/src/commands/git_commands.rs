use serde::Serialize;
use std::env;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub short_hash: String,
    pub is_current: bool,
}

fn run_git(args: &[&str]) -> Result<String, String> {
    let cwd = env::current_dir().map_err(|e| format!("Failed to get current directory: {}", e))?;
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
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
pub async fn list_branches() -> Result<Vec<BranchInfo>, String> {
    let output = run_git(&[
        "branch",
        "--list",
        "--format=%(refname:short)|%(objectname:short)|%(HEAD)",
    ])?;

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
pub async fn get_current_branch() -> Result<String, String> {
    let output = run_git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = output.trim();
    if branch.is_empty() {
        return Err("Unable to determine current branch".to_string());
    }
    Ok(branch.to_string())
}

#[tauri::command]
pub async fn switch_branch(branch: String) -> Result<(), String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("Branch name cannot be empty".to_string());
    }

    let status = run_git(&["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err("Uncommitted changes detected. Please commit or stash before switching branches.".to_string());
    }

    run_git(&["switch", branch])?;
    Ok(())
}

#[tauri::command]
pub async fn git_pull() -> Result<String, String> {
    // Check for uncommitted changes first
    let status = run_git(&["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err("Uncommitted changes detected. Please commit or stash before pulling.".to_string());
    }

    let output = run_git(&["pull"])?;
    Ok(output.trim().to_string())
}

#[tauri::command]
pub async fn git_push() -> Result<String, String> {
    let output = run_git(&["push"])?;
    Ok(output.trim().to_string())
}

#[tauri::command]
pub async fn git_fetch() -> Result<String, String> {
    let output = run_git(&["fetch", "--all"])?;
    Ok(output.trim().to_string())
}
