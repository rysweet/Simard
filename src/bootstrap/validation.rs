use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::error::{SimardError, SimardResult};

pub fn validate_state_root(path: impl AsRef<Path>) -> SimardResult<PathBuf> {
    let raw_path = path.as_ref();
    if raw_path.as_os_str().is_empty() {
        return Err(SimardError::InvalidStateRoot {
            path: raw_path.to_path_buf(),
            reason: "state root must not be empty".to_string(),
        });
    }

    if raw_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(SimardError::InvalidStateRoot {
            path: raw_path.to_path_buf(),
            reason: "state root must not contain '..' path segments".to_string(),
        });
    }

    let absolute_path = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| SimardError::InvalidStateRoot {
                path: raw_path.to_path_buf(),
                reason: format!("current working directory could not be resolved: {error}"),
            })?
            .join(raw_path)
    };

    let (existing_root, missing_segments) = split_existing_prefix(&absolute_path)?;
    let metadata =
        fs::symlink_metadata(&existing_root).map_err(|error| SimardError::InvalidStateRoot {
            path: raw_path.to_path_buf(),
            reason: format!(
                "existing state root ancestor '{}' could not be inspected: {error}",
                existing_root.display()
            ),
        })?;

    if metadata.file_type().is_symlink() {
        return Err(SimardError::InvalidStateRoot {
            path: raw_path.to_path_buf(),
            reason: "state root must not be a symlink".to_string(),
        });
    }
    if !metadata.is_dir() {
        return Err(SimardError::InvalidStateRoot {
            path: raw_path.to_path_buf(),
            reason: "state root must resolve to a directory".to_string(),
        });
    }

    let mut canonical =
        fs::canonicalize(&existing_root).map_err(|error| SimardError::InvalidStateRoot {
            path: raw_path.to_path_buf(),
            reason: format!(
                "state root ancestor '{}' could not be canonicalized: {error}",
                existing_root.display()
            ),
        })?;
    for segment in missing_segments {
        canonical.push(segment);
    }

    Ok(canonical)
}

fn split_existing_prefix(path: &Path) -> SimardResult<(PathBuf, Vec<OsString>)> {
    let mut existing = path.to_path_buf();
    let mut missing_segments = Vec::new();

    loop {
        match fs::symlink_metadata(&existing) {
            Ok(_) => {
                missing_segments.reverse();
                return Ok((existing, missing_segments));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let segment =
                    existing
                        .file_name()
                        .ok_or_else(|| SimardError::InvalidStateRoot {
                            path: path.to_path_buf(),
                            reason: "state root must stay under an existing directory".to_string(),
                        })?;
                missing_segments.push(segment.to_os_string());
                existing = existing
                    .parent()
                    .ok_or_else(|| SimardError::InvalidStateRoot {
                        path: path.to_path_buf(),
                        reason: "state root must stay under an existing directory".to_string(),
                    })?
                    .to_path_buf();
            }
            Err(error) => {
                return Err(SimardError::InvalidStateRoot {
                    path: path.to_path_buf(),
                    reason: format!(
                        "state root '{}' could not be inspected: {error}",
                        existing.display()
                    ),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::validate_state_root;
    use crate::bootstrap::test_support::TestDir;
    use crate::error::SimardError;

    #[test]
    fn validate_state_root_rejects_parent_directory_segments() {
        let error = validate_state_root(PathBuf::from("../outside-state"))
            .expect_err("state root traversal should fail");

        assert_eq!(
            error,
            SimardError::InvalidStateRoot {
                path: PathBuf::from("../outside-state"),
                reason: "state root must not contain '..' path segments".to_string(),
            }
        );
    }

    #[test]
    fn validate_state_root_canonicalizes_safe_existing_directories() {
        let temp_dir = TestDir::new("simard-state-root");
        let nested = temp_dir.path().join("nested");
        fs::create_dir_all(&nested).expect("nested directory should exist");

        let resolved =
            validate_state_root(nested.clone()).expect("existing state root should pass");
        let expected = fs::canonicalize(&nested).expect("existing state root should canonicalize");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn validate_state_root_preserves_missing_segment_order() {
        let temp_dir = TestDir::new("simard-state-root-order");
        let requested = temp_dir.path().join("level1").join("level2").join("level3");

        let resolved =
            validate_state_root(requested).expect("missing state root path should resolve safely");
        let expected = fs::canonicalize(temp_dir.path())
            .expect("existing ancestor should canonicalize")
            .join("level1")
            .join("level2")
            .join("level3");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn validate_state_root_rejects_existing_files() {
        let temp_dir = TestDir::new("simard-state-root-file");
        let file_path = temp_dir.path().join("state-root.txt");
        fs::write(&file_path, "not a directory").expect("file should be written");

        let error =
            validate_state_root(file_path.clone()).expect_err("state root file should fail");

        assert_eq!(
            error,
            SimardError::InvalidStateRoot {
                path: file_path,
                reason: "state root must resolve to a directory".to_string(),
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn validate_state_root_rejects_symlink_roots() {
        use std::os::unix::fs::symlink;

        let temp_dir = TestDir::new("simard-state-root-symlink");
        let real_dir = temp_dir.path().join("real");
        let link_dir = temp_dir.path().join("link");
        fs::create_dir_all(&real_dir).expect("real directory should exist");
        symlink(&real_dir, &link_dir).expect("symlink should be created");

        let error =
            validate_state_root(link_dir.clone()).expect_err("symlink state root should fail");

        assert_eq!(
            error,
            SimardError::InvalidStateRoot {
                path: link_dir,
                reason: "state root must not be a symlink".to_string(),
            }
        );
    }

    #[test]
    fn validate_state_root_rejects_empty_path() {
        let err = validate_state_root("").unwrap_err();
        assert_eq!(
            err,
            SimardError::InvalidStateRoot {
                path: PathBuf::from(""),
                reason: "state root must not be empty".to_string(),
            }
        );
    }

    #[test]
    fn validate_state_root_rejects_double_parent_traversal() {
        let err = validate_state_root("a/../../outside").unwrap_err();
        match err {
            SimardError::InvalidStateRoot { reason, .. } => {
                assert!(reason.contains(".."));
            }
            other => panic!("expected InvalidStateRoot, got {other:?}"),
        }
    }
}
