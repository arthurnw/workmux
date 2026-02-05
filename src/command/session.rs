//! Session management commands for Claude session tracking

use anyhow::{Context, Result};
use crate::{claude, git};

/// List tracked sessions for the current repository, or all registered repos.
pub fn list(all: bool) -> Result<()> {
    if all {
        return list_all();
    }

    let repo_root = git::get_main_worktree_root()
        .context("Not in a git repository")?;

    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("Could not determine repository name"))?;

    let sessions = claude::list_sessions(repo_name)?;
    let worktrees = git::list_worktrees()?;

    // Build a set of active worktree branches
    let active_branches: std::collections::HashSet<_> = worktrees
        .iter()
        .map(|(_, branch)| branch.as_str())
        .collect();

    if sessions.is_empty() {
        println!("No sessions tracked for {}", repo_name);
        return Ok(());
    }

    println!("Sessions for {}:", repo_name);
    for session in sessions {
        let status = if active_branches.contains(session.branch.as_str()) {
            "(active)"
        } else {
            "(worktree removed)"
        };
        let session_id = session.session_id.as_deref().unwrap_or("(no session id)");
        println!("  {}  {}  {}", session.branch, session_id, status);
    }

    Ok(())
}

/// List sessions across all registered repositories.
fn list_all() -> Result<()> {
    let repos = claude::list_all_repos()?;

    if repos.is_empty() {
        println!("No registered repositories found.");
        println!("Repositories are registered when worktrees are created with session capture enabled.");
        return Ok(());
    }

    let mut any_sessions = false;

    for (repo_name, repo_path) in &repos {
        let sessions = claude::list_sessions(repo_name)?;

        if sessions.is_empty() {
            continue;
        }

        any_sessions = true;
        println!("{}  ({})", repo_name, repo_path.display());
        for session in sessions {
            let session_id = session.session_id.as_deref().unwrap_or("(no session id)");
            println!("  {}  {}", session.branch, session_id);
        }
        println!();
    }

    if !any_sessions {
        println!("No sessions tracked across {} registered repositories.", repos.len());
    }

    Ok(())
}

/// Manually capture or set a session ID for a branch
pub fn capture(branch: &str, session_id: Option<&str>) -> Result<()> {
    let repo_root = git::get_main_worktree_root()
        .context("Not in a git repository")?;

    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("Could not determine repository name"))?;

    // Register repo path for --all discovery
    if let Err(e) = claude::store_repo_path(repo_name, &repo_root) {
        tracing::warn!(error = %e, "Failed to store repo path");
    }

    let final_session_id = match session_id {
        Some(id) => {
            // Validate and use provided ID
            claude::store_session(repo_name, branch, id)?;
            id.to_string()
        }
        None => {
            // Auto-detect most recent session
            claude::capture_latest_session(repo_name, branch)?
                .ok_or_else(|| anyhow::anyhow!("No Claude session found to capture"))?
        }
    };

    println!("Stored session ID for '{}': {}", branch, final_session_id);
    Ok(())
}
