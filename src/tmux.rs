use anyhow::{Context, Result, anyhow};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use crate::cmd::Cmd;
use crate::config::{PaneConfig, SplitDirection};

/// Helper function to add prefix to window name
pub fn prefixed(prefix: &str, window_name: &str) -> String {
    format!("{}{}", prefix, window_name)
}

/// Get all tmux window names in a single call
pub fn get_all_window_names() -> Result<HashSet<String>> {
    // tmux list-windows may exit with error if no windows exist
    let windows = Cmd::new("tmux")
        .args(&["list-windows", "-F", "#{window_name}"])
        .run_and_capture_stdout()
        .unwrap_or_default(); // Return empty string if command fails

    Ok(windows.lines().map(String::from).collect())
}

/// Check if tmux server is running
pub fn is_running() -> Result<bool> {
    Cmd::new("tmux").arg("info").run_as_check()
}

/// Check if a tmux window with the given name exists
pub fn window_exists(prefix: &str, window_name: &str) -> Result<bool> {
    let prefixed_name = prefixed(prefix, window_name);
    let windows = Cmd::new("tmux")
        .args(&["list-windows", "-F", "#{window_name}"])
        .run_and_capture_stdout();

    match windows {
        Ok(output) => Ok(output.lines().any(|line| line == prefixed_name)),
        Err(_) => Ok(false), // If command fails, window doesn't exist
    }
}

/// Return the tmux window name for the current pane, if any
pub fn current_window_name() -> Result<Option<String>> {
    match Cmd::new("tmux")
        .args(&["display-message", "-p", "#{window_name}"])
        .run_and_capture_stdout()
    {
        Ok(name) => Ok(Some(name.trim().to_string())),
        Err(_) => Ok(None),
    }
}

/// Create a new tmux window with the given name and working directory
pub fn create_window(prefix: &str, window_name: &str, working_dir: &Path) -> Result<()> {
    let prefixed_name = prefixed(prefix, window_name);
    let working_dir_str = working_dir
        .to_str()
        .ok_or_else(|| anyhow!("Working directory path contains non-UTF8 characters"))?;

    Cmd::new("tmux")
        .args(&["new-window", "-n", &prefixed_name, "-c", working_dir_str])
        .run()
        .context("Failed to create tmux window")?;

    Ok(())
}

/// Select a specific pane
pub fn select_pane(prefix: &str, window_name: &str, pane_index: usize) -> Result<()> {
    let prefixed_name = prefixed(prefix, window_name);
    let target = format!("={}.{}", prefixed_name, pane_index);

    Cmd::new("tmux")
        .args(&["select-pane", "-t", &target])
        .run()
        .context("Failed to select pane")?;

    Ok(())
}

/// Select a specific window
pub fn select_window(prefix: &str, window_name: &str) -> Result<()> {
    let prefixed_name = prefixed(prefix, window_name);
    let target = format!("={}", prefixed_name);

    Cmd::new("tmux")
        .args(&["select-window", "-t", &target])
        .run()
        .context("Failed to select window")?;

    Ok(())
}

/// Kill a tmux window
pub fn kill_window(prefix: &str, window_name: &str) -> Result<()> {
    let prefixed_name = prefixed(prefix, window_name);
    let target = format!("={}", prefixed_name);

    Cmd::new("tmux")
        .args(&["kill-window", "-t", &target])
        .run()
        .context("Failed to kill tmux window")?;

    Ok(())
}

/// Schedule a tmux window to be killed after a short delay. This is useful when
/// the current command is running inside the window that needs to close.
pub fn schedule_window_close(prefix: &str, window_name: &str, delay: Duration) -> Result<()> {
    let prefixed_name = prefixed(prefix, window_name);
    let delay_secs = format!("{:.3}", delay.as_secs_f64());
    let script = format!(
        "sleep {delay}; tmux kill-window -t ={window} >/dev/null 2>&1",
        delay = delay_secs,
        window = prefixed_name
    );

    Cmd::new("tmux")
        .args(&["run-shell", &script])
        .run()
        .context("Failed to schedule tmux window close")?;

    Ok(())
}

