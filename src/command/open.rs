use crate::command::args::PromptArgs;
use crate::multiplexer::{create_backend, detect_backend};
use crate::workflow::prompt_loader::{PromptLoadArgs, load_prompt};
use crate::workflow::{SetupOptions, WorkflowContext};
use crate::{claude, config, direnv, git, workflow};
use anyhow::{Context, Result, bail};

pub fn run(
    name: Option<&str>,
    run_hooks: bool,
    force_files: bool,
    new_window: bool,
    prompt_args: PromptArgs,
) -> Result<()> {
    // Resolve the worktree name
    let resolved_name = match (name, new_window) {
        (Some(n), _) => n.to_string(),
        (None, true) => super::resolve_name(None).context(
            "Could not infer current worktree. Run inside a worktree or provide a name.",
        )?,
        (None, false) => bail!("Worktree name is required unless --new is provided"),
    };

    let (config, config_location) = config::Config::load_with_location(None)?;
    let mux = create_backend(detect_backend());
    let context = WorkflowContext::new(config, mux, config_location)?;

    // Load prompt if any prompt argument is provided
    let prompt = load_prompt(&PromptLoadArgs {
        prompt_editor: prompt_args.prompt_editor,
        prompt_inline: prompt_args.prompt.as_deref(),
        prompt_file: prompt_args.prompt_file.as_ref(),
    })?;

    // Write prompt to temp file if provided
    // Use unique filename with timestamp to prevent race condition when opening multiple duplicates
    let prompt_file_path = if let Some(ref p) = prompt {
        let unique_name = format!(
            "{}-{}",
            resolved_name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        Some(crate::workflow::write_prompt_file(&unique_name, p)?)
    } else {
        None
    };

    // Construct setup options (pane commands always run on open)
    let mut options = SetupOptions::new(run_hooks, force_files, true);
    options.prompt_file_path = prompt_file_path;

    // Only announce hooks if we're forcing a new window (otherwise we might just switch)
    if new_window {
        super::announce_hooks(
            &context.config,
            Some(&options),
            super::HookPhase::PostCreate,
        );
    }

    // Get worktree path for trust and direnv operations
    let worktree_path = git::find_worktree(&resolved_name)
        .ok()
        .map(|(path, _)| path);

    // Auto-trust and direnv allow
    if let Some(ref wt_path) = worktree_path {
        if context.config.claude.auto_trust
            && let Err(e) = claude::trust_directory(wt_path)
        {
            tracing::warn!(error = %e, "Failed to trust directory in Claude");
        }

        if context.config.direnv.auto_allow
            && let Err(e) = direnv::auto_allow(wt_path)
        {
            tracing::warn!(error = %e, "Failed to auto-allow direnv");
        }
    }

    let result = workflow::open(&resolved_name, &context, options, new_window, None)
        .context("Failed to open worktree environment")?;

    if result.did_switch {
        println!(
            "✓ Switched to existing tmux window for '{}'\n  Worktree: {}",
            resolved_name,
            result.worktree_path.display()
        );
    } else {
        if result.post_create_hooks_run > 0 {
            println!("✓ Setup complete");
        }

        println!(
            "✓ Opened tmux window for '{}'\n  Worktree: {}",
            resolved_name,
            result.worktree_path.display()
        );
    }

    Ok(())
}
