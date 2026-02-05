//! direnv integration for automatic .envrc approval

use anyhow::Result;
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::debug;

/// Run 'direnv allow' in a directory if .envrc exists and direnv is installed.
/// Silently succeeds if direnv is not installed or .envrc doesn't exist.
pub fn auto_allow(dir: &Path) -> Result<()> {
    let envrc = dir.join(".envrc");
    if !envrc.exists() {
        return Ok(());
    }

    // Check if direnv is available
    if which::which("direnv").is_err() {
        debug!("direnv not found, skipping auto-allow");
        return Ok(());
    }

    debug!(dir = %dir.display(), "running direnv allow");

    let status = Command::new("direnv")
        .arg("allow")
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => {
            debug!(dir = %dir.display(), "direnv allow succeeded");
        }
        Ok(s) => {
            debug!(dir = %dir.display(), code = ?s.code(), "direnv allow failed");
        }
        Err(e) => {
            debug!(dir = %dir.display(), error = %e, "direnv allow error");
        }
    }

    Ok(())
}
