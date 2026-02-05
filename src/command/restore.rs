//! Restore command - opens all worktrees with optional Claude session resumption

use anyhow::Result;
use crate::multiplexer::{create_backend, detect_backend};
use crate::workflow::{SetupOptions, WorkflowContext};
use crate::{claude, config, git, workflow};

pub fn run(dry_run: bool) -> Result<()> {
    let (config, config_location) = config::Config::load_with_location(None)?;
    let mux = create_backend(detect_backend());
    let context = WorkflowContext::new(config.clone(), mux, config_location)?;

    let repo_name = context
        .main_worktree_root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("Could not determine repository name"))?;

    let worktrees = git::list_worktrees()?;
    let main_worktree = git::get_main_worktree_root()?;

    println!("Restoring worktrees for {}...", repo_name);

    let mut restored = 0;
    let mut skipped = 0;

    for (wt_path, branch) in worktrees {
        // Skip the main worktree
        if wt_path == main_worktree {
            continue;
        }

        let handle = wt_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&branch);

        // Check if window already exists
        if context.mux.window_exists(&context.prefix, handle)? {
            println!("  {}: window already exists, skipping", handle);
            skipped += 1;
            continue;
        }

        // Check for stored session ID
        let session_id = if config.claude.capture_sessions {
            claude::get_session(repo_name, &branch)?
        } else {
            None
        };

        if dry_run {
            if let Some(ref id) = session_id {
                println!("  {}: would restore with session {}", handle, id);
            } else {
                println!("  {}: would open (no saved session)", handle);
            }
            continue;
        }

        // Open the worktree
        let options = SetupOptions::new(false, false, true);
        match workflow::open(&branch, &context, options, false) {
            Ok(_result) => {
                if let Some(ref id) = session_id {
                    println!("  {}: restored with session {}", handle, &id[..8.min(id.len())]);
                } else {
                    println!("  {}: opened (no saved session)", handle);
                }
                restored += 1;
            }
            Err(e) => {
                println!("  {}: failed to open - {}", handle, e);
            }
        }
    }

    if dry_run {
        println!("\nDry run complete. No changes made.");
    } else {
        println!("\nRestore complete: {} restored, {} skipped", restored, skipped);
    }

    Ok(())
}
