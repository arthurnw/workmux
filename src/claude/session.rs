//! Claude Code session management.
//!
//! Session IDs are discovered by scanning `~/.claude/projects/<encoded-path>/`
//! for `.jsonl` files whose stems are valid UUIDs.

use anyhow::{Context, Result};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

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

/// Build session info for a set of worktrees by querying Claude project dirs.
pub fn list_sessions_for_worktrees(worktrees: &[(PathBuf, String)]) -> Result<Vec<SessionInfo>> {
    let mut sessions = Vec::new();

    for (wt_path, branch) in worktrees {
        let session_id = find_latest_project_session(wt_path)?;
        sessions.push(SessionInfo {
            branch: branch.clone(),
            session_id,
        });
    }

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
    fs::write(&tmp_path, abs_path.to_string_lossy().as_bytes()).with_context(|| {
        format!(
            "Failed to write temp repo_path file: {}",
            tmp_path.display()
        )
    })?;

    fs::rename(&tmp_path, &path)
        .with_context(|| format!("Failed to rename repo_path file: {}", path.display()))?;

    debug!(repo, path = %abs_path.display(), "Stored repo path");
    Ok(())
}

/// Store the tmux session name for a repository.
///
/// Writes to: `~/.local/state/workmux/sessions/<repo>/tmux_session`
/// Uses atomic write (temp + rename) for crash safety.
pub fn store_tmux_session(repo: &str, session_name: &str) -> Result<()> {
    let path = get_sessions_dir()?.join(repo).join("tmux_session");

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create session directory: {}", parent.display()))?;
    }

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, session_name).with_context(|| {
        format!(
            "Failed to write temp tmux_session file: {}",
            tmp_path.display()
        )
    })?;

    fs::rename(&tmp_path, &path)
        .with_context(|| format!("Failed to rename tmux_session file: {}", path.display()))?;

    debug!(repo, session_name, "Stored tmux session name");
    Ok(())
}

/// Retrieve the stored tmux session name for a repository.
///
/// Returns None if no session name is stored.
pub fn get_tmux_session(repo: &str) -> Result<Option<String>> {
    let path = get_sessions_dir()?.join(repo).join("tmux_session");

    match fs::read_to_string(&path) {
        Ok(content) => {
            let session_name = content.trim().to_string();
            if session_name.is_empty() {
                Ok(None)
            } else {
                Ok(Some(session_name))
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => {
            Err(e).with_context(|| format!("Failed to read tmux_session file: {}", path.display()))
        }
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

    for entry in fs::read_dir(&sessions_dir).with_context(|| {
        format!(
            "Failed to read sessions directory: {}",
            sessions_dir.display()
        )
    })? {
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

/// Encode a filesystem path the way Claude Code encodes project directories.
///
/// Replaces `/`, `_`, and `.` with `-`, producing names like `-Users-anw-code-workmux`.
pub fn encode_path_for_claude_projects(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.chars()
        .map(|c| match c {
            '/' | '_' | '.' => '-',
            _ => c,
        })
        .collect()
}

/// Get the Claude projects directory path.
///
/// Returns: `~/.claude/projects`
fn get_claude_projects_dir() -> Option<PathBuf> {
    home::home_dir().map(|h| h.join(".claude").join("projects"))
}

/// Find the latest Claude session for a worktree by scanning project directory.
///
/// Looks in `~/.claude/projects/<encoded-path>/` for `.jsonl` files and
/// returns the UUID (filename stem) of the most recently modified one.
pub fn find_latest_project_session(worktree_path: &Path) -> Result<Option<String>> {
    let projects_dir = get_claude_projects_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

    find_latest_project_session_in(&projects_dir, worktree_path)
}

/// Testable inner function that accepts a custom projects base directory.
fn find_latest_project_session_in(
    projects_base: &Path,
    worktree_path: &Path,
) -> Result<Option<String>> {
    let encoded = encode_path_for_claude_projects(worktree_path);
    let project_dir = projects_base.join(&encoded);

    if !project_dir.exists() {
        return Ok(None);
    }

    let mut latest: Option<(std::time::SystemTime, String)> = None;

    for entry in fs::read_dir(&project_dir)? {
        let entry = entry?;
        let path = entry.path();

        // Only consider .jsonl files
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        if !is_valid_uuid(&stem) {
            continue;
        }

        if let Ok(metadata) = path.metadata()
            && let Ok(modified) = metadata.modified()
        {
            match &latest {
                Some((latest_time, _)) if modified > *latest_time => {
                    latest = Some((modified, stem));
                }
                None => {
                    latest = Some((modified, stem));
                }
                _ => {}
            }
        }
    }

    Ok(latest.map(|(_, id)| id))
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

        let repos = list_all_repos().unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].0, "repo-a");
        assert_eq!(repos[1].0, "repo-b");

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
    fn test_find_latest_project_session_picks_newest_jsonl() {
        let temp_dir = TempDir::new().unwrap();
        let project_dir = temp_dir.path().join("-test-path");
        fs::create_dir_all(&project_dir).unwrap();

        let old_id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let new_id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";

        let old_file = project_dir.join(format!("{}.jsonl", old_id));
        let new_file = project_dir.join(format!("{}.jsonl", new_id));

        fs::write(&old_file, "old session").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&new_file, "new session").unwrap();

        let result =
            find_latest_project_session_in(temp_dir.path(), Path::new("/test/path")).unwrap();
        assert_eq!(result, Some(new_id.to_string()));
    }

    #[test]
    fn test_find_latest_project_session_nonexistent_dir() {
        let temp_dir = TempDir::new().unwrap();
        let result =
            find_latest_project_session_in(temp_dir.path(), Path::new("/no/such/worktree"))
                .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_find_latest_project_session_no_jsonl_files() {
        let temp_dir = TempDir::new().unwrap();
        let project_dir = temp_dir.path().join("-test-path");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(project_dir.join("some_other_file.txt"), "not jsonl").unwrap();

        let result =
            find_latest_project_session_in(temp_dir.path(), Path::new("/test/path")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_find_latest_project_session_invalid_uuid_skipped() {
        let temp_dir = TempDir::new().unwrap();
        let project_dir = temp_dir.path().join("-test-path");
        fs::create_dir_all(&project_dir).unwrap();

        fs::write(project_dir.join("not-a-uuid.jsonl"), "data").unwrap();

        let result =
            find_latest_project_session_in(temp_dir.path(), Path::new("/test/path")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_encode_path_for_claude_projects() {
        assert_eq!(
            encode_path_for_claude_projects(Path::new("/Users/anw/code/workmux")),
            "-Users-anw-code-workmux"
        );
        assert_eq!(
            encode_path_for_claude_projects(Path::new(
                "/Users/anw/code/oss/workmux__worktrees/feature"
            )),
            "-Users-anw-code-oss-workmux--worktrees-feature"
        );
    }
}
