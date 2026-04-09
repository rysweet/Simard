use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{SimardError, SimardResult};
use crate::handoff::{FileBackedHandoffStore, RuntimeHandoffSnapshot, RuntimeHandoffStore};

use super::types::{COMPATIBILITY_HANDOFF_FILE_NAME, ScopedHandoffMode, SelectedHandoffArtifact};

pub fn compatibility_handoff_path(state_root: &Path) -> PathBuf {
    state_root.join(COMPATIBILITY_HANDOFF_FILE_NAME)
}

pub fn scoped_handoff_path(state_root: &Path, mode: ScopedHandoffMode) -> PathBuf {
    state_root.join(mode.scoped_file_name())
}

pub fn persist_handoff_artifacts(
    state_root: &Path,
    mode: ScopedHandoffMode,
    snapshot: &RuntimeHandoffSnapshot,
) -> SimardResult<()> {
    FileBackedHandoffStore::try_new(compatibility_handoff_path(state_root))?
        .save(snapshot.clone())?;
    FileBackedHandoffStore::try_new(scoped_handoff_path(state_root, mode))?
        .save(snapshot.clone())?;
    Ok(())
}

pub fn select_handoff_artifact_for_read(
    state_root: &Path,
    mode: ScopedHandoffMode,
    mode_label: &str,
) -> SimardResult<SelectedHandoffArtifact> {
    if let Some(path) = validate_optional_regular_file(
        state_root,
        &scoped_handoff_path(state_root, mode),
        mode.scoped_file_name(),
        mode_label,
    )? {
        return Ok(SelectedHandoffArtifact {
            path,
            file_name: mode.scoped_file_name(),
        });
    }

    Ok(SelectedHandoffArtifact {
        path: require_regular_file(
            state_root,
            &compatibility_handoff_path(state_root),
            COMPATIBILITY_HANDOFF_FILE_NAME,
            mode_label,
        )?,
        file_name: COMPATIBILITY_HANDOFF_FILE_NAME,
    })
}

pub fn select_optional_handoff_artifact(
    state_root: &Path,
    mode: ScopedHandoffMode,
    mode_label: &str,
) -> SimardResult<Option<SelectedHandoffArtifact>> {
    if let Some(path) = validate_optional_regular_file(
        state_root,
        &scoped_handoff_path(state_root, mode),
        mode.scoped_file_name(),
        mode_label,
    )? {
        return Ok(Some(SelectedHandoffArtifact {
            path,
            file_name: mode.scoped_file_name(),
        }));
    }

    if let Some(path) = validate_optional_regular_file(
        state_root,
        &compatibility_handoff_path(state_root),
        COMPATIBILITY_HANDOFF_FILE_NAME,
        mode_label,
    )? {
        return Ok(Some(SelectedHandoffArtifact {
            path,
            file_name: COMPATIBILITY_HANDOFF_FILE_NAME,
        }));
    }

    Ok(None)
}

pub fn load_runtime_handoff_snapshot(
    artifact: &SelectedHandoffArtifact,
    consumer_label: &str,
) -> SimardResult<RuntimeHandoffSnapshot> {
    let store = FileBackedHandoffStore::try_new(&artifact.path).map_err(|error| {
        SimardError::InvalidHandoffSnapshot {
            field: artifact.file_name.to_string(),
            reason: format!(
                "{consumer_label} could not load {} cleanly: {error}",
                artifact.file_name
            ),
        }
    })?;

    store
        .latest()
        .map_err(|error| SimardError::InvalidHandoffSnapshot {
            field: artifact.file_name.to_string(),
            reason: format!(
                "{consumer_label} could not read {} cleanly: {error}",
                artifact.file_name
            ),
        })?
        .ok_or_else(|| SimardError::InvalidHandoffSnapshot {
            field: artifact.file_name.to_string(),
            reason: format!(
                "{consumer_label} requires {} to contain a persisted handoff snapshot",
                artifact.file_name
            ),
        })
}

fn validate_optional_regular_file(
    state_root: &Path,
    path: &Path,
    file_name: &str,
    mode_label: &str,
) -> SimardResult<Option<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(SimardError::InvalidStateRoot {
                    path: state_root.to_path_buf(),
                    reason: format!(
                        "{mode_label} requires {file_name} to exist as a regular file, not a symlink"
                    ),
                });
            }
            if metadata.is_file() {
                return Ok(Some(path.to_path_buf()));
            }
            Err(SimardError::InvalidStateRoot {
                path: state_root.to_path_buf(),
                reason: format!("{mode_label} requires {file_name} to exist as a regular file"),
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!(
                "{mode_label} requires {file_name} to exist as a regular file: {error}"
            ),
        }),
    }
}

