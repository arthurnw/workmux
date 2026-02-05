//! Claude Code session ID capture and storage.
//!
//! Sessions are stored at: `~/.local/state/workmux/sessions/<repo>/<branch>/session_id`
//!
//! The capture mechanism works by monitoring `~/.claude/session-env/` for new directories,
//! which are created by Claude Code when a new session starts.

use anyhow::{Context, Result};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Information about a stored session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub branch: String,
    pub session_id: Option<String>,
}

/// Get the XDG state directory.
///
/// Checks XDG_STATE_HOME first, falls back to ~/.local/state.
fn get_state_dir() -> Result<PathBuf> {
    if let Ok(state_home) = std::env::var("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home));
    }

    if let Some(home_dir) = home::home_dir() {
        return Ok(home_dir.join(".local/state"));
    }

    anyhow::bail!("Could not determine state directory")
}

/// Get the sessions directory path.
///
/// Returns: `~/.local/state/workmux/sessions`
pub fn get_sessions_dir() -> Result<PathBuf> {
    Ok(get_state_dir()?.join("workmux").join("sessions"))
}

/// Get the path for a specific session file.
///
/// Returns: `~/.local/state/workmux/sessions/<repo>/<branch>/session_id`
pub fn get_session_path(repo: &str, branch: &str) -> Result<PathBuf> {
    Ok(get_sessions_dir()?.join(repo).join(branch).join("session_id"))
}

/// Store a session ID for a repo/branch combination.
///
/// Creates the directory structure if it doesn't exist.
/// Uses atomic write for crash safety.
pub fn store_session(repo: &str, branch: &str, session_id: &str) -> Result<()> {
    let path = get_session_path(repo, branch)?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create session directory: {}", parent.display()))?;
    }

    // Atomic write: temp file + rename
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, session_id)
        .with_context(|| format!("Failed to write temp session file: {}", tmp_path.display()))?;

    fs::rename(&tmp_path, &path)
        .with_context(|| format!("Failed to rename session file: {}", path.display()))?;

    info!(repo, branch, session_id, "Stored session ID");
    Ok(())
}

