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

    fn file_mut(&mut self) -> &mut File {
        self.file
            .as_mut()
            .expect("temporary persistence file should stay open until rename")
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

    let payload =
        serde_json::to_vec_pretty(value).map_err(|error| SimardError::PersistentStoreIo {
            store: store.to_string(),
            action: "serialize".to_string(),
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    let mut temp_file = TempFileGuard::new(store, path)?;
    temp_file
        .file_mut()
        .write_all(&payload)
        .map_err(|error| SimardError::PersistentStoreIo {
            store: store.to_string(),
            action: "write-temp".to_string(),
            path: temp_file.path().to_path_buf(),
            reason: error.to_string(),
        })?;
    temp_file
        .file_mut()
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
mod tests {
    use super::{TempFileGuard, persist_json};
    use std::fs;
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(label: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("{label}-{unique}"));
            fs::create_dir_all(&path).expect("test directory should be created");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn persist_json_ignores_planted_legacy_temp_symlink() {
        let temp_dir = TestDir::new("simard-persistence");
        let victim_path = temp_dir.path().join("victim.txt");
        let store_path = temp_dir.path().join("memory_records.json");
        let legacy_temp_path = temp_dir.path().join("memory_records.json.tmp");
        fs::write(&victim_path, "leave-me-alone").expect("victim file should exist");
        symlink(&victim_path, &legacy_temp_path).expect("legacy temp symlink should exist");

        persist_json("memory", &store_path, &vec!["fresh"])
            .expect("persistence should succeed without following the planted symlink");

        let victim_contents =
            fs::read_to_string(&victim_path).expect("victim file should remain readable");
        let store_contents =
            fs::read_to_string(&store_path).expect("store file should be written directly");

        assert_eq!(victim_contents, "leave-me-alone");
        assert!(
            store_contents.contains("fresh"),
            "store payload should be written to the requested destination"
        );
    }

    #[test]
    fn temp_file_guard_removes_uncommitted_temp_file_on_drop() {
        let temp_dir = TestDir::new("simard-persistence-cleanup");
        let store_path = temp_dir.path().join("memory_records.json");
        let temp_path = {
            let mut temp_file =
                TempFileGuard::new("memory", &store_path).expect("temp file guard should open");
            temp_file
                .file_mut()
                .write_all(br#"["pending"]"#)
                .expect("temporary payload should be writable");
            let temp_path = temp_file.path().to_path_buf();
            assert!(
                temp_path.is_file(),
                "temporary persistence file should exist before the guard drops"
            );
            temp_path
        };

        assert!(
            !temp_path.exists(),
            "dropping an uncommitted temp file guard should remove the leaked temp file"
        );
        assert!(
            !store_path.exists(),
            "cleanup must not create the destination file before rename succeeds"
        );
    }
}
