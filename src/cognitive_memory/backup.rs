//! NativeCognitiveMemory backup, restore, and DB-recovery helpers.

use std::fs;
use std::os::unix::io::AsRawFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use crate::error::{SimardError, SimardResult};

use super::NativeCognitiveMemory;

impl NativeCognitiveMemory {
    /// Return the WAL file paths that LadybugDB may use for a given DB path.
    fn wal_paths(db_path: &Path) -> [PathBuf; 2] {
        let wal1 = db_path.with_extension("ladybug.wal");
        let wal2 = {
            let mut p = db_path.as_os_str().to_owned();
            p.push(".wal");
            PathBuf::from(p)
        };
        [wal1, wal2]
    }

    /// Remove WAL files that are empty or unreadable — these cause LadybugDB
    /// to hit an UNREACHABLE_CODE assertion on open.
    fn preemptive_wal_cleanup(db_path: &Path) {
        for wal in Self::wal_paths(db_path) {
            if !wal.exists() {
                continue;
            }
            let should_remove = match std::fs::metadata(&wal) {
                Ok(meta) => meta.len() == 0,
                Err(_) => true, // unreadable WAL — remove it
            };
            if should_remove {
                eprintln!(
                    "[simard] removing corrupt/empty WAL file: {}",
                    wal.display()
                );
                let _ = std::fs::remove_file(&wal);
            }
        }
    }