/// Retrieve a stored session ID for a repo/branch combination.
///
/// Returns None if no session is stored.
pub fn get_session(repo: &str, branch: &str) -> Result<Option<String>> {
    let path = get_session_path(repo, branch)?;

    match fs::read_to_string(&path) {
        Ok(content) => {
            let session_id = content.trim().to_string();
            if session_id.is_empty() {
                Ok(None)
            } else {
                Ok(Some(session_id))
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Failed to read session file: {}", path.display())),
    }
}

/// Remove session data for a repo/branch combination.
///
/// No-op if the session doesn't exist.
/// Also removes empty parent directories.
pub fn remove_session(repo: &str, branch: &str) -> Result<()> {
    let path = get_session_path(repo, branch)?;

    match fs::remove_file(&path) {
        Ok(()) => {
            debug!(repo, branch, "Removed session file");
            // Try to clean up empty directories
            if let Some(branch_dir) = path.parent() {
                let _ = fs::remove_dir(branch_dir); // Ignore errors (may not be empty)
                if let Some(repo_dir) = branch_dir.parent() {
                    // If no branch subdirectories remain, remove repo_path too
                    let has_branch_dirs = fs::read_dir(repo_dir)
                        .ok()
                        .map(|entries| {
                            entries
                                .filter_map(|e| e.ok())
                                .any(|e| e.path().is_dir())
                        })
                        .unwrap_or(false);

                    if !has_branch_dirs {
                        let repo_path_file = repo_dir.join("repo_path");
                        let _ = fs::remove_file(&repo_path_file);
                        debug!(repo, "Removed repo_path file (no branch dirs remain)");
                    }

                    let _ = fs::remove_dir(repo_dir); // Ignore errors
                }
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("Failed to remove session file"),
    }
}

/// List all sessions for a repository.
///
/// Returns session info for all branches that have session directories.
pub fn list_sessions(repo: &str) -> Result<Vec<SessionInfo>> {
    let repo_dir = get_sessions_dir()?.join(repo);

    if !repo_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    for entry in fs::read_dir(&repo_dir)
        .with_context(|| format!("Failed to read sessions directory: {}", repo_dir.display()))?
    {
        let entry = entry?;
        let branch_path = entry.path();

        if !branch_path.is_dir() {
            continue;
        }

        let branch = entry
            .file_name()
            .to_string_lossy()
            .to_string();

        let session_id_path = branch_path.join("session_id");
        let session_id = match fs::read_to_string(&session_id_path) {
            Ok(content) => {
                let id = content.trim().to_string();
                if id.is_empty() { None } else { Some(id) }
            }
            Err(_) => None,
        };

        sessions.push(SessionInfo { branch, session_id });
    }

    // Sort by branch name for consistent output
    sessions.sort_by(|a, b| a.branch.cmp(&b.branch));

    Ok(sessions)
}

/// Store the absolute path to a repository for cross-repo discovery.
///
/// Writes to: `~/.local/state/workmux/sessions/<repo>/repo_path`
/// Uses atomic write (temp + rename) for crash safety.
pub fn store_repo_path(repo: &str, repo_path: &Path) -> Result<()> {
    let path = get_sessions_dir()?.join(repo).join("repo_path");

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create session directory: {}", parent.display()))?;
    }

    let abs_path = repo_path
        .canonicalize()
        .unwrap_or_else(|_| repo_path.to_path_buf());

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, abs_path.to_string_lossy().as_bytes())
        .with_context(|| format!("Failed to write temp repo_path file: {}", tmp_path.display()))?;

    fs::rename(&tmp_path, &path)
        .with_context(|| format!("Failed to rename repo_path file: {}", path.display()))?;

    debug!(repo, path = %abs_path.display(), "Stored repo path");
    Ok(())
}

/// Retrieve the stored path for a repository.
///
/// Returns None if no repo_path file exists.
pub fn get_repo_path(repo: &str) -> Result<Option<PathBuf>> {
    let path = get_sessions_dir()?.join(repo).join("repo_path");

    match fs::read_to_string(&path) {
        Ok(content) => {
            let repo_path = content.trim().to_string();
            if repo_path.is_empty() {
                Ok(None)
            } else {
                Ok(Some(PathBuf::from(repo_path)))
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Failed to read repo_path file: {}", path.display())),
    }
}

/// List all registered repositories with valid paths.
///
/// Scans `sessions/*/repo_path`, validates paths exist on disk.
/// Returns sorted `(name, path)` pairs. Skips repos with missing
/// repo_path files or non-existent paths.
pub fn list_all_repos() -> Result<Vec<(String, PathBuf)>> {
    let sessions_dir = get_sessions_dir()?;

    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut repos = Vec::new();

    for entry in fs::read_dir(&sessions_dir)
        .with_context(|| format!("Failed to read sessions directory: {}", sessions_dir.display()))?
    {
        let entry = entry?;
        let repo_dir = entry.path();

        if !repo_dir.is_dir() {
            continue;
        }

        let repo_name = entry.file_name().to_string_lossy().to_string();
        let repo_path_file = repo_dir.join("repo_path");

        match fs::read_to_string(&repo_path_file) {
            Ok(content) => {
                let path = PathBuf::from(content.trim());
                if path.exists() {
                    repos.push((repo_name, path));
                } else {
                    warn!(repo = %repo_name, path = %path.display(), "Repo path no longer exists on disk");
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                debug!(repo = %repo_name, "No repo_path file, skipping");
            }
            Err(e) => {
                warn!(repo = %repo_name, error = %e, "Failed to read repo_path file");
            }
        }
    }

    repos.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(repos)
}

/// Validate that a string is a valid UUID format.
///
/// Expected format: 8-4-4-4-12 hexadecimal digits (e.g., "550e8400-e29b-41d4-a716-446655440000")
pub fn is_valid_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();

    if parts.len() != 5 {
        return false;
    }

    let expected_lengths = [8, 4, 4, 4, 12];

    for (part, &expected_len) in parts.iter().zip(expected_lengths.iter()) {
        if part.len() != expected_len {
            return false;
        }
        if !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
    }

    true
}

/// Get the Claude session-env directory path.
///
/// Returns: `~/.claude/session-env`
fn get_claude_session_env_dir() -> Option<PathBuf> {
    home::home_dir().map(|h| h.join(".claude").join("session-env"))
}

/// Count the number of session directories in `~/.claude/session-env/`.
pub fn count_session_dirs() -> Result<usize> {
    let session_env_dir = get_claude_session_env_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

    if !session_env_dir.exists() {
        return Ok(0);
    }

    let count = fs::read_dir(&session_env_dir)
        .with_context(|| format!("Failed to read session-env directory: {}", session_env_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .count();

    Ok(count)
}

/// Spawn a detached subprocess to capture the Claude session ID.
///
/// Uses self re-exec with `_internal capture-session` command.
/// The subprocess will:
/// 1. Wait 5 seconds for Claude to start
/// 2. Poll `~/.claude/session-env/` every 2 seconds
/// 3. When a new directory appears, extract and store the session ID
/// 4. Exit after timeout or success
#[cfg(unix)]
pub fn spawn_session_capture(repo: &str, branch: &str, timeout_secs: u32) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let initial_count = count_session_dirs()?;
    let exe = std::env::current_exe().context("Failed to get current executable path")?;

    debug!(
        repo,
        branch,
        initial_count,
        timeout_secs,
        "Spawning session capture subprocess"
    );

    let mut cmd = Command::new(&exe);
    cmd.arg("_internal")
        .arg("capture-session")
        .arg("--repo")
        .arg(repo)
        .arg("--branch")
        .arg(branch)
        .arg("--initial-count")
        .arg(initial_count.to_string())
        .arg("--timeout")
        .arg(timeout_secs.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // SAFETY: setsid() creates a new session, detaching from the terminal.
    // This is safe to call in a pre_exec hook.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    cmd.spawn()
        .context("Failed to spawn session capture subprocess")?;

    Ok(())
}

/// Non-Unix fallback - just spawn without detaching.
#[cfg(not(unix))]
pub fn spawn_session_capture(repo: &str, branch: &str, timeout_secs: u32) -> Result<()> {
    let initial_count = count_session_dirs()?;
    let exe = std::env::current_exe().context("Failed to get current executable path")?;

    debug!(
        repo,
        branch,
        initial_count,
        timeout_secs,
        "Spawning session capture subprocess"
    );

    Command::new(&exe)
        .arg("_internal")
        .arg("capture-session")
        .arg("--repo")
        .arg(repo)
        .arg("--branch")
        .arg(branch)
        .arg("--initial-count")
        .arg(initial_count.to_string())
        .arg("--timeout")
        .arg(timeout_secs.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to spawn session capture subprocess")?;

    Ok(())
}

/// Run the capture loop to detect and store a new Claude session ID.
///
/// This is called by the `_internal capture-session` command.
///
/// Algorithm:
/// 1. Wait 5 seconds for Claude to start
/// 2. Poll `~/.claude/session-env/` every 2 seconds
/// 3. When count > initial_count, find the newest directory
/// 4. Validate the directory name is a UUID
/// 5. Store the session ID
/// 6. Exit after success or timeout
pub fn run_capture_loop(
    repo: &str,
    branch: &str,
    initial_count: usize,
    timeout_secs: u32,
) -> Result<()> {
    let session_env_dir = get_claude_session_env_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

    // Initial delay for Claude to start
    info!(
        repo,
        branch,
        initial_count,
        timeout_secs,
        "Starting session capture, waiting 5s for Claude to start"
    );
    thread::sleep(Duration::from_secs(5));

    let poll_interval = Duration::from_secs(2);
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_secs as u64);

    while start.elapsed() < timeout {
        let current_count = count_session_dirs().unwrap_or(0);
        debug!(current_count, initial_count, "Polling session-env");

        if current_count > initial_count {
            // Find the newest session directory
            if let Some(session_id) = find_latest_session_id(&session_env_dir)? {
                if is_valid_uuid(&session_id) {
                    store_session(repo, branch, &session_id)?;
                    info!(repo, branch, session_id, "Session ID captured successfully");
                    return Ok(());
                } else {
                    warn!(session_id, "Found directory is not a valid UUID, continuing to poll");
                }
            }
        }

        thread::sleep(poll_interval);
    }

    warn!(repo, branch, "Session capture timed out");
    Ok(())
}

/// Find the most recent session directory in `~/.claude/session-env/`.
fn find_latest_session_id(session_env_dir: &PathBuf) -> Result<Option<String>> {
    if !session_env_dir.exists() {
        return Ok(None);
    }

    let mut latest: Option<(std::time::SystemTime, String)> = None;

    for entry in fs::read_dir(session_env_dir)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();

        // Get modification time
        if let Ok(metadata) = path.metadata() {
            if let Ok(modified) = metadata.modified() {
                match &latest {
                    Some((latest_time, _)) if modified > *latest_time => {
                        latest = Some((modified, name));
                    }
                    None => {
                        latest = Some((modified, name));
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(latest.map(|(_, name)| name))
}

/// Auto-detect and capture the most recent Claude session ID.
///
/// Used by `workmux session capture` when no session ID is provided.
pub fn capture_latest_session(repo: &str, branch: &str) -> Result<Option<String>> {
    let session_env_dir = get_claude_session_env_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

    if let Some(session_id) = find_latest_session_id(&session_env_dir)? {
        if is_valid_uuid(&session_id) {
            store_session(repo, branch, &session_id)?;
            return Ok(Some(session_id));
        } else {
            warn!(session_id, "Latest session directory is not a valid UUID");
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Mutex to ensure tests that modify env vars run serially
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_valid_uuid() {
        // Valid UUIDs
        assert!(is_valid_uuid("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_valid_uuid("00000000-0000-0000-0000-000000000000"));
        assert!(is_valid_uuid("ffffffff-ffff-ffff-ffff-ffffffffffff"));
        assert!(is_valid_uuid("FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF"));
        assert!(is_valid_uuid("abcdef12-3456-7890-abcd-ef1234567890"));

        // Invalid UUIDs
        assert!(!is_valid_uuid(""));
        assert!(!is_valid_uuid("not-a-uuid"));
        assert!(!is_valid_uuid("550e8400-e29b-41d4-a716")); // Too short
        assert!(!is_valid_uuid("550e8400-e29b-41d4-a716-4466554400001")); // Too long
        assert!(!is_valid_uuid("550e8400e29b41d4a716446655440000")); // No dashes
        assert!(!is_valid_uuid("550e8400-e29b-41d4-a716-44665544000g")); // Invalid char
        assert!(!is_valid_uuid("550e840-e29b-41d4-a716-446655440000")); // Wrong segment length
    }

    #[test]
    fn test_session_storage_roundtrip() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        // SAFETY: Protected by mutex, only one test modifies env at a time
        unsafe {
            std::env::set_var("XDG_STATE_HOME", temp_dir.path());
        }

        let repo = "test-repo-roundtrip";
        let branch = "test-branch";
        let session_id = "550e8400-e29b-41d4-a716-446655440000";

        // Initially no session
        let result = get_session(repo, branch).unwrap();
        assert!(result.is_none());

        // Store session
        store_session(repo, branch, session_id).unwrap();

        // Retrieve session
        let result = get_session(repo, branch).unwrap();
        assert_eq!(result, Some(session_id.to_string()));

        // List sessions
        let sessions = list_sessions(repo).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].branch, branch);
        assert_eq!(sessions[0].session_id, Some(session_id.to_string()));

        // Remove session
        remove_session(repo, branch).unwrap();

        // Should be gone
        let result = get_session(repo, branch).unwrap();
        assert!(result.is_none());

        // Clean up env var
        // SAFETY: Protected by mutex
        unsafe {
            std::env::remove_var("XDG_STATE_HOME");
        }
    }

    #[test]
    fn test_list_sessions_multiple_branches() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        // SAFETY: Protected by mutex, only one test modifies env at a time
        unsafe {
            std::env::set_var("XDG_STATE_HOME", temp_dir.path());
        }

        let repo = "multi-branch-repo";

        store_session(repo, "branch-a", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
        store_session(repo, "branch-b", "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap();
        store_session(repo, "branch-c", "cccccccc-cccc-cccc-cccc-cccccccccccc").unwrap();

        let sessions = list_sessions(repo).unwrap();
        assert_eq!(sessions.len(), 3);

        // Should be sorted alphabetically
        assert_eq!(sessions[0].branch, "branch-a");
        assert_eq!(sessions[1].branch, "branch-b");
        assert_eq!(sessions[2].branch, "branch-c");

        // SAFETY: Protected by mutex
        unsafe {
            std::env::remove_var("XDG_STATE_HOME");
        }
    }

    #[test]
    fn test_list_sessions_nonexistent_repo() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        // SAFETY: Protected by mutex, only one test modifies env at a time
        unsafe {
            std::env::set_var("XDG_STATE_HOME", temp_dir.path());
        }

        let sessions = list_sessions("nonexistent-repo").unwrap();
        assert!(sessions.is_empty());

        // SAFETY: Protected by mutex
        unsafe {
            std::env::remove_var("XDG_STATE_HOME");
        }
    }

    #[test]
    fn test_remove_nonexistent_session() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        // SAFETY: Protected by mutex, only one test modifies env at a time
        unsafe {
            std::env::set_var("XDG_STATE_HOME", temp_dir.path());
        }

        // Should not error
        remove_session("nonexistent-repo", "nonexistent-branch").unwrap();

        // SAFETY: Protected by mutex
        unsafe {
            std::env::remove_var("XDG_STATE_HOME");
        }
    }

    #[test]
    fn test_store_and_get_repo_path() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        // SAFETY: Protected by mutex
        unsafe {
            std::env::set_var("XDG_STATE_HOME", temp_dir.path());
        }

        let repo = "test-repo-path";
        // Use temp_dir itself as a path that exists on disk
        let repo_path = temp_dir.path().to_path_buf();

        // Initially no repo path
        let result = get_repo_path(repo).unwrap();
        assert!(result.is_none());

        // Store repo path
        store_repo_path(repo, &repo_path).unwrap();

        // Retrieve repo path
        let result = get_repo_path(repo).unwrap();
        assert!(result.is_some());
        // Canonicalized paths should match
        let expected = repo_path.canonicalize().unwrap();
        assert_eq!(result.unwrap(), expected);

        // SAFETY: Protected by mutex
        unsafe {
            std::env::remove_var("XDG_STATE_HOME");
        }
    }

    #[test]
    fn test_list_all_repos() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        // SAFETY: Protected by mutex
        unsafe {
            std::env::set_var("XDG_STATE_HOME", temp_dir.path());
        }

        // Create some repos with valid paths (use temp_dir subdirectories)
        let repo_a_path = temp_dir.path().join("repo_a_dir");
        let repo_b_path = temp_dir.path().join("repo_b_dir");
        fs::create_dir(&repo_a_path).unwrap();
        fs::create_dir(&repo_b_path).unwrap();

        store_repo_path("repo-a", &repo_a_path).unwrap();
        store_repo_path("repo-b", &repo_b_path).unwrap();

        // Also store a session to make sure repo_path file doesn't interfere with list_sessions
        store_session("repo-a", "branch-1", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();

        let repos = list_all_repos().unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].0, "repo-a");
        assert_eq!(repos[1].0, "repo-b");

        // Verify list_sessions still works (repo_path file is not a dir, so it's skipped)
        let sessions = list_sessions("repo-a").unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].branch, "branch-1");

        // SAFETY: Protected by mutex
        unsafe {
            std::env::remove_var("XDG_STATE_HOME");
        }
    }

    #[test]
    fn test_list_all_repos_skips_nonexistent_paths() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        // SAFETY: Protected by mutex
        unsafe {
            std::env::set_var("XDG_STATE_HOME", temp_dir.path());
        }

        // Write a repo_path pointing to a nonexistent directory
        let sessions_dir = get_sessions_dir().unwrap();
        let repo_dir = sessions_dir.join("stale-repo");
        fs::create_dir_all(&repo_dir).unwrap();
        fs::write(repo_dir.join("repo_path"), "/nonexistent/path/to/repo").unwrap();

        let repos = list_all_repos().unwrap();
        assert!(repos.is_empty());

        // SAFETY: Protected by mutex
        unsafe {
            std::env::remove_var("XDG_STATE_HOME");
        }
    }

    #[test]
    fn test_remove_session_cleans_repo_path() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let temp_dir = TempDir::new().unwrap();
        // SAFETY: Protected by mutex
        unsafe {
            std::env::set_var("XDG_STATE_HOME", temp_dir.path());
        }

        let repo = "cleanup-repo";
        let repo_path = temp_dir.path().to_path_buf();

        store_repo_path(repo, &repo_path).unwrap();
        store_session(repo, "only-branch", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();

        // repo_path file should exist
        assert!(get_repo_path(repo).unwrap().is_some());

        // Remove the only branch session
        remove_session(repo, "only-branch").unwrap();

        // repo_path file should be cleaned up since no branch dirs remain
        let repo_path_file = get_sessions_dir().unwrap().join(repo).join("repo_path");
        assert!(!repo_path_file.exists());

        // SAFETY: Protected by mutex
        unsafe {
            std::env::remove_var("XDG_STATE_HOME");
        }
    }
}
