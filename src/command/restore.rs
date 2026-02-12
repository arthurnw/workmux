//! Restore command - opens all worktrees with optional Claude session resumption

use crate::multiplexer::{create_backend, detect_backend};
use crate::state::{AgentState, StateStore};
use crate::workflow::{SetupOptions, WorkflowContext};
use crate::{claude, config, git, workflow};
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

pub fn run(dry_run: bool, all: bool) -> Result<()> {
    if all {
        return run_all(dry_run);
    }
    run_single_repo(dry_run)
}

fn run_single_repo(dry_run: bool) -> Result<()> {
    let (config, config_location) = config::Config::load_with_location(None)?;
    let mux = create_backend(detect_backend());
    let context = WorkflowContext::new(config.clone(), mux, config_location)?;

    let repo_name = context
        .main_worktree_root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("Could not determine repository name"))?;

    // Register repo path for --all discovery
    if let Err(e) = claude::store_repo_path(repo_name, &context.main_worktree_root) {
        tracing::warn!(error = %e, "Failed to store repo path");
    }

    // Register tmux session if running inside one
    if let Some(session) = context.mux.current_session()
        && let Err(e) = claude::store_tmux_session(repo_name, &session)
    {
        tracing::warn!(error = %e, "Failed to store tmux session");
    }

    let (restored, skipped) = restore_repo(&context, repo_name, &config, dry_run, None, None)?;

    if dry_run {
        println!("\nDry run complete. No changes made.");
    } else {
        println!(
            "\nRestore complete: {} restored, {} skipped",
            restored, skipped
        );
    }

    Ok(())
}

/// Restore worktrees for a single repository. Returns (restored, skipped) counts.
///
/// When `external_orphans` is provided (the `run_all` path), uses the pre-drained
/// orphan map instead of calling `drain_orphans()` internally. This prevents the
/// first repo from consuming and discarding orphan state that belongs to later repos.
fn restore_repo(
    context: &WorkflowContext,
    repo_name: &str,
    config: &config::Config,
    dry_run: bool,
    target_session: Option<&str>,
    external_orphans: Option<&mut HashMap<PathBuf, AgentState>>,
) -> Result<(usize, usize)> {
    let worktrees = git::list_worktrees()?;
    let main_worktree = git::get_main_worktree_root()?;

    // Filter to secondary worktrees only (skip main)
    let secondary_worktrees: Vec<_> = worktrees
        .into_iter()
        .filter(|(p, _)| p != &main_worktree)
        .collect();

    if secondary_worktrees.is_empty() {
        return Ok((0, 0));
    }

    // Pre-create the target session with the main worktree root as cwd,
    // so the session's initial window points at the main checkout rather
    // than the first restored worktree.
    if let Some(session) = target_session {
        context.mux.ensure_session(session, &main_worktree)?;
    }

    // Use external orphans if provided (run_all path), otherwise drain locally
    // (single-repo path). This ensures drain_orphans() is called only once
    // across all repos in the run_all case.
    let mut local_orphans;
    let orphan_map: &mut HashMap<PathBuf, AgentState> = if let Some(ext) = external_orphans {
        ext
    } else {
        local_orphans = {
            let store = StateStore::new().ok();
            let live = context.mux.get_all_live_pane_info().unwrap_or_default();
            store
                .and_then(|s| s.drain_orphans(&live).ok())
                .unwrap_or_default()
        };
        &mut local_orphans
    };

    println!("Restoring worktrees for {}...", repo_name);

    let mut restored = 0;
    let mut skipped = 0;

    for (wt_path, branch) in secondary_worktrees {
        let handle = wt_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&branch);

        // Check if window already exists (in target session if specified)
        if context
            .mux
            .window_exists_in_session(&context.prefix, handle, target_session)?
        {
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

        // Open the worktree with pre-collected orphan state (if any)
        let mut options = SetupOptions::new(false, false, true);
        options.resume_session_id = session_id.clone();
        options.focus_window = false;

        // Compute effective workdir (matches open.rs logic for config_rel_dir)
        let effective_workdir = if !context.config_rel_dir.as_os_str().is_empty() {
            let subdir = wt_path.join(&context.config_rel_dir);
            if subdir.exists() {
                subdir
            } else {
                wt_path.clone()
            }
        } else {
            wt_path.clone()
        };
        options.prior_agent_state = orphan_map.remove(&effective_workdir);

        match workflow::open(&branch, context, options, false, target_session) {
            Ok(_result) => {
                if let Some(ref id) = session_id {
                    println!(
                        "  {}: restored with session {}",
                        handle,
                        &id[..8.min(id.len())]
                    );
                } else {
                    println!("  {}: opened (no saved session)", handle);
                    if config.claude.capture_sessions
                        && let Err(e) = claude::spawn_session_capture(
                            repo_name,
                            &branch,
                            config.claude.capture_timeout,
                        )
                    {
                        tracing::warn!(error = %e, "Failed to spawn session capture");
                    }
                }
                restored += 1;
            }
            Err(e) => {
                println!("  {}: failed to open - {}", handle, e);
            }
        }
    }

    Ok((restored, skipped))
}

