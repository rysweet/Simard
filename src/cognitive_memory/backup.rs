//! NativeCognitiveMemory backup, restore, and DB-recovery helpers.

use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{SimardError, SimardResult};

use super::NativeCognitiveMemory;
use super::fsync;

/// Compute the SHA-256 hex digest of `path`'s full contents.
///
/// Used by `atomic_copy_with_fsync` after the fsync pipeline completes to
/// **verify post-fsync state**: we re-open both src and dst, hash each,
/// and require they match before declaring the backup durable. Trusting
/// only the syscall return is insufficient — a torn write that returned
/// `Ok(0)` from the fs layer (rare but observed on flaky storage) would
/// otherwise produce a "valid" backup whose bytes drifted from the
/// source. Issue #1973, decision D3.
fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    // SHA-256 is exactly 32 bytes → 64 hex chars. Pre-allocate once and
    // write each byte in place; the previous `map(format!).collect()`
    // pattern produced 32 transient `String`s plus the final collected
    // buffer (33 allocations) for a payload whose total length is known
    // at compile time.
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        write!(hex, "{b:02x}").expect("writing to String cannot fail");
    }
    Ok(hex)
}

/// Atomically copy `src` to `dst`: write to `<dst>.tmp`, fsync the bytes
/// and the directory entry, then rename. The result is either the new
/// `dst` is fully present and durable, or `dst` is unchanged.
///
/// **Verified-fsync** (issue #1973, decision D3): after the rename + dir
/// fsync, the function re-opens `dst` and recomputes its SHA-256 against
/// `src`'s SHA-256. A mismatch surfaces as `SimardError::PersistentStoreIo`
/// with `action = "verify-readback"` so a torn or buffered write cannot
/// pass as a successful backup. Without this check the previous version
/// happily returned `Ok(())` even when the fsync syscalls below silently
/// dropped errors via `let _ = ...`.
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
    // leave a torn payload that becomes the next durable backup. Errors
    // are propagated (issue #1973): the previous `let _ = f.sync_all()`
    // could return Ok with un-fsynced bytes if the kernel surfaced EIO,
    // which is the exact failure mode the per-write barrier exists to
    // prevent.
    fsync::open_and_fsync(&tmp, "backup-open-tmp-for-fsync", "backup-fsync-tmp", None)?;

    std::fs::rename(&tmp, dst).map_err(|e| SimardError::PersistentStoreIo {
        store: "cognitive-memory".into(),
        action: "backup-rename".into(),
        path: dst.to_path_buf(),
        reason: e.to_string(),
    })?;

    // fsync the parent directory so the rename itself is durable.
    // Propagated as PersistentStoreIo with action="backup-fsync-parent-dir"
    // — the prior `let _ = d.sync_all()` could lose an EIO and present a
    // non-durable dirent as a successful backup. Issue #1973.
    if let Some(parent) = dst.parent() {
        fsync::open_and_fsync(
            parent,
            "backup-open-parent-for-fsync",
            "backup-fsync-parent-dir",
            None,
        )?;
    }

    // Verified-fsync: re-open src + dst (separate fds from the ones we
    // fsynced above) and hash each. A mismatch means the on-disk dst is
    // not a bit-exact replica of src, even though every preceding syscall
    // returned success — surface this as a typed error rather than
    // silently returning Ok. Issue #1973 decision D3.
    let src_hash = sha256_file(src).map_err(|e| SimardError::PersistentStoreIo {
        store: "cognitive-memory".into(),
        action: "verify-readback-src-hash".into(),
        path: src.to_path_buf(),
        reason: e.to_string(),
    })?;
    let dst_hash = sha256_file(dst).map_err(|e| SimardError::PersistentStoreIo {
        store: "cognitive-memory".into(),
        action: "verify-readback-dst-hash".into(),
        path: dst.to_path_buf(),
        reason: e.to_string(),
    })?;
    if src_hash != dst_hash {
        return Err(SimardError::PersistentStoreIo {
            store: "cognitive-memory".into(),
            action: "verify-readback".into(),
            path: dst.to_path_buf(),
            reason: format!(
                "post-fsync hash mismatch: src={src_hash} dst={dst_hash} \
                 (the backup file on disk is not a bit-exact replica of \
                 the source — fsync pipeline succeeded but durability is \
                 not guaranteed)"
            ),
        });
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
    ///
    /// Returns `Err` if a WAL that *should* be removed cannot be deleted
    /// (issue #1975: previously swallowed via `let _ =`).
    pub fn preemptive_wal_cleanup(db_path: &Path) -> SimardResult<()> {
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
                std::fs::remove_file(&wal).map_err(|e| {
                    super::metrics::increment("wal_cleanup_failed", "preemptive_wal_cleanup");
                    SimardError::PersistentStoreIo {
                        store: "cognitive-memory".into(),
                        action: "preemptive_wal_cleanup".into(),
                        path: wal.clone(),
                        reason: e.to_string(),
                    }
                })?;
            }
        }
        Ok(())
    }

    /// Recovery-replay fsync helper (issue #1973, decision D4).
    ///
    /// After `try_restore_from_backup` copies a candidate backup into
    /// `db_path`, this routine fsyncs the freshly-restored file and its
    /// parent directory so a crash before `open_db_with_recovery`
    /// completes cannot leave a half-written replica that a subsequent
    /// recovery cycle treats as canonical.
    ///
    /// Errors map to `SimardError::PersistentStoreIo` with
    /// `action = "recovery-replay-fsync"` for symmetry with the
    /// `post_write_barrier` write-path pipeline.
    #[cfg(unix)]
    fn fsync_recovery_replay(db_path: &Path) -> SimardResult<()> {
        fsync::open_and_fsync(
            db_path,
            "recovery-replay-fsync-open",
            "recovery-replay-fsync",
            None,
        )?;
        if let Some(parent) = db_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fsync::open_and_fsync(
                parent,
                "recovery-replay-fsync-parent-open",
                "recovery-replay-fsync-parent",
                None,
            )?;
        }
        Ok(())
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
                std::fs::remove_file(db_path).map_err(|e| {
                    super::metrics::increment(
                        "restore_cleanup_failed",
                        "try_restore_from_backup:pre",
                    );
                    SimardError::PersistentStoreIo {
                        store: "cognitive-memory".into(),
                        action: "restore-cleanup-pre".into(),
                        path: db_path.to_path_buf(),
                        reason: e.to_string(),
                    }
                })?;
            }

            if let Err(e) = std::fs::copy(&backup_path, db_path) {
                let msg = format!("copy from {} failed: {e}", backup_path.display());
                eprintln!("[simard] {msg}");
                last_err = Some(msg);
                continue;
            }

            // Per-write barrier on recovery replay (issue #1973, decision D4):
            // fsync the freshly-copied DB file and its parent directory so a
            // subsequent crash before this restored DB is opened cannot leave
            // a half-written replica. Mirrors the pattern in
            // `NativeCognitiveMemory::post_write_barrier`. Failure short-
            // circuits this candidate and falls through to the next backup.
            if let Err(e) = Self::fsync_recovery_replay(db_path) {
                let msg = format!(
                    "recovery-replay fsync failed for {}: {e}",
                    backup_path.display()
                );
                eprintln!("[simard] {msg}");
                last_err = Some(msg);
                if db_path.exists() {
                    let _ = std::fs::remove_file(db_path);
                }
                continue;
            }

            // Clean WAL files before opening the restored copy.
            // Best-effort in restore loop — don't abort the fallback sequence.
            let _ = Self::preemptive_wal_cleanup(db_path);

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
            if db_path.exists()
                && let Err(e) = std::fs::remove_file(db_path)
            {
                super::metrics::increment("restore_cleanup_failed", "try_restore_from_backup:post");
                eprintln!(
                    "[simard] failed to remove partial restore at {}: {e}",
                    db_path.display()
                );
            }
            // WAL cleanup is best-effort here: we're in a fallback loop
            // and failing to remove a WAL sibling shouldn't block trying
            // the next candidate.
            let _ = Self::preemptive_wal_cleanup(db_path);
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
        Self::preemptive_wal_cleanup(db_path)?;

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
                // Issue #1967: lock contention is NOT corruption. If a peer
                // process (typically `simard-ooda`) holds the LadybugDB lock,
                // LadybugDB surfaces "Could not set lock on file" — historic
                // behavior treated this as corruption, renamed the file to
                // `cognitive_memory.corrupt-<ts>`, and restored from backup,
                // silently rolling state back hours. Detect that signature
                // here and return a clear lock-contention error without
                // touching the on-disk DB.
                if Self::is_lock_contention_error(&e) {
                    return Err(SimardError::RuntimeInitFailed {
                        component: "cognitive-memory".into(),
                        reason: format!(
                            "LadybugDB at {} is locked by another process \
                             (typically the simard-ooda daemon). The meeting / \
                             goal CLI must share the daemon's writer bridge \
                             instead of opening the DB directly. Underlying \
                             error: {e}",
                            db_path.display()
                        ),
                    });
                }
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
            if wal.exists()
                && let Err(e) = std::fs::remove_file(&wal)
            {
                super::metrics::increment("wal_cleanup_failed", "open_db_with_recovery:step3");
                eprintln!(
                    "[simard] failed to remove WAL sibling {}: {e}",
                    wal.display()
                );
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
            std::fs::remove_file(db_path).map_err(|e| {
                super::metrics::increment(
                    "restore_cleanup_failed",
                    "open_db_with_recovery:last_resort",
                );
                SimardError::PersistentStoreIo {
                    store: "cognitive-memory".into(),
                    action: "recovery-last-resort-cleanup".into(),
                    path: db_path.to_path_buf(),
                    reason: e.to_string(),
                }
            })?;
        }
        // Best-effort WAL cleanup before fresh create — don't abort
        // the last-resort path if this fails.
        let _ = Self::preemptive_wal_cleanup(db_path);
        try_open(db_path)
    }

    /// Returns true when the open-error signature matches LadybugDB
    /// reporting that another process already holds the file lock.
    ///
    /// Issue #1967: the meeting REPL — and any other client that opens the
    /// DB directly while the daemon is running — receives this error from
    /// LadybugDB. Historically, the recovery ladder above treated *every*
    /// open failure as corruption, renamed the on-disk file to
    /// `cognitive_memory.corrupt-<ts>`, and silently restored from a stale
    /// backup. Detecting the lock signature here lets callers fail fast
    /// with a clear error and leaves the DB untouched.
    ///
    /// Detection is string-based on the `Display` of the inner LadybugDB
    /// error because the upstream library does not yet expose a typed
    /// `Locked` variant. The matched substrings come from
    /// LadybugDB's own error messages and are stable across versions
    /// observed in production.
    pub(super) fn is_lock_contention_error(err: &SimardError) -> bool {
        let msg = err.to_string();
        msg.contains("Could not set lock on file")
            || msg.contains("Could not set lock")
            || msg.contains("Resource temporarily unavailable")
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
    ///
    /// Returns a [`PruneOutcome`](super::metrics::PruneOutcome) so the caller
    /// can observe per-file failures without aborting the whole prune
    /// (issue #1975).
    pub fn prune_old_backups(state_root: &Path, keep: usize) -> super::metrics::PruneOutcome {
        let mut outcome = super::metrics::PruneOutcome {
            removed: 0,
            failed: Vec::new(),
        };
        let backup_dir = state_root.join("backups");
        if !backup_dir.is_dir() {
            return outcome;
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
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    outcome.removed += 1;
                }
                Err(e) => {
                    super::metrics::increment("prune_remove_failed", "prune_old_backups:main");
                    eprintln!(
                        "[simard] failed to remove old backup {}: {e}",
                        path.display()
                    );
                    outcome.failed.push((path, e));
                }
            }
            // Remove paired .wal files with the same epoch (any of the two
            // wal_name variants lbug may use).
            for wal_name in ["cognitive_memory.ladybug.wal", "cognitive_memory.wal"] {
                let paired = backup_dir.join(format!("{wal_name}.{epoch}"));
                if paired.exists() {
                    if let Err(e) = std::fs::remove_file(&paired) {
                        super::metrics::increment(
                            "prune_wal_remove_failed",
                            "prune_old_backups:wal",
                        );
                        eprintln!(
                            "[simard] failed to remove paired WAL backup {}: {e}",
                            paired.display()
                        );
                        outcome.failed.push((paired, e));
                    } else {
                        outcome.removed += 1;
                    }
                }
            }
        }
        outcome
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

