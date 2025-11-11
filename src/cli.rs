use crate::{claude, config, git, workflow};
use anyhow::{Context, Result, anyhow};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use std::io::{self, Write};

#[derive(Clone, Debug)]
struct WorktreeBranchParser;

impl WorktreeBranchParser {
    fn new() -> Self {
        Self
    }

    fn get_branches(&self) -> Vec<String> {
        // Don't attempt completions if not in a git repo.
        if !git::is_git_repo().unwrap_or(false) {
            return Vec::new();
        }

        let worktrees = match git::list_worktrees() {
            Ok(wt) => wt,
            // Fail silently on completion; don't disrupt the user's shell.
            Err(_) => return Vec::new(),
        };

        let main_branch = git::get_default_branch().ok();

        worktrees
            .into_iter()
            .map(|(_, branch)| branch)
            // Filter out the main branch, as it's not a candidate for merging/removing.
            .filter(|branch| main_branch.as_deref() != Some(branch.as_str()))
            // Filter out detached HEAD states.
            .filter(|branch| branch != "(detached)")
            .collect()
    }
}

impl clap::builder::TypedValueParser for WorktreeBranchParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        _arg: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, clap::Error> {
        // Use the default string parser for validation.
        clap::builder::StringValueParser::new().parse_ref(cmd, None, value)
    }

    fn possible_values(
        &self,
    ) -> Option<Box<dyn Iterator<Item = clap::builder::PossibleValue> + '_>> {
        let branches = self.get_branches();

        let branches_static: Vec<&'static str> = branches
            .into_iter()
            .map(|s| Box::leak(s.into_boxed_str()) as &'static str)
            .collect();

        Some(Box::new(
            branches_static
                .into_iter()
                .map(clap::builder::PossibleValue::new),
        ))
    }
}

#[derive(clap::Args, Debug)]
struct RemoveArgs {
    /// Name of the branch to remove (defaults to current branch)
    #[arg(value_parser = WorktreeBranchParser::new())]
    branch_name: Option<String>,

    /// Skip confirmation and ignore uncommitted changes
    #[arg(short, long)]
    force: bool,

    /// Also delete the remote branch
    #[arg(short = 'r', long)]
    delete_remote: bool,
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(name = "workmux")]
#[command(about = "An opinionated workflow tool that orchestrates git worktrees and tmux")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new worktree and tmux window
    Add {
        /// Name of the branch (creates if it doesn't exist)
        branch_name: String,

        /// Base branch/commit/tag to branch from
        #[arg(long)]
        from: Option<String>,
    },

    /// Open a tmux window for an existing worktree
    Open {
        /// Name of the branch with an existing worktree
        #[arg(value_parser = WorktreeBranchParser::new())]
        branch_name: String,

        /// Re-run post-create hooks (e.g., pnpm install)
        #[arg(long)]
        run_hooks: bool,

        /// Re-apply file operations (copy/symlink)
        #[arg(long)]
        force_files: bool,
    },

    /// Merge a branch, then clean up the worktree and tmux window
    Merge {
        /// Name of the branch to merge (defaults to current branch)
        #[arg(value_parser = WorktreeBranchParser::new())]
        branch_name: Option<String>,

        /// Ignore uncommitted and staged changes
        #[arg(long)]
        ignore_uncommitted: bool,

        /// Also delete the remote branch
        #[arg(short = 'r', long)]
        delete_remote: bool,

        /// Rebase the branch onto the main branch before merging (fast-forward)
        #[arg(long, group = "merge_strategy")]
        rebase: bool,

        /// Squash all commits from the branch into a single commit on the main branch
        #[arg(long, group = "merge_strategy")]
        squash: bool,
    },

    /// Remove a worktree, tmux window, and branch without merging
    #[command(visible_alias = "rm")]
    Remove(RemoveArgs),

    /// List all worktrees
    #[command(visible_alias = "ls")]
    List,

    /// Generate example .workmux.yaml configuration file
    Init,

    /// Claude Code integration commands
    Claude {
        #[command(subcommand)]
        command: ClaudeCommands,
    },

    /// Generate shell completions
    Completions {
        /// The shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum ClaudeCommands {
    /// Remove stale entries from ~/.claude.json for deleted worktrees
    Prune,
}