fn require_regular_file(
    state_root: &Path,
    path: &Path,
    file_name: &str,
    mode_label: &str,
) -> SimardResult<PathBuf> {
    validate_optional_regular_file(state_root, path, file_name, mode_label)?.ok_or_else(|| {
        SimardError::InvalidStateRoot {
            path: state_root.to_path_buf(),
            reason: format!("{mode_label} requires {file_name} to exist as a regular file"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- path builders --

    #[test]
    fn compatibility_handoff_path_joins_correctly() {
        let root = Path::new("/state");
        let path = compatibility_handoff_path(root);
        assert_eq!(path, PathBuf::from("/state/latest_handoff.json"));
    }

    #[test]
    fn scoped_handoff_path_terminal() {
        let root = Path::new("/state");
        let path = scoped_handoff_path(root, ScopedHandoffMode::Terminal);
        assert_eq!(path, PathBuf::from("/state/latest_terminal_handoff.json"));
    }

    #[test]
    fn scoped_handoff_path_engineer() {
        let root = Path::new("/state");
        let path = scoped_handoff_path(root, ScopedHandoffMode::Engineer);
        assert_eq!(path, PathBuf::from("/state/latest_engineer_handoff.json"));
    }

    // -- validate_optional_regular_file --

    #[test]
    fn validate_optional_regular_file_returns_none_for_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no_such_file");
        let result =
            validate_optional_regular_file(dir.path(), &missing, "no_such_file", "test").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn validate_optional_regular_file_returns_some_for_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("data.json");
        fs::write(&file_path, "{}").unwrap();
        let result =
            validate_optional_regular_file(dir.path(), &file_path, "data.json", "test").unwrap();
        assert_eq!(result, Some(file_path));
    }

    #[test]
    fn validate_optional_regular_file_rejects_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();
        let result = validate_optional_regular_file(dir.path(), &sub, "subdir", "test");
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn validate_optional_regular_file_rejects_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.json");
        fs::write(&target, "{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let result = validate_optional_regular_file(dir.path(), &link, "link.json", "test");
        assert!(result.is_err());
    }

    // -- require_regular_file --

    #[test]
    fn require_regular_file_returns_path_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("f.json");
        fs::write(&file_path, "{}").unwrap();
        let result = require_regular_file(dir.path(), &file_path, "f.json", "test").unwrap();
        assert_eq!(result, file_path);
    }

    #[test]
    fn require_regular_file_errors_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.json");
        let result = require_regular_file(dir.path(), &missing, "missing.json", "test");
        assert!(result.is_err());
    }

    // -- select_optional_handoff_artifact --

    #[test]
    fn select_optional_returns_none_when_neither_exists() {
        let dir = tempfile::tempdir().unwrap();
        let result =
            select_optional_handoff_artifact(dir.path(), ScopedHandoffMode::Terminal, "test")
                .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn select_optional_returns_compatibility_when_only_compat_exists() {
        let dir = tempfile::tempdir().unwrap();
        let compat = dir.path().join(COMPATIBILITY_HANDOFF_FILE_NAME);
        fs::write(&compat, "{}").unwrap();
        let result =
            select_optional_handoff_artifact(dir.path(), ScopedHandoffMode::Terminal, "test")
                .unwrap();
        let artifact = result.unwrap();
        assert_eq!(artifact.file_name, COMPATIBILITY_HANDOFF_FILE_NAME);
    }

    #[test]
    fn select_optional_prefers_scoped_over_compatibility() {
        let dir = tempfile::tempdir().unwrap();
        let compat = dir.path().join(COMPATIBILITY_HANDOFF_FILE_NAME);
        let scoped = dir
            .path()
            .join(ScopedHandoffMode::Terminal.scoped_file_name());
        fs::write(&compat, "{}").unwrap();
        fs::write(&scoped, "{}").unwrap();
        let result =
            select_optional_handoff_artifact(dir.path(), ScopedHandoffMode::Terminal, "test")
                .unwrap();
        let artifact = result.unwrap();
        assert_eq!(
            artifact.file_name,
            ScopedHandoffMode::Terminal.scoped_file_name()
        );
    }

    // -- select_handoff_artifact_for_read --

    #[test]
    fn select_for_read_falls_back_to_compatibility() {
        let dir = tempfile::tempdir().unwrap();
        let compat = dir.path().join(COMPATIBILITY_HANDOFF_FILE_NAME);
        fs::write(&compat, "{}").unwrap();
        let result =
            select_handoff_artifact_for_read(dir.path(), ScopedHandoffMode::Engineer, "test")
                .unwrap();
        assert_eq!(result.file_name, COMPATIBILITY_HANDOFF_FILE_NAME);
    }

    #[test]
    fn select_for_read_prefers_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let compat = dir.path().join(COMPATIBILITY_HANDOFF_FILE_NAME);
        let scoped = dir
            .path()
            .join(ScopedHandoffMode::Engineer.scoped_file_name());
        fs::write(&compat, "{}").unwrap();
        fs::write(&scoped, "{}").unwrap();
        let result =
            select_handoff_artifact_for_read(dir.path(), ScopedHandoffMode::Engineer, "test")
                .unwrap();
        assert_eq!(
            result.file_name,
            ScopedHandoffMode::Engineer.scoped_file_name()
        );
    }

    #[test]
    fn select_for_read_errors_when_nothing_exists() {
        let dir = tempfile::tempdir().unwrap();
        let result =
            select_handoff_artifact_for_read(dir.path(), ScopedHandoffMode::Terminal, "test");
        assert!(result.is_err());
    }
}
