use std::fs;
use std::path::{Path, PathBuf};

use super::state_root::EngineerReadArtifacts;

pub(super) fn validate_meeting_read_state_root(state_root: &Path) -> crate::SimardResult<()> {
    validate_existing_read_state_root_root("meeting read", state_root)?;
    require_existing_read_file_for_mode(
        "meeting read",
        state_root,
        &state_root.join("memory_records.json"),
    )?;
    Ok(())
}

pub(super) fn validate_engineer_read_state_root(state_root: &Path) -> crate::SimardResult<()> {
    validated_engineer_read_artifacts(state_root)?;
    Ok(())
}

pub(super) fn validate_terminal_read_state_root(state_root: &Path) -> crate::SimardResult<()> {
    validated_terminal_read_artifacts(state_root)?;
    Ok(())
}

pub(super) fn validate_improvement_curation_read_state_root(
    state_root: &Path,
) -> crate::SimardResult<()> {
    validate_existing_read_state_root_root("improvement-curation read", state_root)?;

    require_existing_read_directory_for_mode(
        "improvement-curation read",
        state_root,
        &crate::review_artifacts_dir(state_root),
        "review-artifacts/",
    )?;
    require_existing_read_file_for_mode(
        "improvement-curation read",
        state_root,
        &state_root.join("memory_records.json"),
    )?;
    require_existing_read_file_for_mode(
        "improvement-curation read",
        state_root,
        &state_root.join("goal_records.json"),
    )?;
    Ok(())
}

pub(crate) fn validate_existing_read_state_root_root(
    mode_label: &str,
    state_root: &Path,
) -> crate::SimardResult<()> {
    let root_metadata =
        fs::symlink_metadata(state_root).map_err(|error| crate::SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!("{mode_label} requires an existing state root directory: {error}"),
        })?;
    if root_metadata.file_type().is_symlink() {
        return Err(crate::SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!("{mode_label} requires state-root to be a directory, not a symlink"),
        });
    }
    if root_metadata.is_dir() {
        return Ok(());
    }

    Err(crate::SimardError::InvalidStateRoot {
        path: state_root.to_path_buf(),
        reason: format!("{mode_label} requires state-root to resolve to a directory"),
    })
}

fn require_existing_read_directory_for_mode(
    mode_label: &str,
    state_root: &Path,
    path: &Path,
    label: &str,
) -> crate::SimardResult<()> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| crate::SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!("{mode_label} requires {label} to exist as a directory: {error}"),
        })?;
    if metadata.file_type().is_symlink() {
        return Err(crate::SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!("{mode_label} requires {label} to exist as a directory, not a symlink"),
        });
    }
    if metadata.is_dir() {
        return Ok(());
    }

    Err(crate::SimardError::InvalidStateRoot {
        path: state_root.to_path_buf(),
        reason: format!("{mode_label} requires {label} to exist as a directory"),
    })
}

pub(crate) fn require_existing_read_file_for_mode(
    mode_label: &str,
    state_root: &Path,
    path: &Path,
) -> crate::SimardResult<PathBuf> {
    let file_name = artifact_name(path);
    let metadata =
        fs::symlink_metadata(path).map_err(|error| crate::SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!(
                "{mode_label} requires {file_name} to exist as a regular file: {error}"
            ),
        })?;
    if metadata.file_type().is_symlink() {
        return Err(crate::SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!(
                "{mode_label} requires {file_name} to exist as a regular file, not a symlink"
            ),
        });
    }
    if metadata.is_file() {
        return Ok(path.to_path_buf());
    }

    Err(crate::SimardError::InvalidStateRoot {
        path: state_root.to_path_buf(),
        reason: format!("{mode_label} requires {file_name} to exist as a regular file"),
    })
}

fn artifact_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file")
}

pub(crate) fn validated_terminal_read_artifacts(
    state_root: &Path,
) -> crate::SimardResult<EngineerReadArtifacts> {
    validate_existing_read_state_root_root("terminal read", state_root)?;
    let selected_handoff = crate::terminal_engineer_bridge::select_handoff_artifact_for_read(
        state_root,
        crate::terminal_engineer_bridge::ScopedHandoffMode::Terminal,
        "terminal read",
    )?;
    Ok(EngineerReadArtifacts {
        handoff_path: selected_handoff.path,
        handoff_file_name: selected_handoff.file_name.to_string(),
        memory_path: require_existing_read_file_for_mode(
            "terminal read",
            state_root,
            &state_root.join("memory_records.json"),
        )?,
        evidence_path: require_existing_read_file_for_mode(
            "terminal read",
            state_root,
            &state_root.join("evidence_records.json"),
        )?,
    })
}