// --- Public Entry Point ---
pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Add { branch_name, from } => create_worktree(&branch_name, from.as_deref()),
        Commands::Open {
            branch_name,
            run_hooks,
            force_files,
        } => open_worktree(&branch_name, run_hooks, force_files),
        Commands::Merge {
            branch_name,
            ignore_uncommitted,
            delete_remote,
            rebase,
            squash,
        } => merge_worktree(
            branch_name.as_deref(),
            ignore_uncommitted,
            delete_remote,
            rebase,
            squash,
        ),
        Commands::Remove(args) => {
            remove_worktree(args.branch_name.as_deref(), args.force, args.delete_remote)
        }
        Commands::List => list_worktrees(),
        Commands::Init => config::Config::init(),
        Commands::Claude { command } => match command {
            ClaudeCommands::Prune => prune_claude_config(),
        },
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            generate(shell, &mut cmd, name, &mut io::stdout());
            Ok(())
        }
    }
}

fn create_worktree(branch_name: &str, base_branch: Option<&str>) -> Result<()> {
    let config = config::Config::load()?;

    // Print setup status if there are post-create hooks
    if config.post_create.as_ref().is_some_and(|v| !v.is_empty()) {
        println!("Running setup commands...");
    }

    let result = workflow::create(branch_name, base_branch, &config)
        .context("Failed to create worktree environment")?;

    if result.post_create_hooks_run > 0 {
        println!("✓ Setup complete");
    }

    println!(
        "✓ Successfully created worktree and tmux window for '{}'",
        result.branch_name
    );
    if let Some(ref base) = result.base_branch {
        println!("  Base: {}", base);
    }
    println!("  Worktree: {}", result.worktree_path.display());

    Ok(())
}

fn open_worktree(branch_name: &str, run_hooks: bool, force_files: bool) -> Result<()> {
    let config = config::Config::load()?;

    if run_hooks && config.post_create.as_ref().is_some_and(|v| !v.is_empty()) {
        println!("Running setup commands...");
    }

    let result = workflow::open(branch_name, run_hooks, force_files, &config)
        .context("Failed to open worktree environment")?;

    if result.post_create_hooks_run > 0 {
        println!("✓ Setup complete");
    }

    println!(
        "✓ Successfully opened tmux window for '{}'\n  Worktree: {}",
        result.branch_name,
        result.worktree_path.display()
    );

    Ok(())
}

fn merge_worktree(
    branch_name: Option<&str>,
    ignore_uncommitted: bool,
    delete_remote: bool,
    rebase: bool,
    squash: bool,
) -> Result<()> {
    let config = config::Config::load()?;

    // Print status if there are pre-delete hooks
    if config.pre_delete.as_ref().is_some_and(|v| !v.is_empty()) {
        println!("Running pre-delete commands...");
    }

    let result = workflow::merge(
        branch_name,
        ignore_uncommitted,
        delete_remote,
        rebase,
        squash,
        &config,
    )
    .context("Failed to merge worktree")?;

    if result.had_staged_changes {
        println!("✓ Committed staged changes");
    }

    println!(
        "Merging '{}' into '{}'...",
        result.branch_merged, result.main_branch
    );
    println!("✓ Merged '{}'", result.branch_merged);

    println!(
        "✓ Successfully merged and cleaned up '{}'",
        result.branch_merged
    );

    Ok(())
}

