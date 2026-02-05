//! Claude Code trust management for ~/.claude.json

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Get the path to the Claude Code configuration file
fn get_config_path() -> Option<PathBuf> {
    home::home_dir().map(|h| h.join(".claude.json"))
}

/// Ensure ~/.claude.json exists and has a valid structure.
/// Creates the file if missing, fixes malformed JSON, ensures .projects key exists.
fn ensure_valid_config() -> Result<(PathBuf, Value)> {
    let config_path =
        get_config_path().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

    let config_value = if config_path.exists() {
        let contents = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;

        match serde_json::from_str::<Value>(&contents) {
            Ok(mut val) => {
                // Ensure .projects key exists
                if val.get("projects").is_none() {
                    val.as_object_mut()
                        .map(|obj| obj.insert("projects".to_string(), json!({})));
                }
                val
            }
            Err(e) => {
                warn!(
                    path = %config_path.display(),
                    error = %e,
                    "Malformed ~/.claude.json, recreating"
                );
                json!({"projects": {}})
            }
        }
    } else {
        json!({"projects": {}})
    };

    Ok((config_path, config_value))
}

/// Write config atomically (temp file + rename)
fn write_config_atomic(path: &Path, value: &Value) -> Result<()> {
    let content = serde_json::to_string_pretty(value)?;
    let tmp_path = path.with_extension("json.tmp");

    fs::write(&tmp_path, &content)
        .with_context(|| format!("Failed to write temp file {}", tmp_path.display()))?;

    fs::rename(&tmp_path, path)
        .with_context(|| format!("Failed to rename temp file to {}", path.display()))?;

    Ok(())
}

/// Create the trust entry for a new project
fn create_trust_entry() -> Value {
    json!({
        "allowedTools": [],
        "mcpContextUris": [],
        "mcpServers": {},
        "enabledMcpjsonServers": [],
        "disabledMcpjsonServers": [],
        "hasTrustDialogAccepted": true,
        "projectOnboardingSeenCount": 0,
        "hasClaudeMdExternalIncludesApproved": false,
        "hasClaudeMdExternalIncludesWarningShown": false,
        "hasCompletedProjectOnboarding": true
    })
}

/// Add a directory to Claude's trusted projects in ~/.claude.json.
/// Merges with existing entry to preserve user customizations.
pub fn trust_directory(path: &Path) -> Result<()> {
    let path_str = path.to_string_lossy().to_string();
    debug!(path = %path_str, "trusting directory in Claude");

    let (config_path, mut config_value) = ensure_valid_config()?;

    let projects = config_value
        .get_mut("projects")
        .and_then(|p| p.as_object_mut())
        .ok_or_else(|| anyhow::anyhow!("Invalid config structure"))?;

    // Merge with existing entry or create new
    let trust_entry = create_trust_entry();
    let entry = projects.entry(path_str.clone()).or_insert(json!({}));

    if let (Some(existing), Some(new)) = (entry.as_object_mut(), trust_entry.as_object()) {
        for (key, value) in new {
            existing.entry(key.clone()).or_insert(value.clone());
        }
    }

    write_config_atomic(&config_path, &config_value)?;
    debug!(path = %path_str, "directory trusted in Claude");

    Ok(())
}

/// Remove a directory from Claude's trusted projects.
/// No-op if the directory is not in the config.
pub fn untrust_directory(path: &Path) -> Result<()> {
    let path_str = path.to_string_lossy().to_string();

    let config_path = match get_config_path() {
        Some(p) if p.exists() => p,
        _ => return Ok(()), // No config file, nothing to untrust
    };

    let contents = fs::read_to_string(&config_path)?;
    let mut config_value: Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(_) => return Ok(()), // Malformed, nothing to do
    };

    let projects = match config_value
        .get_mut("projects")
        .and_then(|p| p.as_object_mut())
    {
        Some(p) => p,
        None => return Ok(()),
    };

    if projects.remove(&path_str).is_some() {
        write_config_atomic(&config_path, &config_value)?;
        debug!(path = %path_str, "directory untrusted from Claude");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_trust_entry_structure() {
        let entry = create_trust_entry();
        assert!(entry
            .get("hasTrustDialogAccepted")
            .unwrap()
            .as_bool()
            .unwrap());
        assert!(entry
            .get("hasCompletedProjectOnboarding")
            .unwrap()
            .as_bool()
            .unwrap());
        assert!(entry
            .get("allowedTools")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
    }
}
