//! NativeCognitiveMemory backup, restore, and DB-recovery helpers.

use std::os::unix::io::AsRawFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use crate::error::{SimardError, SimardResult};

use super::NativeCognitiveMemory;

/// Atomically copy `src` to `dst`: write to `<dst>.tmp`, fsync the bytes
/// and the directory entry, then rename. The result is either the new
/// `dst` is fully present and durable, or `dst` is unchanged.
fn atomic_copy_with_fsync(src: &Path, dst: &Path) -> SimardResult<()> {
    let tmp = {
        let mut s = dst.as_os_str().to_owned();
        s.push(".tmp");
        PathBuf::from(s)
    };
    // Best-effort cleanup of any leftover .tmp from a prior crashed backup.
    let _ = std::fs::remove_file(&tmp);

    std::fs::copy(src, &tmp).map_err(|e| SimardError::PersistentStoreIo {
        store: "cognitive-memory".into(),
        action: "backup-copy-tmp".into(),
        path: tmp.clone(),
        reason: e.to_string(),
    })?;

    // fsync the file contents so a crash between copy and rename does not
    // leave a torn payload that becomes the next durable backup.
    if let Ok(f) = std::fs::OpenOptions::new().read(true).open(&tmp) {
        let _ = f.sync_all();
    }

    std::fs::rename(&tmp, dst).map_err(|e| SimardError::PersistentStoreIo {
        store: "cognitive-memory".into(),
        action: "backup-rename".into(),
        path: dst.to_path_buf(),
        reason: e.to_string(),
    })?;

    // fsync the parent directory so the rename itself is durable.
    if let Some(parent) = dst.parent()
        && let Ok(d) = std::fs::File::open(parent)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

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

    /// Return all verified backup files in `{state_root}/backups/`, newest
    /// epoch first. Empty `Vec` if the directory is missing or has no
    /// matching entries. Used by `try_restore_from_backup` so it can fall
    /// through to older snapshots when newer ones fail verification.
    fn find_backups_newest_first(db_path: &Path) -> Vec<PathBuf> {
        let Some(state_root) = db_path.parent() else {
            return Vec::new();
        };
        let backup_dir = state_root.join("backups");
        if !backup_dir.is_dir() {
            return Vec::new();
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
        candidates.into_iter().map(|(_, p)| p).collect()
    }

    /// Iterate through every available backup (newest first) and return the
    /// first one that opens cleanly and passes the health check. Each
    /// candidate is copied into place at `db_path`; if the open or health
    /// check fails, the corrupt copy is removed before the next candidate
    /// is tried so we don't leave a half-restored DB behind. Returns
    /// `Err(...)` only when zero backups exist OR every candidate failed
    /// verification.
    fn try_restore_from_backup(db_path: &Path) -> SimardResult<lbug::Database> {
        let backups = Self::find_backups_newest_first(db_path);
        if backups.is_empty() {
            return Err(SimardError::RuntimeInitFailed {
                component: "cognitive-memory".into(),
                reason: "No backups available for restore".into(),
            });
        }

        let total = backups.len();
        let mut last_err: Option<String> = None;

        for (idx, backup_path) in backups.into_iter().enumerate() {
            let epoch_str = backup_path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix("cognitive_memory.ladybug."))
                .unwrap_or("unknown");
            let epoch_secs: u64 = epoch_str.parse().unwrap_or(0);
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(epoch_secs);
            let age_seconds = now_secs.saturating_sub(epoch_secs);

            eprintln!(
                "[simard] attempting restore from backup {} (candidate {}/{}, epoch {epoch_str}, {age_seconds}s old)",
                backup_path.display(),
                idx + 1,
                total
            );

            // Clear any prior partial restore at db_path. We don't keep a
            // separate corrupt-backup here because Step 3 of the recovery
            // sequence already preserved the original under a `.corrupt-*`
            // suffix before reaching this code.
            if db_path.exists() {
                let _ = std::fs::remove_file(db_path);
            }

            if let Err(e) = std::fs::copy(&backup_path, db_path) {
                let msg = format!("copy from {} failed: {e}", backup_path.display());
                eprintln!("[simard] {msg}");
                last_err = Some(msg);
                continue;
            }

            // Clean WAL files before opening the restored copy.
            Self::preemptive_wal_cleanup(db_path);

            // Open with catch_unwind because a corrupt backup can panic
            // inside lbug just like a corrupt main DB does — without this
            // a single bad backup file would crash the whole recovery
            // sequence and prevent us from trying older candidates.
            let db_path_owned = db_path.to_path_buf();
            let opened = catch_unwind(AssertUnwindSafe(|| {
                Self::with_open_lock(&db_path_owned, || {
                    lbug::Database::new(&db_path_owned, lbug::SystemConfig::default()).map_err(
                        |e| SimardError::RuntimeInitFailed {
                            component: "cognitive-memory".into(),
                            reason: format!("Failed to open restored backup: {e}"),
                        },
                    )
                })
                .and_then(|db| Self::verify_db_health(&db).map(|_| db))
            }));

            match opened {
                Ok(Ok(db)) => {
                    eprintln!(
                        "[simard] recovered from backup {} ({age_seconds}s old)",
                        backup_path.display()
                    );
                    return Ok(db);
                }
                Ok(Err(e)) => {
                    let msg = format!(
                        "backup {} failed open/health-check: {e}",
                        backup_path.display()
                    );
                    eprintln!("[simard] {msg}");
                    last_err = Some(msg);
                }
                Err(panic_info) => {
                    let panic_msg = panic_info
                        .downcast_ref::<String>()
                        .map(|s| s.as_str())
                        .or_else(|| panic_info.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown panic");
                    let msg = format!(
                        "backup {} panicked on open: {panic_msg}",
                        backup_path.display()
                    );
                    eprintln!("[simard] {msg}");
                    last_err = Some(msg);
                }
            }

            // This candidate failed; clear the partial copy + WAL before
            // trying the next one so we never present a half-restored DB
            // as "successfully recovered".
            if db_path.exists() {
                let _ = std::fs::remove_file(db_path);
            }
            Self::preemptive_wal_cleanup(db_path);
        }

        Err(SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: format!(
                "All {total} backups failed verification (last error: {})",
                last_err.unwrap_or_else(|| "<none>".into())
            ),
        })
    }

    /// Open LadybugDB with WAL corruption recovery and backup restore.
    ///
    /// Strategy (issue #1710 — restore-from-backup must precede empty-DB):
    /// 1. Preemptively remove empty/unreadable WAL files
    /// 2. Try opening with `catch_unwind` to survive WAL corruption panics
    /// 3. On success, verify the DB is usable with a health-check query
    /// 4. On panic / health-check failure: rename corrupt DB aside,
    ///    remove WAL siblings
    /// 5. **Attempt backup-restore FIRST** — iterate every available backup
    ///    newest-first via `try_restore_from_backup`; the first one that
    ///    opens cleanly and passes the health check wins
    /// 6. Only if **all** backups failed (or none exist) do we fall through
    ///    to creating a fresh empty DB — and we log the data loss loudly
    ///
    /// The previous implementation retried `try_open_and_verify(db_path)`
    /// at step 5, which always succeeded by creating a fresh empty DB
    /// (because step 4 had renamed the corrupt original away). That made
    /// the subsequent `try_restore_from_backup` call dead code and caused
    /// real data loss. See `recovery_uses_backup_when_main_corrupt` in
    /// `src/cognitive_memory/tests_mod.rs` for the regression pin.
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

        // Step 3: rename the corrupt DB aside (preserve for forensics) and
        // remove WAL siblings so a subsequent open is unambiguous.
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

        // Step 4 (was Step 5): try restoring from backup BEFORE creating an
        // empty DB. This is the issue-#1710 fix: the previous ordering let
        // the retry-open path silently succeed with a fresh empty DB and
        // never reached the restore code, destroying user data.
        //
        // `try_restore_from_backup` iterates every backup newest-first and
        // returns Ok with the first one that opens + passes health check;
        // it returns Err only when no backups exist OR every candidate
        // failed verification.
        let backup_count = Self::find_backups_newest_first(db_path).len();
        match Self::try_restore_from_backup(db_path) {
            Ok(db) => return Ok(db),
            Err(e) => {
                if backup_count == 0 {
                    eprintln!(
                        "[simard] no backups available — creating fresh empty DB \
                         (DATA LOSS). Corrupt original preserved at {}",
                        corrupt_backup.display()
                    );
                } else {
                    eprintln!(
                        "[simard] all {backup_count} backups failed verification — \
                         creating fresh empty DB (DATA LOSS). Last error: {e}. \
                         Corrupt original preserved at {}",
                        corrupt_backup.display()
                    );
                }
            }
        }

        // Step 5 (was Step 6): last resort — ensure clean slate and create
        // fresh empty DB. This branch is now reached ONLY when restore
        // truly cannot recover anything; the data-loss log line above is
        // unconditional so a crash here will always leave a paper trail.
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        Self::preemptive_wal_cleanup(db_path);
        try_open(db_path)
    }

    /// Create a verified backup of the DB file. Returns the backup path on
    /// success. Used by the OODA daemon for periodic backups.
    ///
    /// Captures the main DB file and any associated `.wal` files atomically:
    /// each is copied to a `.tmp` sibling, fsynced, then renamed into place
    /// with a shared timestamp suffix. Restore = copy both files back.
    /// Callers should call [`Self::checkpoint`] on a live writer **before**
    /// invoking this so committed-but-WAL-resident writes are flushed
    /// (issue #1631).
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

        // Atomic copy: write to .tmp, fsync, then rename. Same TS suffix
        // for the main file and any .wal sibling so a restore is unambiguous.
        atomic_copy_with_fsync(&db_path, &backup_path)?;

        // Capture WAL siblings alongside (same TS). Either, both, or neither
        // may exist depending on lbug version; whichever exist are paired.
        for wal_src in Self::wal_paths(&db_path) {
            if !wal_src.exists() {
                continue;
            }
            let wal_name = wal_src
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("cognitive_memory.ladybug.wal");
            // The destination filename is `<wal_name>.<ts>` so the pair is
            // discoverable next to the main backup with the same timestamp.
            let wal_dst = backup_dir.join(format!("{wal_name}.{ts}"));
            atomic_copy_with_fsync(&wal_src, &wal_dst)?;
        }

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

    /// Remove old backups, keeping only the `keep` most recent ones. Removes
    /// any paired `.wal` backup files that share the same timestamp suffix.
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
                // Index only the main-DB backups by epoch; .wal siblings are
                // pruned alongside their main file by epoch.
                if let Some(epoch_str) = name_str.strip_prefix(prefix)
                    && let Ok(epoch) = epoch_str.parse::<u64>()
                {
                    backups.push((epoch, entry.path()));
                }
            }
        }
        backups.sort_by_key(|x| std::cmp::Reverse(x.0));
        for (epoch, path) in backups.into_iter().skip(keep) {
            if let Err(e) = std::fs::remove_file(&path) {
                eprintln!(
                    "[simard] failed to remove old backup {}: {e}",
                    path.display()
                );
            }
            // Remove paired .wal files with the same epoch (any of the two
            // wal_name variants lbug may use).
            for wal_name in ["cognitive_memory.ladybug.wal", "cognitive_memory.wal"] {
                let paired = backup_dir.join(format!("{wal_name}.{epoch}"));
                if paired.exists() {
                    let _ = std::fs::remove_file(&paired);
                }
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
