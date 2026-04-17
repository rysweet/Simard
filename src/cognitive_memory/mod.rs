//! Native cognitive memory backed by LadybugDB.
//!
//! Replaces the Python bridge (`simard_memory_server.py`) with a direct Rust
//! implementation. The [`CognitiveMemoryOps`] trait defines the API shared by
//! both the native backend ([`NativeCognitiveMemory`]) and the legacy bridge
//! client ([`CognitiveMemoryBridge`](crate::memory_bridge::CognitiveMemoryBridge)).
//!
//! The flock-based multi-writer serialization is copied from the skwaq
//! reference implementation in `ladybug_db.rs`.

pub(crate) mod schema;

#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

/// Trait abstracting cognitive memory operations.
///
/// Both [`NativeCognitiveMemory`] (LadybugDB) and
/// [`CognitiveMemoryBridge`](crate::memory_bridge::CognitiveMemoryBridge)
/// (Python subprocess) implement this trait so callers are backend-agnostic.
pub trait CognitiveMemoryOps: Send + Sync {
    fn record_sensory(
        &self,
        modality: &str,
        raw_data: &str,
        ttl_seconds: u64,
    ) -> SimardResult<String>;

    fn prune_expired_sensory(&self) -> SimardResult<usize>;

    fn push_working(
        &self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> SimardResult<String>;

    fn get_working(&self, task_id: &str) -> SimardResult<Vec<CognitiveWorkingSlot>>;

    fn clear_working(&self, task_id: &str) -> SimardResult<usize>;

    fn store_episode(
        &self,
        content: &str,
        source_label: &str,
        metadata: Option<&serde_json::Value>,
    ) -> SimardResult<String>;

    fn consolidate_episodes(&self, batch_size: u32) -> SimardResult<Option<String>>;

    fn store_fact(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String>;

    fn search_facts(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>>;

    fn store_procedure(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
    ) -> SimardResult<String>;

    fn recall_procedure(&self, query: &str, limit: u32) -> SimardResult<Vec<CognitiveProcedure>>;

    fn store_prospective(
        &self,
        description: &str,
        trigger_condition: &str,
        action_on_trigger: &str,
        priority: i64,
    ) -> SimardResult<String>;

    fn check_triggers(&self, content: &str) -> SimardResult<Vec<CognitiveProspective>>;

    fn get_statistics(&self) -> SimardResult<CognitiveStatistics>;
}

// ============================================================================
// NativeCognitiveMemory — LadybugDB backend
// ============================================================================

/// Native cognitive memory backed by an embedded LadybugDB graph database.
///
/// Uses flock serialization for safe multi-writer access (same pattern as
/// the skwaq `LadybugGraphDb`). All errors propagate via [`SimardResult`].
pub struct NativeCognitiveMemory {
    db: Arc<lbug::Database>,
    #[allow(dead_code)]
    path: PathBuf,
    #[allow(dead_code)]
    _temp_dir: Option<Arc<tempfile::TempDir>>,
}

// SAFETY: lbug::Database is thread-safe by design (internal locking).
unsafe impl Send for NativeCognitiveMemory {}
unsafe impl Sync for NativeCognitiveMemory {}

impl NativeCognitiveMemory {
    /// Open or create a LadybugDB cognitive memory database under `state_root`.
    ///
    /// The database directory is `<state_root>/cognitive_memory.ladybug`.
    /// Uses flock to serialize `Database::new()` across processes.
    #[cfg(unix)]
    pub fn open(state_root: &Path) -> SimardResult<Self> {
        std::fs::create_dir_all(state_root).map_err(|e| SimardError::PersistentStoreIo {
            store: "cognitive-memory".into(),
            action: "create_dir".into(),
            path: state_root.to_path_buf(),
            reason: e.to_string(),
        })?;
        let db_path = state_root.join("cognitive_memory.ladybug");

        // Migrate from old KuzuDB directory layout to native LadybugDB file.
        // The Python bridge stored KuzuDB data as a directory; lbug expects a file.
        if db_path.is_dir() {
            let backup = state_root.join("cognitive_memory.ladybug.kuzu-backup");
            eprintln!(
                "[simard] migrating old KuzuDB directory → {}",
                backup.display()
            );
            std::fs::rename(&db_path, &backup).map_err(|e| SimardError::PersistentStoreIo {
                store: "cognitive-memory".into(),
                action: "migrate-kuzu-backup".into(),
                path: db_path.clone(),
                reason: e.to_string(),
            })?;
        }

        let db = Self::open_db_with_recovery(&db_path)?;
        let mem = Self {
            db: Arc::new(db),
            path: db_path,
            _temp_dir: None,
        };
        mem.ensure_schema()?;
        eprintln!(
            "[simard] native cognitive memory active — LadybugDB at {}",
            state_root.display()
        );
        Ok(mem)
    }

    /// Create an in-memory LadybugDB for tests (no flock needed).
    pub fn in_memory() -> SimardResult<Self> {
        let tmp = tempfile::tempdir().map_err(|e| SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: format!("Failed to create temp dir: {e}"),
        })?;
        let db_path = tmp.path().join("cognitive_memory_test");
        let db = lbug::Database::new(
            &db_path,
            lbug::SystemConfig::default()
                .buffer_pool_size(64 * 1024 * 1024)
                .max_db_size(1 << 28)
                .max_num_threads(1),
        )
        .map_err(|e| SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: format!("Failed to create in-memory LadybugDB: {e}"),
        })?;
        let mem = Self {
            db: Arc::new(db),
            path: db_path,
            _temp_dir: Some(Arc::new(tmp)),
        };
        mem.ensure_schema()?;
        Ok(mem)
    }

    /// Open LadybugDB in **read-only** mode for concurrent access.
    ///
    /// Multiple processes can open the same DB read-only simultaneously
    /// (no exclusive flock needed). Uses `SystemConfig::read_only(true)`
    /// following the skwaq `LadybugGraphDb::open_read_only` pattern.
    /// Write operations will fail — use `open()` for the primary writer.
    #[cfg(unix)]
    pub fn open_read_only(state_root: &Path) -> SimardResult<Self> {
        let db_path = state_root.join("cognitive_memory.ladybug");
        if !db_path.exists() {
            return Err(SimardError::RuntimeInitFailed {
                component: "cognitive-memory".into(),
                reason: format!(
                    "Cannot open LadybugDB read-only: {} does not exist",
                    db_path.display()
                ),
            });
        }
        let config = lbug::SystemConfig::default().read_only(true);
        let db = Self::with_open_lock(&db_path, || {
            lbug::Database::new(&db_path, config).map_err(|e| SimardError::RuntimeInitFailed {
                component: "cognitive-memory".into(),
                reason: format!(
                    "Failed to open LadybugDB read-only at {}: {e}",
                    db_path.display()
                ),
            })
        })?;
        let mem = Self {
            db: Arc::new(db),
            path: db_path,
            _temp_dir: None,
        };
        eprintln!(
            "[simard] cognitive memory opened read-only — LadybugDB at {}",
            state_root.display()
        );
        Ok(mem)
    }

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
        let conn =
            lbug::Connection::new(db).map_err(|e| SimardError::RuntimeInitFailed {
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
                    conn.query("RETURN 1").map_err(|e2| SimardError::RuntimeInitFailed {
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
                if let Some(epoch_str) = name_str.strip_prefix(prefix) {
                    if let Ok(epoch) = epoch_str.parse::<u64>() {
                        candidates.push((epoch, entry.path()));
                    }
                }
            }
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0));
        candidates.into_iter().next().map(|(_, p)| p)
    }

    /// Try to restore from the most recent verified backup.
    fn try_restore_from_backup(db_path: &Path) -> SimardResult<lbug::Database> {
        let backup_path = Self::find_latest_backup(db_path).ok_or_else(|| {
            SimardError::RuntimeInitFailed {
                component: "cognitive-memory".into(),
                reason: "No backups available for restore".into(),
            }
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
    fn open_db_with_recovery(db_path: &Path) -> SimardResult<lbug::Database> {
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
                if let Some(epoch_str) = name_str.strip_prefix(prefix) {
                    if let Ok(epoch) = epoch_str.parse::<u64>() {
                        backups.push((epoch, entry.path()));
                    }
                }
            }
        }
        backups.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, path) in backups.into_iter().skip(keep) {
            if let Err(e) = std::fs::remove_file(&path) {
                eprintln!("[simard] failed to remove old backup {}: {e}", path.display());
            }
        }
    }

    #[cfg(unix)]
    fn with_open_lock<T>(db_path: &Path, f: impl FnOnce() -> SimardResult<T>) -> SimardResult<T> {
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

        let result = f();

        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
        drop(lock_file);

        result
    }

    fn conn(&self) -> SimardResult<lbug::Connection<'_>> {
        lbug::Connection::new(&self.db).map_err(|e| SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: format!("Failed to create LadybugDB connection: {e}"),
        })
    }

    fn query(&self, cypher: &str) -> SimardResult<Vec<Vec<lbug::Value>>> {
        let conn = self.conn()?;
        let result = conn
            .query(cypher)
            .map_err(|e| SimardError::BridgeCallFailed {
                bridge: "cognitive-memory-native".into(),
                method: "query".into(),
                reason: format!("{e}\nCypher: {cypher}"),
            })?;
        Ok(result.collect())
    }

    fn execute(&self, cypher: &str) -> SimardResult<()> {
        self.conn()?
            .query(cypher)
            .map_err(|e| SimardError::BridgeCallFailed {
                bridge: "cognitive-memory-native".into(),
                method: "execute".into(),
                reason: format!("{e}\nCypher: {cypher}"),
            })?;
        Ok(())
    }

    fn ensure_schema(&self) -> SimardResult<()> {
        for ddl in schema::SCHEMA_DDL {
            if let Err(e) = self.execute(ddl) {
                let msg = format!("{e}");
                if !msg.contains("already exists") {
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    fn new_id(prefix: &str) -> String {
        format!("{prefix}_{}", uuid::Uuid::now_v7().simple())
    }

    fn now_secs() -> SimardResult<f64> {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .map_err(|_| SimardError::ClockBeforeUnixEpoch {
                reason: "system clock before Unix epoch".into(),
            })
    }
}

fn as_str(val: &lbug::Value) -> Option<&str> {
    match val {
        lbug::Value::String(s) => Some(s.as_str()),
        _ => None,
    }
}

fn as_i64(val: &lbug::Value) -> Option<i64> {
    match val {
        lbug::Value::Int64(n) => Some(*n),
        _ => None,
    }
}

fn as_f64(val: &lbug::Value) -> Option<f64> {
    match val {
        lbug::Value::Double(d) => Some(*d),
        lbug::Value::Int64(n) => Some(*n as f64),
        _ => None,
    }
}

/// Escape a string for safe inclusion in a single-quoted Cypher literal.
///
/// Handles backslash, single-quote, newlines, carriage returns, tabs, and
/// null bytes — the full set of characters that can break or inject into
/// Cypher string literals.
fn escape_cypher(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            _ => out.push(c),
        }
    }
    out
}

impl CognitiveMemoryOps for NativeCognitiveMemory {
    fn record_sensory(
        &self,
        modality: &str,
        raw_data: &str,
        ttl_seconds: u64,
    ) -> SimardResult<String> {
        let id = Self::new_id("sen");
        let expires_at = Self::now_secs()? + ttl_seconds as f64;
        self.execute(&format!(
            "CREATE (s:Sensory {{id: '{}', modality: '{}', raw_data: '{}', observation_order: 0, expires_at: {expires_at}}})",
            escape_cypher(&id),
            escape_cypher(modality),
            escape_cypher(raw_data),
        ))?;
        Ok(id)
    }

    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        let now = Self::now_secs()?;
        let rows = self.query(&format!(
            "MATCH (s:Sensory) WHERE s.expires_at < {now} RETURN count(s)"
        ))?;
        let count = rows
            .first()
            .and_then(|r| r.first())
            .and_then(as_i64)
            .unwrap_or(0) as usize;
        if count > 0 {
            self.execute(&format!(
                "MATCH (s:Sensory) WHERE s.expires_at < {now} DELETE s"
            ))?;
        }
        Ok(count)
    }

    fn push_working(
        &self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> SimardResult<String> {
        let id = Self::new_id("wrk");
        self.execute(&format!(
            "CREATE (w:WorkingMemory {{id: '{}', slot_type: '{}', content: '{}', task_id: '{}', relevance: {relevance}}})",
            escape_cypher(&id),
            escape_cypher(slot_type),
            escape_cypher(content),
            escape_cypher(task_id),
        ))?;
        Ok(id)
    }

    fn get_working(&self, task_id: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        let rows = self.query(&format!(
            "MATCH (w:WorkingMemory) WHERE w.task_id = '{}' RETURN w.id, w.slot_type, w.content, w.relevance, w.task_id",
            escape_cypher(task_id)
        ))?;
        Ok(rows
            .iter()
            .map(|row| CognitiveWorkingSlot {
                node_id: as_str(&row[0]).unwrap_or("").to_string(),
                slot_type: as_str(&row[1]).unwrap_or("").to_string(),
                content: as_str(&row[2]).unwrap_or("").to_string(),
                relevance: as_f64(&row[3]).unwrap_or(0.0),
                task_id: as_str(&row[4]).unwrap_or("").to_string(),
            })
            .collect())
    }

    fn clear_working(&self, task_id: &str) -> SimardResult<usize> {
        let rows = self.query(&format!(
            "MATCH (w:WorkingMemory) WHERE w.task_id = '{}' RETURN count(w)",
            escape_cypher(task_id)
        ))?;
        let count = rows
            .first()
            .and_then(|r| r.first())
            .and_then(as_i64)
            .unwrap_or(0) as usize;
        if count > 0 {
            self.execute(&format!(
                "MATCH (w:WorkingMemory) WHERE w.task_id = '{}' DELETE w",
                escape_cypher(task_id)
            ))?;
        }
        Ok(count)
    }

    fn store_episode(
        &self,
        content: &str,
        source_label: &str,
        _metadata: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        let id = Self::new_id("epi");
        self.execute(&format!(
            "CREATE (e:Episode {{id: '{}', content: '{}', source_label: '{}', temporal_index: 0, compressed: 0}})",
            escape_cypher(&id),
            escape_cypher(content),
            escape_cypher(source_label),
        ))?;
        Ok(id)
    }

    fn consolidate_episodes(&self, batch_size: u32) -> SimardResult<Option<String>> {
        let rows = self.query(&format!(
            "MATCH (e:Episode) WHERE e.compressed = 0 RETURN e.id, e.content ORDER BY e.temporal_index LIMIT {batch_size}"
        ))?;
        if rows.len() < 2 {
            return Ok(None);
        }
        let contents: Vec<&str> = rows.iter().filter_map(|r| as_str(&r[1])).collect();
        let original_count = contents.len();
        let mut seen = std::collections::HashSet::new();
        let unique_contents: Vec<&str> = contents
            .iter()
            .filter(|c| seen.insert(c.trim()))
            .copied()
            .collect();
        let unique_count = unique_contents.len();
        eprintln!(
            "[simard] episode consolidation: {original_count} → {unique_count} (compression ratio {:.1}%)",
            if original_count > 0 {
                (1.0 - unique_count as f64 / original_count as f64) * 100.0
            } else {
                0.0
            }
        );
        let summary = format!(
            "[consolidated {}→{} episodes]: {}",
            original_count,
            unique_count,
            unique_contents.join(" | ")
        );
        let summary_id = Self::new_id("epi");
        self.execute(&format!(
            "CREATE (e:Episode {{id: '{}', content: '{}', source_label: 'consolidation', temporal_index: 0, compressed: 1}})",
            escape_cypher(&summary_id),
            escape_cypher(&summary),
        ))?;
        for row in &rows {
            if let Some(eid) = as_str(&row[0]) {
                self.execute(&format!(
                    "MATCH (e:Episode {{id: '{}'}}) SET e.compressed = 1",
                    escape_cypher(eid)
                ))?;
            }
        }
        Ok(Some(summary_id))
    }

    fn store_fact(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        let id = Self::new_id("sem");
        let tags_str = tags.join(",");
        self.execute(&format!(
            "CREATE (f:Fact {{id: '{}', concept: '{}', content: '{}', confidence: {confidence}, tags: '{}', source_id: '{}'}})",
            escape_cypher(&id),
            escape_cypher(concept),
            escape_cypher(content),
            escape_cypher(&tags_str),
            escape_cypher(source_id),
        ))?;
        Ok(id)
    }

    fn search_facts(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        let q = escape_cypher(query);
        let rows = self.query(&format!(
            "MATCH (f:Fact) WHERE (f.concept CONTAINS '{q}' OR f.content CONTAINS '{q}') AND f.confidence >= {min_confidence} RETURN f.id, f.concept, f.content, f.confidence, f.source_id, f.tags LIMIT {limit}"
        ))?;
        Ok(rows
            .iter()
            .map(|row| {
                let tags_str = as_str(&row[5]).unwrap_or("");
                CognitiveFact {
                    node_id: as_str(&row[0]).unwrap_or("").to_string(),
                    concept: as_str(&row[1]).unwrap_or("").to_string(),
                    content: as_str(&row[2]).unwrap_or("").to_string(),
                    confidence: as_f64(&row[3]).unwrap_or(0.0),
                    source_id: as_str(&row[4]).unwrap_or("").to_string(),
                    tags: if tags_str.is_empty() {
                        vec![]
                    } else {
                        tags_str.split(',').map(|s| s.to_string()).collect()
                    },
                }
            })
            .collect())
    }

    fn store_procedure(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
    ) -> SimardResult<String> {
        let id = Self::new_id("proc");
        let steps_json = serde_json::to_string(steps).unwrap_or_default();
        let prereqs_json = serde_json::to_string(prerequisites).unwrap_or_default();
        self.execute(&format!(
            "CREATE (p:Procedure {{id: '{}', name: '{}', steps: '{}', prerequisites: '{}', usage_count: 0}})",
            escape_cypher(&id),
            escape_cypher(name),
            escape_cypher(&steps_json),
            escape_cypher(&prereqs_json),
        ))?;
        Ok(id)
    }

    fn recall_procedure(&self, query: &str, limit: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        let q = escape_cypher(query);
        let rows = self.query(&format!(
            "MATCH (p:Procedure) WHERE p.name CONTAINS '{q}' OR p.steps CONTAINS '{q}' RETURN p.id, p.name, p.steps, p.prerequisites, p.usage_count LIMIT {limit}"
        ))?;
        Ok(rows
            .iter()
            .map(|row| {
                let steps_str = as_str(&row[2]).unwrap_or("[]");
                let prereqs_str = as_str(&row[3]).unwrap_or("[]");
                CognitiveProcedure {
                    node_id: as_str(&row[0]).unwrap_or("").to_string(),
                    name: as_str(&row[1]).unwrap_or("").to_string(),
                    steps: serde_json::from_str(steps_str).unwrap_or_default(),
                    prerequisites: serde_json::from_str(prereqs_str).unwrap_or_default(),
                    usage_count: as_i64(&row[4]).unwrap_or(0),
                }
            })
            .collect())
    }

    fn store_prospective(
        &self,
        description: &str,
        trigger_condition: &str,
        action_on_trigger: &str,
        priority: i64,
    ) -> SimardResult<String> {
        let id = Self::new_id("pro");
        self.execute(&format!(
            "CREATE (p:Prospective {{id: '{}', description: '{}', trigger_condition: '{}', action_on_trigger: '{}', status: 'pending', priority: {priority}}})",
            escape_cypher(&id),
            escape_cypher(description),
            escape_cypher(trigger_condition),
            escape_cypher(action_on_trigger),
        ))?;
        Ok(id)
    }

    fn check_triggers(&self, content: &str) -> SimardResult<Vec<CognitiveProspective>> {
        let c = escape_cypher(content);
        let rows = self.query(&format!(
            "MATCH (p:Prospective) WHERE p.status = 'pending' AND '{c}' CONTAINS p.trigger_condition RETURN p.id, p.description, p.trigger_condition, p.action_on_trigger, p.status, p.priority"
        ))?;
        Ok(rows
            .iter()
            .map(|row| CognitiveProspective {
                node_id: as_str(&row[0]).unwrap_or("").to_string(),
                description: as_str(&row[1]).unwrap_or("").to_string(),
                trigger_condition: as_str(&row[2]).unwrap_or("").to_string(),
                action_on_trigger: as_str(&row[3]).unwrap_or("").to_string(),
                status: as_str(&row[4]).unwrap_or("pending").to_string(),
                priority: as_i64(&row[5]).unwrap_or(0),
            })
            .collect())
    }

    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        let count_query = |table: &str| -> SimardResult<u64> {
            let rows = self.query(&format!("MATCH (n:{table}) RETURN count(n)"))?;
            Ok(rows
                .first()
                .and_then(|r| r.first())
                .and_then(as_i64)
                .unwrap_or(0) as u64)
        };
        Ok(CognitiveStatistics {
            sensory_count: count_query("Sensory")?,
            working_count: count_query("WorkingMemory")?,
            episodic_count: count_query("Episode")?,
            semantic_count: count_query("Fact")?,
            procedural_count: count_query("Procedure")?,
            prospective_count: count_query("Prospective")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_mem() -> NativeCognitiveMemory {
        NativeCognitiveMemory::in_memory().expect("in-memory DB should create")
    }

    #[test]
    fn open_in_memory_creates_schema() {
        let mem = test_mem();
        let stats = mem.get_statistics().unwrap();
        assert_eq!(stats.total(), 0);
    }

    #[test]
    fn store_and_search_fact() {
        let mem = test_mem();
        let id = mem
            .store_fact("rust", "systems language", 0.9, &[], "test")
            .unwrap();
        assert!(id.starts_with("sem_"));

        let facts = mem.search_facts("rust", 10, 0.0).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "rust");
        assert!((facts[0].confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn search_facts_respects_min_confidence() {
        let mem = test_mem();
        mem.store_fact("low", "low confidence", 0.1, &[], "test")
            .unwrap();
        mem.store_fact("high", "high confidence", 0.9, &[], "test")
            .unwrap();

        let results = mem.search_facts("confidence", 10, 0.5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].concept, "high");
    }

    #[test]
    fn record_and_prune_sensory() {
        let mem = test_mem();
        mem.record_sensory("test", "data", 0).unwrap(); // expires immediately
        let pruned = mem.prune_expired_sensory().unwrap();
        assert!(pruned >= 1);
    }

    #[test]
    fn push_get_clear_working() {
        let mem = test_mem();
        mem.push_working("goal", "build it", "task-1", 1.0).unwrap();
        mem.push_working("context", "extra", "task-1", 0.5).unwrap();

        let slots = mem.get_working("task-1").unwrap();
        assert_eq!(slots.len(), 2);

        let cleared = mem.clear_working("task-1").unwrap();
        assert_eq!(cleared, 2);
        assert!(mem.get_working("task-1").unwrap().is_empty());
    }

    #[test]
    fn store_episode_and_consolidate() {
        let mem = test_mem();
        for i in 0..5 {
            mem.store_episode(&format!("event {i}"), "test", None)
                .unwrap();
        }
        let consolidated = mem.consolidate_episodes(5).unwrap();
        assert!(consolidated.is_some());
        let stats = mem.get_statistics().unwrap();
        // 5 original (now compressed=1) + 1 summary = 6
        assert_eq!(stats.episodic_count, 6);
    }

    #[test]
    fn consolidate_episodes_returns_none_when_insufficient() {
        let mem = test_mem();
        mem.store_episode("only one", "test", None).unwrap();
        assert!(mem.consolidate_episodes(5).unwrap().is_none());
    }

    #[test]
    fn store_and_recall_procedure() {
        let mem = test_mem();
        let steps = vec!["compile".to_string(), "test".to_string()];
        mem.store_procedure("build", &steps, &[]).unwrap();

        let procs = mem.recall_procedure("build", 5).unwrap();
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].name, "build");
        assert_eq!(procs[0].steps, steps);
    }

    #[test]
    fn store_prospective_and_check_triggers() {
        let mem = test_mem();
        mem.store_prospective("watch errors", "error", "alert", 5)
            .unwrap();
        let triggered = mem.check_triggers("an error occurred").unwrap();
        assert_eq!(triggered.len(), 1);
        assert_eq!(triggered[0].description, "watch errors");
    }

    #[test]
    fn check_triggers_ignores_non_matching() {
        let mem = test_mem();
        mem.store_prospective("watch errors", "error", "alert", 5)
            .unwrap();
        let triggered = mem.check_triggers("all good").unwrap();
        assert!(triggered.is_empty());
    }

    #[test]
    fn get_statistics_counts_all_types() {
        let mem = test_mem();
        mem.record_sensory("vis", "img", 300).unwrap();
        mem.push_working("ctx", "data", "t1", 1.0).unwrap();
        mem.store_episode("event", "src", None).unwrap();
        mem.store_fact("f", "fact", 0.5, &[], "").unwrap();
        mem.store_procedure("p", &[], &[]).unwrap();
        mem.store_prospective("desc", "trigger", "action", 1)
            .unwrap();
        let stats = mem.get_statistics().unwrap();
        assert_eq!(stats.total(), 6);
    }

    #[test]
    fn cypher_injection_escaped() {
        let mem = test_mem();
        let result = mem.store_fact("test'DROP", "con'tent", 0.5, &[], "src");
        assert!(result.is_ok(), "single quotes should be escaped");
    }

    #[test]
    fn escape_cypher_handles_special_chars() {
        assert_eq!(escape_cypher("a'b"), "a\\'b");
        assert_eq!(escape_cypher("a\\b"), "a\\\\b");
        assert_eq!(escape_cypher("line\nbreak"), "line\\nbreak");
        assert_eq!(escape_cypher("tab\there"), "tab\\there");
        assert_eq!(escape_cypher("null\0byte"), "null\\0byte");
        assert_eq!(escape_cypher("cr\rreturn"), "cr\\rreturn");
    }

    #[test]
    fn newline_in_content_does_not_break_query() {
        let mem = test_mem();
        let result = mem.store_fact("key", "line1\nline2\ttab", 0.5, &[], "src");
        assert!(result.is_ok(), "newlines and tabs should be safely escaped");
        let facts = mem.search_facts("key", 10, 0.0).unwrap();
        assert_eq!(facts.len(), 1);
    }

    #[test]
    fn disk_persist_facts_survive_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_path_buf();

        {
            let mem = NativeCognitiveMemory::open(&path).unwrap();
            mem.store_fact("rust", "systems language", 0.95, &[], "test")
                .unwrap();
        } // drop closes the DB

        let mem2 = NativeCognitiveMemory::open(&path).unwrap();
        let facts = mem2.search_facts("rust", 10, 0.0).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "rust");
        assert_eq!(facts[0].content, "systems language");
        assert!((facts[0].confidence - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn disk_persist_procedures_survive_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_path_buf();

        {
            let mem = NativeCognitiveMemory::open(&path).unwrap();
            let steps = vec![
                "compile".to_string(),
                "test".to_string(),
                "deploy".to_string(),
            ];
            mem.store_procedure("release", &steps, &[]).unwrap();
        }

        let mem2 = NativeCognitiveMemory::open(&path).unwrap();
        let procs = mem2.recall_procedure("release", 5).unwrap();
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].name, "release");
        assert_eq!(
            procs[0].steps,
            vec![
                "compile".to_string(),
                "test".to_string(),
                "deploy".to_string()
            ]
        );
    }

    #[test]
    fn disk_persist_episodes_and_consolidation_survive_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_path_buf();

        {
            let mem = NativeCognitiveMemory::open(&path).unwrap();
            for i in 0..5 {
                mem.store_episode(&format!("event {i}"), "test", None)
                    .unwrap();
            }
            let consolidated = mem.consolidate_episodes(5).unwrap();
            assert!(consolidated.is_some());
        }

        let mem2 = NativeCognitiveMemory::open(&path).unwrap();
        // Query for the consolidated episode (compressed=1 with source_label='consolidation')
        let rows = mem2
            .query("MATCH (e:Episode) WHERE e.compressed = 1 AND e.source_label = 'consolidation' RETURN e.content")
            .unwrap();
        assert_eq!(rows.len(), 1, "consolidated episode should survive reopen");
        let content = super::as_str(&rows[0][0]).unwrap();
        assert!(
            content.starts_with("[consolidated 5"),
            "consolidated content should start with marker, got: {content}"
        );
    }

    #[test]
    fn consolidate_episodes_deduplicates() {
        let mem = test_mem();
        // Store duplicate episodes
        mem.store_episode("duplicate event", "test", None).unwrap();
        mem.store_episode("duplicate event", "test", None).unwrap();
        mem.store_episode("  duplicate event  ", "test", None)
            .unwrap();
        mem.store_episode("unique event", "test", None).unwrap();

        let consolidated = mem.consolidate_episodes(10).unwrap();
        assert!(consolidated.is_some());

        let rows = mem
            .query("MATCH (e:Episode) WHERE e.compressed = 1 AND e.source_label = 'consolidation' RETURN e.content")
            .unwrap();
        assert_eq!(rows.len(), 1);
        let content = super::as_str(&rows[0][0]).unwrap();
        // 4 original → 2 unique
        assert!(
            content.contains("4→2"),
            "should show dedup ratio, got: {content}"
        );
    }
}