fn remove_worktree(branch_name: Option<&str>, mut force: bool, delete_remote: bool) -> Result<()> {
    // Determine the branch to remove
    let branch_to_remove = if let Some(name) = branch_name {
        name.to_string()
    } else {
        // Running from within a worktree - get current branch
        git::get_current_branch().context("Failed to get current branch")?
    };

    // Handle user confirmation prompt if needed (before calling workflow)
    if !force {
        // First check for uncommitted changes (must be checked before unmerged prompt)
        // to avoid prompting user about unmerged commits only to error on uncommitted changes
        if let Ok(worktree_path) = git::get_worktree_path(&branch_to_remove)
            && git::has_uncommitted_changes(&worktree_path)?
        {
            return Err(anyhow!(
                "Worktree has uncommitted changes. Use --force to delete anyway."
            ));
        }

        // Check if we need to prompt for unmerged commits
        let main_branch = git::get_default_branch()?;
        let base_branch = git::get_merge_base(&main_branch)?;
        let unmerged_branches = git::get_unmerged_branches(&base_branch)?;
        let has_unmerged = unmerged_branches.contains(&branch_to_remove);

        if has_unmerged {
            println!(
                "This will delete the worktree, tmux window, and local branch for '{}'.",
                branch_to_remove
            );
            if delete_remote {
                println!("The remote branch will also be deleted.");
            }
            println!(
                "Warning: Branch '{}' has commits that are not merged into '{}'.",
                branch_to_remove, base_branch
            );
            println!("This action cannot be undone.");
            print!("Are you sure you want to continue? [y/N] ");

            // Flush stdout to ensure the prompt is displayed before reading input
            io::stdout().flush()?;

            let mut confirmation = String::new();
            io::stdin().read_line(&mut confirmation)?;

            if confirmation.trim().to_lowercase() != "y" {
                println!("Aborted.");
                return Ok(());
            }

            // User confirmed deletion of unmerged branch - treat as force for git operations
            // This is safe because we already verified there are no uncommitted changes above
            force = true;
        }
    }

    let config = config::Config::load()?;

    // Print status if there are pre-delete hooks
    if config.pre_delete.as_ref().is_some_and(|v| !v.is_empty()) {
        println!("Running pre-delete commands...");
    }

    let result = workflow::remove(&branch_to_remove, force, delete_remote, &config)
        .context("Failed to remove worktree")?;

    println!(
        "✓ Successfully removed worktree and branch '{}'",
        result.branch_removed
    );

    Ok(())
}

fn list_worktrees() -> Result<()> {
    let config = config::Config::load()?;
    let worktrees = workflow::list(&config)?;

    if worktrees.is_empty() {
        println!("No worktrees found");
        return Ok(());
    }

    // Prepare display data
    struct DisplayInfo {
        branch: String,
        path_str: String,
        tmux_status: String,
        unmerged_status: String,
    }

    let display_data: Vec<DisplayInfo> = worktrees
        .into_iter()
        .map(|wt| DisplayInfo {
            branch: wt.branch,
            path_str: wt.path.display().to_string(),
            tmux_status: if wt.has_tmux {
                "✓".to_string()
            } else {
                "-".to_string()
            },
            unmerged_status: if wt.has_unmerged {
                "●".to_string()
            } else {
                "-".to_string()
            },
        })
        .collect();

    const BRANCH_HEADER: &str = "BRANCH";
    const TMUX_HEADER: &str = "TMUX";
    const UNMERGED_HEADER: &str = "UNMERGED";
    const PATH_HEADER: &str = "PATH";

    // Determine column widths based on the longest content in each column
    let max_branch_width = display_data
        .iter()
        .map(|wt| wt.branch.len())
        .max()
        .unwrap_or(0)
        .max(BRANCH_HEADER.len());

    // Add padding for visual separation
    let branch_col_width = max_branch_width + 4;
    let tmux_col_width = TMUX_HEADER.len() + 4;
    let unmerged_col_width = UNMERGED_HEADER.len() + 4;

    // Print Header
    println!(
        "{:<branch_width$}{:<tmux_width$}{:<unmerged_width$}{}",
        BRANCH_HEADER,
        TMUX_HEADER,
        UNMERGED_HEADER,
        PATH_HEADER,
        branch_width = branch_col_width,
        tmux_width = tmux_col_width,
        unmerged_width = unmerged_col_width
    );

    // Print Separator
    let branch_separator = "-".repeat(max_branch_width);
    let tmux_separator = "-".repeat(TMUX_HEADER.len());
    let unmerged_separator = "-".repeat(UNMERGED_HEADER.len());
    let path_separator = "-".repeat(PATH_HEADER.len());
    println!(
        "{:<branch_width$}{:<tmux_width$}{:<unmerged_width$}{}",
        branch_separator,
        tmux_separator,
        unmerged_separator,
        path_separator,
        branch_width = branch_col_width,
        tmux_width = tmux_col_width,
        unmerged_width = unmerged_col_width,
    );

    // Print Data Rows
    for wt in display_data {
        println!(
            "{:<branch_width$}{:<tmux_width$}{:<unmerged_width$}{}",
            wt.branch,
            wt.tmux_status,
            wt.unmerged_status,
            wt.path_str,
            branch_width = branch_col_width,
            tmux_width = tmux_col_width,
            unmerged_width = unmerged_col_width
        );
    }

    Ok(())
}

fn prune_claude_config() -> Result<()> {
    claude::prune_stale_entries().context("Failed to prune Claude configuration")?;
    Ok(())
}
