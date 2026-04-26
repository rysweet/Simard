use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{SimardError, SimardResult};

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempFileGuard {
    path: PathBuf,
    file: Option<File>,
    keep: bool,
}

impl TempFileGuard {
    fn new(store: &str, destination: &Path) -> SimardResult<Self> {
        let (path, file) = create_temp_file(store, destination)?;
        let guard = Self {
            path,
            file: Some(file),
            keep: false,
        };
        set_owner_only_permissions(store, guard.path())?;
        Ok(guard)
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn file_mut(&mut self) -> SimardResult<&mut File> {
        self.file
            .as_mut()
            .ok_or_else(|| SimardError::PersistentStoreIo {
                store: String::new(),
                action: "write".to_string(),
                path: self.path.clone(),
                reason: "temporary persistence file was already closed".to_string(),
            })
    }

    fn close(&mut self) {
        if let Some(file) = self.file.take() {
            drop(file);
        }
    }

    fn persist(mut self, store: &str, destination: &Path) -> SimardResult<()> {
        self.close();
        fs::rename(self.path(), destination).map_err(|error| SimardError::PersistentStoreIo {
            store: store.to_string(),
            action: "rename".to_string(),
            path: destination.to_path_buf(),
            reason: error.to_string(),
        })?;
        self.keep = true;
        set_owner_only_permissions(store, destination)
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        self.close();
        let _ = fs::remove_file(&self.path);
    }
}

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

    let payload = serde_json::to_vec(value).map_err(|error| SimardError::PersistentStoreIo {
        store: store.to_string(),
        action: "serialize".to_string(),
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    let mut temp_file = TempFileGuard::new(store, path)?;
    temp_file
        .file_mut()?
        .write_all(&payload)
        .map_err(|error| SimardError::PersistentStoreIo {
            store: store.to_string(),
            action: "write-temp".to_string(),
            path: temp_file.path().to_path_buf(),
            reason: error.to_string(),
        })?;
    temp_file
        .file_mut()?
        .sync_all()
        .map_err(|error| SimardError::PersistentStoreIo {
            store: store.to_string(),
            action: "sync-temp".to_string(),
            path: temp_file.path().to_path_buf(),
            reason: error.to_string(),
        })?;
    temp_file.persist(store, path)
}

fn create_temp_file(store: &str, path: &Path) -> SimardResult<(PathBuf, File)> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "simard-store".to_string());

    for attempt in 0..32 {
        let temp_path = unique_temp_path(parent, &file_name, attempt);
        match open_exclusive_temp_file(&temp_path) {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(SimardError::PersistentStoreIo {
                    store: store.to_string(),
                    action: "create-temp".to_string(),
                    path: temp_path,
                    reason: error.to_string(),
                });
            }
        }
    }

    Err(SimardError::PersistentStoreIo {
        store: store.to_string(),
        action: "create-temp".to_string(),
        path: parent.join(file_name),
        reason: "unable to allocate a unique temporary file".to_string(),
    })
}

fn unique_temp_path(parent: &Path, file_name: &str, attempt: u32) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{file_name}.tmp.{}.{}.{}.{}",
        std::process::id(),
        timestamp,
        sequence,
        attempt
    ))
}

fn open_exclusive_temp_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    options.open(path)
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

#[cfg(test)]
mod tests;
