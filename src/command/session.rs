//! Session management commands for Claude session tracking

use crate::{claude, git};
use anyhow::{Context, Result};

/// List tracked sessions for the current repository, or all registered repos.
pub fn list(all: bool) -> Result<()> {
    if all {
        return list_all();
    }

    let repo_root = git::get_main_worktree_root().context("Not in a git repository")?;

    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("Could not determine repository name"))?;

    let worktrees = git::list_worktrees()?;
    let sessions = claude::list_sessions_for_worktrees(&worktrees)?;

    if sessions.is_empty() {
        println!("No sessions found for {}", repo_name);
        return Ok(());
    }

    println!("Sessions for {}:", repo_name);
    for session in sessions {
        let session_id = session.session_id.as_deref().unwrap_or("(no session)");
        println!("  {}  {}", session.branch, session_id);
    }

    Ok(())
}

/// List sessions across all registered repositories.
fn list_all() -> Result<()> {
    let repos = claude::list_all_repos()?;

    if repos.is_empty() {
        println!("No registered repositories found.");
        println!("Repositories are registered automatically when worktrees are created.");
        return Ok(());
    }

    let original_dir = std::env::current_dir().ok();
    let mut any_sessions = false;

    for (repo_name, repo_path) in &repos {
        if let Err(e) = std::env::set_current_dir(repo_path) {
            tracing::warn!(repo = %repo_name, error = %e, "Failed to enter repo directory");
            continue;
        }

        let worktrees = match git::list_worktrees() {
            Ok(wt) => wt,
            Err(e) => {
                tracing::warn!(repo = %repo_name, error = %e, "Failed to list worktrees");
                continue;
            }
        };

        let sessions = match claude::list_sessions_for_worktrees(&worktrees) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(repo = %repo_name, error = %e, "Failed to list sessions");
                continue;
            }
        };

        if sessions.is_empty() {
            continue;
        }

        any_sessions = true;
        println!("{}  ({})", repo_name, repo_path.display());
        for session in sessions {
            let session_id = session.session_id.as_deref().unwrap_or("(no session)");
            println!("  {}  {}", session.branch, session_id);
        }
        println!();
    }

    if let Some(dir) = original_dir {
        let _ = std::env::set_current_dir(dir);
    }

    if !any_sessions {
        println!(
            "No sessions tracked across {} registered repositories.",
            repos.len()
        );
    }

    Ok(())
}