    /// Run a basic health-check query to verify the DB is actually usable.
    ///
    /// Tries a table-specific query first; if the table doesn't exist yet
    /// (fresh DB before schema init), runs a lightweight catalog query
    /// instead to confirm the engine is responsive.
    fn verify_db_health(db: &lbug::Database) -> SimardResult<()> {
        let conn = lbug::Connection::new(db).map_err(|e| SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: format!("Health check connection failed: {e}"),
        })?;
        match conn.query("MATCH (n:Fact) RETURN count(n)") {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = format!("{e}");
                if msg.contains("does not exist") {
                    // Table missing — DB is valid but schema not yet applied.
                    // Verify engine works with a simple query.
                    conn.query("RETURN 1")
                        .map_err(|e2| SimardError::RuntimeInitFailed {
                            component: "cognitive-memory".into(),
                            reason: format!("Health check basic query failed: {e2}"),
                        })?;
                    Ok(())
                } else {
                    Err(SimardError::RuntimeInitFailed {
                        component: "cognitive-memory".into(),
                        reason: format!("Health check query failed: {e}"),
                    })
                }
            }
        }
    }

    /// Find the most recent verified backup in `{state_root}/backups/`.
    fn find_latest_backup(db_path: &Path) -> Option<PathBuf> {
        let state_root = db_path.parent()?;
        let backup_dir = state_root.join("backups");
        if !backup_dir.is_dir() {
            return None;
        }
        let prefix = "cognitive_memory.ladybug.";
        let mut candidates: Vec<(u64, PathBuf)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&backup_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if let Some(epoch_str) = name_str.strip_prefix(prefix)
                    && let Ok(epoch) = epoch_str.parse::<u64>()
                {
                    candidates.push((epoch, entry.path()));
                }
            }
        }
        candidates.sort_by_key(|x| std::cmp::Reverse(x.0));
        candidates.into_iter().next().map(|(_, p)| p)
    }

    /// Try to restore from the most recent verified backup.
    fn try_restore_from_backup(db_path: &Path) -> SimardResult<lbug::Database> {
        let backup_path =
            Self::find_latest_backup(db_path).ok_or_else(|| SimardError::RuntimeInitFailed {
                component: "cognitive-memory".into(),
                reason: "No backups available for restore".into(),
            })?;

        let epoch_str = backup_path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("cognitive_memory.ladybug."))
            .unwrap_or("unknown");
        eprintln!(
            "[simard] restoring from backup at {} (created epoch {epoch_str})",
            backup_path.display()
        );

        std::fs::copy(&backup_path, db_path).map_err(|e| SimardError::PersistentStoreIo {
            store: "cognitive-memory".into(),
            action: "restore-backup-copy".into(),
            path: db_path.to_path_buf(),
            reason: e.to_string(),
        })?;

        // Clean WAL files before opening restored copy.
        Self::preemptive_wal_cleanup(db_path);

        let db = Self::with_open_lock(db_path, || {
            lbug::Database::new(db_path, lbug::SystemConfig::default()).map_err(|e| {
                SimardError::RuntimeInitFailed {
                    component: "cognitive-memory".into(),
                    reason: format!("Failed to open restored backup: {e}"),
                }
            })
        })?;

        Self::verify_db_health(&db)?;
        eprintln!("[simard] successfully restored from backup");
        Ok(db)
    }

    /// Open LadybugDB with WAL corruption recovery and backup restore.
    ///
    /// Strategy:
    /// 1. Preemptively remove empty/unreadable WAL files
    /// 2. Try opening with `catch_unwind` to survive WAL corruption panics
    /// 3. On success, verify the DB is usable with a health-check query
    /// 4. On panic: back up corrupt DB, remove WAL, retry
    /// 5. If retry also fails: try restoring from most recent verified backup
    /// 6. If no backup: create a fresh empty DB (data loss — logged clearly)
    #[cfg(unix)]
    pub(super) fn open_db_with_recovery(db_path: &Path) -> SimardResult<lbug::Database> {
        // Step 1: preemptive WAL cleanup.
        Self::preemptive_wal_cleanup(db_path);

        let try_open = |p: &Path| -> SimardResult<lbug::Database> {
            Self::with_open_lock(p, || {
                lbug::Database::new(p, lbug::SystemConfig::default()).map_err(|e| {
                    SimardError::RuntimeInitFailed {
                        component: "cognitive-memory".into(),
                        reason: format!("Failed to open LadybugDB at {}: {e}", p.display()),
                    }
                })
            })
        };

        let try_open_and_verify = |p: &Path| -> SimardResult<lbug::Database> {
            let db = try_open(p)?;
            Self::verify_db_health(&db)?;
            Ok(db)
        };

        // Step 2: first attempt — catch panics from WAL corruption assertions.
        let db_path_owned = db_path.to_path_buf();
        let first = catch_unwind(AssertUnwindSafe(|| try_open_and_verify(&db_path_owned)));
        match first {
            Ok(Ok(db)) => return Ok(db),
            Ok(Err(e)) => {
                eprintln!("[simard] LadybugDB opened but failed health check: {e}");
            }
            Err(panic_info) => {
                let msg = panic_info
                    .downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| panic_info.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                eprintln!("[simard] LadybugDB panicked on open (likely WAL corruption): {msg}");
            }
        }

        // Step 3: back up the corrupt DB and remove WAL files.
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let corrupt_backup = db_path.with_extension(format!("corrupt-{ts}"));
        if db_path.exists() {
            if let Err(e) = std::fs::rename(db_path, &corrupt_backup) {
                eprintln!("[simard] failed to back up corrupt DB: {e}");
            } else {
                eprintln!(
                    "[simard] backed up corrupt DB to {}",
                    corrupt_backup.display()
                );
            }
        }

        for wal in Self::wal_paths(db_path) {
            if wal.exists() {
                let _ = std::fs::remove_file(&wal);
            }
        }

        // Step 4: retry open (DB was renamed away, so this creates a fresh one
        // OR we try restore first).
        eprintln!("[simard] retrying LadybugDB open after recovery...");
        let second = catch_unwind(AssertUnwindSafe(|| try_open_and_verify(&db_path_owned)));
        match second {
            Ok(Ok(db)) => {
                eprintln!(
                    "[simard] recovery created new empty DB — data loss occurred. \
                     Old data backed up to {}",
                    corrupt_backup.display()
                );
                return Ok(db);
            }
            Ok(Err(e)) => {
                eprintln!("[simard] retry also failed: {e}");
            }
            Err(_) => {
                eprintln!("[simard] retry also panicked");
            }
        }

        // Step 5: try restoring from backup.
        if let Ok(db) = Self::try_restore_from_backup(db_path) {
            return Ok(db);
        }

        // Step 6: last resort — ensure clean slate and create fresh DB.
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        Self::preemptive_wal_cleanup(db_path);
        eprintln!(
            "[simard] all recovery options exhausted — creating fresh empty DB (data loss). \
             Corrupt data backed up to {}",
            corrupt_backup.display()
        );
        try_open(db_path)
    }

    /// Create a verified backup of the DB file. Returns the backup path on
    /// success. Used by the OODA daemon for periodic backups.
    #[cfg(unix)]
    pub fn create_verified_backup(state_root: &Path) -> SimardResult<PathBuf> {
        let db_path = state_root.join("cognitive_memory.ladybug");
        if !db_path.exists() {
            return Err(SimardError::RuntimeInitFailed {
                component: "cognitive-memory".into(),
                reason: "DB file does not exist, nothing to back up".into(),
            });
        }

        let backup_dir = state_root.join("backups");
        std::fs::create_dir_all(&backup_dir).map_err(|e| SimardError::PersistentStoreIo {
            store: "cognitive-memory".into(),
            action: "create_backup_dir".into(),
            path: backup_dir.clone(),
            reason: e.to_string(),
        })?;

        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let backup_path = backup_dir.join(format!("cognitive_memory.ladybug.{ts}"));

        std::fs::copy(&db_path, &backup_path).map_err(|e| SimardError::PersistentStoreIo {
            store: "cognitive-memory".into(),
            action: "backup-copy".into(),
            path: backup_path.clone(),
            reason: e.to_string(),
        })?;

        // Verify the backup by opening read-only and running a health check.
        let config = lbug::SystemConfig::default().read_only(true);
        let verify_result = Self::with_open_lock(&backup_path, || {
            let db = lbug::Database::new(&backup_path, config).map_err(|e| {
                SimardError::RuntimeInitFailed {
                    component: "cognitive-memory".into(),
                    reason: format!("Backup verification open failed: {e}"),
                }
            })?;
            Self::verify_db_health(&db)?;
            Ok(())
        });

        if let Err(e) = verify_result {
            let _ = std::fs::remove_file(&backup_path);
            return Err(SimardError::RuntimeInitFailed {
                component: "cognitive-memory".into(),
                reason: format!("Backup verification failed, removed invalid backup: {e}"),
            });
        }

        Ok(backup_path)
    }

    /// Remove old backups, keeping only the `keep` most recent ones.
    pub fn prune_old_backups(state_root: &Path, keep: usize) {
        let backup_dir = state_root.join("backups");
        if !backup_dir.is_dir() {
            return;
        }
        let prefix = "cognitive_memory.ladybug.";
        let mut backups: Vec<(u64, PathBuf)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&backup_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if let Some(epoch_str) = name_str.strip_prefix(prefix)
                    && let Ok(epoch) = epoch_str.parse::<u64>()
                {
                    backups.push((epoch, entry.path()));
                }
            }
        }
        backups.sort_by_key(|x| std::cmp::Reverse(x.0));
        for (_, path) in backups.into_iter().skip(keep) {
            if let Err(e) = std::fs::remove_file(&path) {
                eprintln!(
                    "[simard] failed to remove old backup {}: {e}",
                    path.display()
                );
            }
        }
    }

    #[cfg(unix)]
    pub(super) fn with_open_lock<T>(
        db_path: &Path,
        f: impl FnOnce() -> SimardResult<T>,
    ) -> SimardResult<T> {
        let lock_path = db_path.with_extension("open.lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SimardError::PersistentStoreIo {
                store: "cognitive-memory".into(),
                action: "create_lock_dir".into(),
                path: parent.to_path_buf(),
                reason: e.to_string(),
            })?;
        }
        let lock_file =
            std::fs::File::create(&lock_path).map_err(|e| SimardError::PersistentStoreIo {
                store: "cognitive-memory".into(),
                action: "create_lock_file".into(),
                path: lock_path.clone(),
                reason: e.to_string(),
            })?;
        let fd = lock_file.as_raw_fd();

        let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            return Err(SimardError::PersistentStoreIo {
                store: "cognitive-memory".into(),
                action: "flock".into(),
                path: lock_path,
                reason: err.to_string(),
            });
        }

        // Record our pid so external tooling (and `memory_ipc::reap_stale_open_lock`)
        // can tell whether the lock owner is still alive after an unclean exit.
        {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&lock_path)
            {
                let _ = writeln!(f, "{}", std::process::id());
            }
        }

        let result = f();

        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
        drop(lock_file);

        result
    }
}