// ============================================================================
// Inline unit tests for backup.rs (issue #2036)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cognitive_memory::CognitiveMemoryOps;

    fn test_mem() -> NativeCognitiveMemory {
        NativeCognitiveMemory::in_memory().expect("in-memory DB should create")
    }

    // ── sha256_file ────────────────────────────────────────────────────

    #[test]
    fn sha256_file_produces_64_hex_chars() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"hello world").unwrap();
        let hash = sha256_file(tmp.path()).unwrap();
        assert_eq!(hash.len(), 64, "SHA-256 hex digest must be 64 chars");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "must be hex chars"
        );
    }

    #[test]
    fn sha256_file_deterministic() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"deterministic content").unwrap();
        let h1 = sha256_file(tmp.path()).unwrap();
        let h2 = sha256_file(tmp.path()).unwrap();
        assert_eq!(h1, h2, "same content must produce same hash");
    }

    #[test]
    fn sha256_file_differs_for_different_content() {
        let f1 = tempfile::NamedTempFile::new().unwrap();
        let f2 = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f1.path(), b"aaa").unwrap();
        std::fs::write(f2.path(), b"bbb").unwrap();
        let h1 = sha256_file(f1.path()).unwrap();
        let h2 = sha256_file(f2.path()).unwrap();
        assert_ne!(h1, h2, "different content must produce different hashes");
    }

    #[test]
    fn sha256_file_errors_on_missing_file() {
        let result = sha256_file(std::path::Path::new("/nonexistent/file"));
        assert!(result.is_err());
    }

    // ── atomic_copy_with_fsync ─────────────────────────────────────────

    #[test]
    fn atomic_copy_with_fsync_creates_exact_copy() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.db");
        let dst = dir.path().join("dest.db");
        std::fs::write(&src, b"important data here").unwrap();

        atomic_copy_with_fsync(&src, &dst).unwrap();

        assert!(dst.exists(), "destination must exist after copy");
        assert_eq!(
            std::fs::read(&src).unwrap(),
            std::fs::read(&dst).unwrap(),
            "dst must be byte-identical to src"
        );
    }

    #[test]
    fn atomic_copy_with_fsync_removes_leftover_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.db");
        let dst = dir.path().join("dest.db");
        let tmp = dir.path().join("dest.db.tmp");
        std::fs::write(&src, b"source data").unwrap();
        std::fs::write(&tmp, b"stale tmp leftover").unwrap();

        atomic_copy_with_fsync(&src, &dst).unwrap();

        assert!(!tmp.exists(), "leftover .tmp must be cleaned up");
        assert!(dst.exists());
    }

    #[test]
    fn atomic_copy_with_fsync_errors_on_missing_src() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("missing.db");
        let dst = dir.path().join("dest.db");
        let result = atomic_copy_with_fsync(&src, &dst);
        assert!(result.is_err(), "missing source must be an error");
    }

    #[test]
    fn atomic_copy_with_fsync_overwrites_existing_dst() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.db");
        let dst = dir.path().join("dest.db");
        std::fs::write(&src, b"new content").unwrap();
        std::fs::write(&dst, b"old content").unwrap();

        atomic_copy_with_fsync(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"new content");
    }

    // ── post_write_barrier ─────────────────────────────────────────────

    #[test]
    fn post_write_barrier_noop_for_in_memory() {
        let mem = test_mem();
        assert!(!mem.durable_writes);
        mem.post_write_barrier("test")
            .expect("in-memory barrier must be no-op");
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn post_write_barrier_succeeds_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let mem = NativeCognitiveMemory::open(tmp.path()).unwrap();
        assert!(mem.durable_writes);
        mem.post_write_barrier("test_backup")
            .expect("on-disk barrier must succeed");
    }

    // ── wal_paths ──────────────────────────────────────────────────────

    #[test]
    fn wal_paths_returns_two_entries() {
        let db_path = std::path::Path::new("/tmp/cognitive_memory.ladybug");
        let paths = NativeCognitiveMemory::wal_paths(db_path);
        assert_eq!(paths.len(), 2);
        for p in &paths {
            let s = p.to_string_lossy();
            assert!(s.contains("wal"), "WAL path should contain 'wal': {s}");
        }
    }

    // ── preemptive_wal_cleanup ──────────────────────────────────────────

    #[test]
    fn preemptive_wal_cleanup_removes_empty_wal() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.ladybug");
        std::fs::write(&db_path, b"db data").unwrap();

        for wal in NativeCognitiveMemory::wal_paths(&db_path) {
            std::fs::write(&wal, b"").unwrap(); // empty WAL
        }

        NativeCognitiveMemory::preemptive_wal_cleanup(&db_path).unwrap();

        for wal in NativeCognitiveMemory::wal_paths(&db_path) {
            assert!(
                !wal.exists(),
                "empty WAL should be removed: {}",
                wal.display()
            );
        }
    }

    #[test]
    fn preemptive_wal_cleanup_keeps_nonempty_wal() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.ladybug");
        std::fs::write(&db_path, b"db data").unwrap();

        let wals = NativeCognitiveMemory::wal_paths(&db_path);
        std::fs::write(&wals[0], b"valid wal data").unwrap();

        NativeCognitiveMemory::preemptive_wal_cleanup(&db_path).unwrap();

        assert!(wals[0].exists(), "non-empty WAL must be preserved");
    }

    #[test]
    fn preemptive_wal_cleanup_noop_when_no_wals() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.ladybug");
        std::fs::write(&db_path, b"db data").unwrap();
        // No WAL files exist
        NativeCognitiveMemory::preemptive_wal_cleanup(&db_path).unwrap();
    }

    // ── is_lock_contention_error ────────────────────────────────────────

    #[test]
    fn is_lock_contention_detects_lock_message() {
        let err = SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: "Could not set lock on file /some/path".into(),
        };
        assert!(NativeCognitiveMemory::is_lock_contention_error(&err));
    }

    #[test]
    fn is_lock_contention_detects_eagain() {
        let err = SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: "Resource temporarily unavailable".into(),
        };
        assert!(NativeCognitiveMemory::is_lock_contention_error(&err));
    }

    #[test]
    fn is_lock_contention_rejects_corruption() {
        let err = SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: "WAL header CRC mismatch".into(),
        };
        assert!(!NativeCognitiveMemory::is_lock_contention_error(&err));
    }

    // ── find_backups_newest_first ────────────────────────────────────────

    #[test]
    fn find_backups_newest_first_sorted_descending() {
        let dir = tempfile::tempdir().unwrap();
        let state_root = dir.path();
        let backup_dir = state_root.join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        let db_path = state_root.join("cognitive_memory.ladybug");

        // Create backup files with known epochs
        for epoch in [100u64, 300, 200] {
            std::fs::write(
                backup_dir.join(format!("cognitive_memory.ladybug.{epoch}")),
                b"db",
            )
            .unwrap();
        }

        let backups = NativeCognitiveMemory::find_backups_newest_first(&db_path);
        assert_eq!(backups.len(), 3);
        // Should be sorted newest first: 300, 200, 100
        let names: Vec<String> = backups
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names[0].ends_with(".300"), "newest first: {names:?}");
        assert!(names[1].ends_with(".200"), "middle: {names:?}");
        assert!(names[2].ends_with(".100"), "oldest last: {names:?}");
    }

    #[test]
    fn find_backups_returns_empty_when_no_backup_dir() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("cognitive_memory.ladybug");
        let backups = NativeCognitiveMemory::find_backups_newest_first(&db_path);
        assert!(backups.is_empty());
    }

    // ── verify_db_health ────────────────────────────────────────────────

    #[test]
    fn verify_db_health_succeeds_on_healthy_db() {
        let mem = test_mem();
        NativeCognitiveMemory::verify_db_health(&mem.db).unwrap();
    }

    // ── create_verified_backup ──────────────────────────────────────────

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn create_verified_backup_produces_valid_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let state_root = tmp.path().to_path_buf();

        {
            let mem = NativeCognitiveMemory::open(&state_root).unwrap();
            mem.store_fact("backup-test", "data", 0.9, &[], "test")
                .unwrap();
        }

        let backup_path = NativeCognitiveMemory::create_verified_backup(&state_root).unwrap();
        assert!(backup_path.exists(), "backup file must exist");
        assert!(
            backup_path
                .to_string_lossy()
                .contains("backups/cognitive_memory.ladybug."),
            "backup must be in backups/ dir"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn create_verified_backup_errors_when_no_db_file() {
        let tmp = tempfile::tempdir().unwrap();
        let err = NativeCognitiveMemory::create_verified_backup(tmp.path());
        assert!(err.is_err(), "no DB file → must error");
    }

    // ── prune_old_backups ──────────────────────────────────────────────

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn prune_old_backups_keeps_n_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let state_root = tmp.path().to_path_buf();
        let backup_dir = state_root.join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        for epoch in [100u64, 200, 300, 400, 500] {
            std::fs::write(
                backup_dir.join(format!("cognitive_memory.ladybug.{epoch}")),
                b"db",
            )
            .unwrap();
        }

        let outcome = NativeCognitiveMemory::prune_old_backups(&state_root, 2);
        assert_eq!(outcome.removed, 3, "should remove 3 oldest");
        assert!(outcome.failed.is_empty(), "no failures expected");

        // The 2 newest (500, 400) should survive
        assert!(backup_dir.join("cognitive_memory.ladybug.500").exists());
        assert!(backup_dir.join("cognitive_memory.ladybug.400").exists());
        assert!(!backup_dir.join("cognitive_memory.ladybug.100").exists());
    }

    #[test]
    fn prune_old_backups_noop_when_no_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let outcome = NativeCognitiveMemory::prune_old_backups(tmp.path(), 5);
        assert_eq!(outcome.removed, 0);
        assert!(outcome.failed.is_empty());
    }

    // ── with_open_lock ─────────────────────────────────────────────────

    #[test]
    fn with_open_lock_executes_closure() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.ladybug");
        std::fs::write(&db_path, b"").unwrap();

        let result = NativeCognitiveMemory::with_open_lock(&db_path, || Ok(42u32));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn with_open_lock_propagates_error() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.ladybug");
        std::fs::write(&db_path, b"").unwrap();

        let result: SimardResult<()> = NativeCognitiveMemory::with_open_lock(&db_path, || {
            Err(SimardError::RuntimeInitFailed {
                component: "test".into(),
                reason: "intentional".into(),
            })
        });
        assert!(result.is_err());
    }

    // ── fsync_recovery_replay ──────────────────────────────────────────

    #[test]
    fn fsync_recovery_replay_succeeds_on_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.ladybug");
        std::fs::write(&db_path, b"db contents for fsync test").unwrap();
        NativeCognitiveMemory::fsync_recovery_replay(&db_path)
            .expect("fsync_recovery_replay must succeed on a real file");
    }

    #[test]
    fn fsync_recovery_replay_errors_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nonexistent.ladybug");
        let result = NativeCognitiveMemory::fsync_recovery_replay(&db_path);
        assert!(result.is_err(), "missing file must error");
    }
}
