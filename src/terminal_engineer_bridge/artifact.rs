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