pub(crate) fn validated_engineer_read_artifacts(
    state_root: &Path,
) -> crate::SimardResult<EngineerReadArtifacts> {
    validate_existing_read_state_root_root("engineer read", state_root)?;
    let selected_handoff = crate::terminal_engineer_bridge::select_handoff_artifact_for_read(
        state_root,
        crate::terminal_engineer_bridge::ScopedHandoffMode::Engineer,
        "engineer read",
    )?;
    Ok(EngineerReadArtifacts {
        handoff_path: selected_handoff.path,
        handoff_file_name: selected_handoff.file_name.to_string(),
        memory_path: require_existing_read_file_for_mode(
            "engineer read",
            state_root,
            &state_root.join("memory_records.json"),
        )?,
        evidence_path: require_existing_read_file_for_mode(
            "engineer read",
            state_root,
            &state_root.join("evidence_records.json"),
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn validate_existing_read_state_root_root_nonexistent() {
        let result = validate_existing_read_state_root_root(
            "test mode",
            Path::new("/nonexistent/path/xyz_123"),
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("test mode"));
    }

    #[test]
    fn validate_existing_read_state_root_root_with_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = validate_existing_read_state_root_root("test mode", dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn validate_existing_read_state_root_root_with_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("not_a_dir.txt");
        std::fs::write(&file_path, "content").unwrap();
        let result = validate_existing_read_state_root_root("test mode", &file_path);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("directory"));
    }

    #[test]
    fn require_existing_read_file_succeeds_for_regular_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("test.json");
        std::fs::write(&file_path, "{}").unwrap();
        let result = require_existing_read_file_for_mode("test mode", dir.path(), &file_path);
        assert!(result.is_ok());
    }

    #[test]
    fn require_existing_read_file_fails_for_nonexistent() {
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("missing.json");
        let result = require_existing_read_file_for_mode("test mode", dir.path(), &file_path);
        assert!(result.is_err());
    }

    #[test]
    fn require_existing_read_file_fails_for_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        let result = require_existing_read_file_for_mode("test mode", dir.path(), &subdir);
        assert!(result.is_err());
    }

    #[test]
    fn require_existing_read_directory_succeeds() {
        let dir = tempfile::TempDir::new().unwrap();
        let subdir = dir.path().join("artifacts");
        std::fs::create_dir(&subdir).unwrap();
        let result = require_existing_read_directory_for_mode(
            "test mode",
            dir.path(),
            &subdir,
            "artifacts/",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn require_existing_read_directory_fails_for_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("not_a_dir");
        std::fs::write(&file, "data").unwrap();
        let result =
            require_existing_read_directory_for_mode("test mode", dir.path(), &file, "not_a_dir/");
        assert!(result.is_err());
    }

    #[test]
    fn require_existing_read_directory_fails_for_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("nope");
        let result =
            require_existing_read_directory_for_mode("test mode", dir.path(), &missing, "nope/");
        assert!(result.is_err());
    }

    #[test]
    fn artifact_name_extracts_file_name() {
        assert_eq!(artifact_name(Path::new("/foo/bar/baz.json")), "baz.json");
    }

    #[test]
    fn artifact_name_returns_default_for_root() {
        assert_eq!(artifact_name(Path::new("/")), "file");
    }

    #[test]
    fn artifact_name_with_nested_path() {
        assert_eq!(artifact_name(Path::new("/a/b/c/d/file.json")), "file.json");
    }

    #[test]
    fn artifact_name_with_no_extension() {
        assert_eq!(artifact_name(Path::new("/a/b/Makefile")), "Makefile");
    }

    #[test]
    fn artifact_name_with_empty_path() {
        assert_eq!(artifact_name(Path::new("")), "file");
    }

    #[cfg(unix)]
    #[test]
    fn validate_existing_read_state_root_root_rejects_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let real_dir = dir.path().join("real");
        std::fs::create_dir(&real_dir).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real_dir, &link).unwrap();
        let result = validate_existing_read_state_root_root("test", &link);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn require_existing_read_file_rejects_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let real_file = dir.path().join("real.json");
        std::fs::write(&real_file, "{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&real_file, &link).unwrap();
        let result = require_existing_read_file_for_mode("test", dir.path(), &link);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn require_existing_read_directory_rejects_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let real_dir = dir.path().join("real_dir");
        std::fs::create_dir(&real_dir).unwrap();
        let link = dir.path().join("link_dir");
        std::os::unix::fs::symlink(&real_dir, &link).unwrap();
        let result =
            require_existing_read_directory_for_mode("test", dir.path(), &link, "link_dir/");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("symlink"));
    }
}