/// Builds a shell command string for tmux that executes an optional user command
/// and then leaves an interactive shell open.
///
/// The escaping strategy uses POSIX-style quote escaping ('\'\'). This works
/// correctly with bash, zsh, fish, and other common shells.
pub fn build_startup_command(command: Option<&str>) -> Result<Option<String>> {
    let command = match command {
        Some(c) => c,
        None => return Ok(None),
    };

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

    // To run `user_command` and then `exec shell` inside a new shell instance,
    // we use the form: `$SHELL -c '<user_command>; exec $SHELL'`.
    // We must escape single quotes within the user command using POSIX-style escaping.
    let escaped_command = command.replace('\'', r#"'\''"#);

    let full_command = format!(
        "{shell} -c '{escaped_command}; exec {shell}'",
        shell = shell,
        escaped_command = escaped_command
    );

    Ok(Some(full_command))
}

/// Split a pane with optional command
pub fn split_pane_with_command(
    prefix: &str,
    window_name: &str,
    pane_index: usize,
    direction: &SplitDirection,
    working_dir: &Path,
    command: Option<&str>,
) -> Result<()> {
    let split_arg = match direction {
        SplitDirection::Horizontal => "-h",
        SplitDirection::Vertical => "-v",
    };

    let prefixed_name = prefixed(prefix, window_name);
    let target = format!("={}.{}", prefixed_name, pane_index);
    let working_dir_str = working_dir
        .to_str()
        .ok_or_else(|| anyhow!("Working directory path contains non-UTF8 characters"))?;

    let cmd = Cmd::new("tmux").args(&[
        "split-window",
        split_arg,
        "-t",
        &target,
        "-c",
        working_dir_str,
    ]);

    let cmd = if let Some(cmd_str) = command {
        cmd.arg(cmd_str)
    } else {
        cmd
    };

    cmd.run().context("Failed to split pane")?;
    Ok(())
}

/// Respawn a pane with a new command
pub fn respawn_pane(
    prefix: &str,
    window_name: &str,
    pane_index: usize,
    working_dir: &Path,
    command: &str,
) -> Result<()> {
    let prefixed_name = prefixed(prefix, window_name);
    let target = format!("={}.{}", prefixed_name, pane_index);
    let working_dir_str = working_dir
        .to_str()
        .ok_or_else(|| anyhow!("Working directory path contains non-UTF8 characters"))?;

    Cmd::new("tmux")
        .args(&[
            "respawn-pane",
            "-t",
            &target,
            "-c",
            working_dir_str,
            "-k",
            command,
        ])
        .run()
        .context("Failed to respawn pane")?;

    Ok(())
}

/// Result of setting up panes
pub struct PaneSetupResult {
    /// The index of the pane that should receive focus.
    pub focus_pane_index: usize,
}

/// Setup panes in a window according to configuration
pub fn setup_panes(
    prefix: &str,
    window_name: &str,
    panes: &[PaneConfig],
    working_dir: &Path,
) -> Result<PaneSetupResult> {
    if panes.is_empty() {
        return Ok(PaneSetupResult {
            focus_pane_index: 0,
        });
    }

    let mut focus_pane_index: Option<usize> = None;

    // Handle the first pane (index 0), which already exists from window creation
    if let Some(pane_config) = panes.first() {
        if let Some(cmd_str) = pane_config.command.as_deref()
            && let Some(startup_cmd) = build_startup_command(Some(cmd_str))?
        {
            respawn_pane(prefix, window_name, 0, working_dir, &startup_cmd)?;
        }
        if pane_config.focus {
            focus_pane_index = Some(0);
        }
    }

    let mut actual_pane_count = 1;

    // Create additional panes by splitting
    for (_i, pane_config) in panes.iter().enumerate().skip(1) {
        if let Some(ref direction) = pane_config.split {
            // Determine which pane to split
            let target_pane_to_split = pane_config.target.unwrap_or(actual_pane_count - 1);

            let startup_cmd = build_startup_command(pane_config.command.as_deref())?;

            split_pane_with_command(
                prefix,
                window_name,
                target_pane_to_split,
                direction,
                working_dir,
                startup_cmd.as_deref(),
            )?;

            let new_pane_index = actual_pane_count;

            if pane_config.focus {
                focus_pane_index = Some(new_pane_index);
            }
            actual_pane_count += 1;
        }
    }

    Ok(PaneSetupResult {
        focus_pane_index: focus_pane_index.unwrap_or(0),
    })
}
