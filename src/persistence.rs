use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{SimardError, SimardResult};

pub fn load_json_or_default<T>(store: &str, path: &Path) -> SimardResult<T>
where
    T: DeserializeOwned + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }

    let contents = fs::read(path).map_err(|error| SimardError::PersistentStoreIo {
        store: store.to_string(),
        action: "read".to_string(),
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    serde_json::from_slice(&contents).map_err(|error| SimardError::PersistentStoreIo {
        store: store.to_string(),
        action: "deserialize".to_string(),
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

pub fn persist_json<T>(store: &str, path: &Path, value: &T) -> SimardResult<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| SimardError::PersistentStoreIo {
            store: store.to_string(),
            action: "create-dir".to_string(),
            path: parent.to_path_buf(),
            reason: error.to_string(),
        })?;
    }

    let payload =
        serde_json::to_vec_pretty(value).map_err(|error| SimardError::PersistentStoreIo {
            store: store.to_string(),
            action: "serialize".to_string(),
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    let temp_path = temp_path(path);
    fs::write(&temp_path, payload).map_err(|error| SimardError::PersistentStoreIo {
        store: store.to_string(),
        action: "write".to_string(),
        path: temp_path.clone(),
        reason: error.to_string(),
    })?;
    set_owner_only_permissions(store, &temp_path)?;
    fs::rename(&temp_path, path).map_err(|error| SimardError::PersistentStoreIo {
        store: store.to_string(),
        action: "rename".to_string(),
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    set_owner_only_permissions(store, path)
}

fn temp_path(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(file_name)) => {
            parent.join(format!("{}.tmp", file_name.to_string_lossy()))
        }
        _ => PathBuf::from(format!("{}.tmp", path.to_string_lossy())),
    }
}

#[cfg(unix)]
fn set_owner_only_permissions(store: &str, path: &Path) -> SimardResult<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions).map_err(|error| SimardError::PersistentStoreIo {
        store: store.to_string(),
        action: "chmod".to_string(),
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_store: &str, _path: &Path) -> SimardResult<()> {
    Ok(())
}