fn run_all(dry_run: bool) -> Result<()> {
    let repos = claude::list_all_repos()?;

    if repos.is_empty() {
        println!("No registered repositories found.");
        println!(
            "Repositories are registered when worktrees are created with session capture enabled,"
        );
        println!("or when 'workmux restore' is run from inside a repository.");
        return Ok(());
    }

    let original_dir = std::env::current_dir().ok();
    let mut total_restored = 0;
    let mut total_skipped = 0;
    let mut total_failed = 0;

    // Pre-drain ALL orphans once before processing any repos.
    // drain_orphans() deletes state files as it reads them, so calling it
    // per-repo would cause the first repo to consume all orphan data and
    // leave subsequent repos with no agent status to recover.
    let mut global_orphan_map = {
        let store = StateStore::new().ok();
        let mux = create_backend(detect_backend());
        let live = mux.get_all_live_pane_info().unwrap_or_default();
        store
            .and_then(|s| s.drain_orphans(&live).ok())
            .unwrap_or_default()
    };

    for (name, path) in &repos {
        // Change to repo directory so git/config operations work
        if let Err(e) = std::env::set_current_dir(path) {
            println!(
                "\n{}: failed to enter directory ({}) - {}",
                name,
                path.display(),
                e
            );
            total_failed += 1;
            continue;
        }

        let (config, config_location) = match config::Config::load_with_location(None) {
            Ok(c) => c,
            Err(e) => {
                println!("\n{}: failed to load config - {}", name, e);
                total_failed += 1;
                continue;
            }
        };

        let mux = create_backend(detect_backend());
        let context = match WorkflowContext::new(config.clone(), mux, config_location) {
            Ok(c) => c,
            Err(e) => {
                println!("\n{}: failed to initialize - {}", name, e);
                total_failed += 1;
                continue;
            }
        };

        // Look up stored tmux session, fall back to repo name
        let target_session = claude::get_tmux_session(name)
            .unwrap_or(None)
            .unwrap_or_else(|| name.to_string());

        match restore_repo(
            &context,
            name,
            &config,
            dry_run,
            Some(&target_session),
            Some(&mut global_orphan_map),
        ) {
            Ok((restored, skipped)) => {
                total_restored += restored;
                total_skipped += skipped;
            }
            Err(e) => {
                println!("\n{}: restore failed - {}", name, e);
                total_failed += 1;
            }
        }
    }

    // Restore original directory
    if let Some(dir) = original_dir {
        let _ = std::env::set_current_dir(dir);
    }

    if dry_run {
        println!(
            "\nDry run complete across {} repositories. No changes made.",
            repos.len()
        );
    } else {
        println!(
            "\nRestore complete across {} repositories: {} restored, {} skipped, {} failed",
            repos.len(),
            total_restored,
            total_skipped,
            total_failed,
        );
    }

    Ok(())
}
