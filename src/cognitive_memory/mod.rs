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

    /// Search recent episodes by content prefix.
    ///
    /// Returns `(content, recorded_at)` pairs for episodes whose
    /// `content` starts with `prefix`, ordered most-recent first, capped
    /// at `limit`. Used by the progress-evidence gate
    /// (`update_goal_progress_with_evidence`) to source the `since`
    /// timestamp for goals that have no
    /// `ActiveGoal.last_progress_update_at` field set yet (legacy
    /// on-disk boards from before #1967).
    ///
    /// Default impl returns `Ok(vec![])` — backends without temporal
    /// metadata simply force callers into the next fallback step (the
    /// daemon's process-start timestamp), which is safe.
    fn search_episodes_starting_with(
        &self,
        _prefix: &str,
        _limit: u32,
    ) -> SimardResult<Vec<(String, chrono::DateTime<chrono::Utc>)>> {
        Ok(vec![])
    }

    /// Reports whether this backend was opened in read-only mode.
    ///
    /// Defaulted to `false` because the overwhelming majority of
    /// implementations are writers (the IPC client, the daemon's
    /// in-process Arc, the live `NativeCognitiveMemory::open`).
    /// `NativeCognitiveMemory::open_read_only` overrides this to `true`.
    ///
    /// `WriterBridge` constructors assert that this is `false` so a
    /// read-only handle cannot be silently wrapped as a writer — the
    /// "hollow success" failure mode that issue #1590's follow-up
    /// targets.
    fn is_read_only(&self) -> bool {
        false
    }

    /// Force a WAL checkpoint, collapsing the WAL into the main DB file.
    ///
    /// Defaults to a no-op for backends where this is not meaningful
    /// (IPC client, bridge clients). Overridden by [`NativeCognitiveMemory`]
    /// to issue a `CHECKPOINT;` Cypher statement.
    ///
    /// Call this **before** taking a backup or shutting down the host
    /// process so committed-but-WAL-resident writes are captured (issue #1631).
    fn checkpoint(&self) -> SimardResult<()> {
        Ok(())
    }
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
    /// Whether this handle was created via [`Self::open_read_only`].
    /// Surfaced through [`CognitiveMemoryOps::is_read_only`] so the
    /// `WriterBridge` defensive guard refuses to wrap a read-only
    /// handle (issue #1590 follow-up — closes the dashboard hollow-
    /// success failure mode).
    read_only: bool,
    /// Whether mutating ops must call [`Self::post_write_barrier`] after
    /// every successful Cypher write. `true` for [`Self::open`] (on-disk
    /// writer), `false` for [`Self::in_memory`] (no on-disk file to fsync)
    /// and for [`Self::open_read_only`] (writes aren't possible).
    ///
    /// Per-write fsync barrier: issue #1973, goals G1 + G2 of epic
    /// #1972 (improve-cognitive-memory-persistence). Without the barrier,
    /// SIGKILL between two writes loses acknowledged data because lbug
    /// only flushes its WAL on `Database::drop`.
    durable_writes: bool,
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
            read_only: false,
            durable_writes: true,
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
            read_only: false,
            // In-memory handles back a temp file whose lifetime is tied to
            // the process — there is no recovery scenario where fsyncing it
            // would help, and unit tests rely on the latency profile of an
            // un-fsynced backend. Issue #1973 opt-out.
            durable_writes: false,
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
            read_only: true,
            // Read-only handles cannot mutate, so the barrier is a no-op
            // and we save the cost of even checking the flag inside hot
            // read paths that happen to call into shared code.
            durable_writes: false,
        };
        eprintln!(
            "[simard] cognitive memory opened read-only — LadybugDB at {}",
            state_root.display()
        );
        Ok(mem)
    }
    // Backup/recovery helpers — see backup.rs.
}

mod backup;
mod ops;

impl NativeCognitiveMemory {
    fn conn(&self) -> SimardResult<lbug::Connection<'_>> {
        lbug::Connection::new(&self.db).map_err(|e| SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: format!("Failed to create LadybugDB connection: {e}"),
        })
    }

    pub(crate) fn query(&self, cypher: &str) -> SimardResult<Vec<Vec<lbug::Value>>> {
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

    /// Force a WAL checkpoint, collapsing the WAL into the main DB file.
    ///
    /// Call this **before** copying the DB file (e.g. for a snapshot or
    /// backup) so committed-but-WAL-resident writes are captured. Also
    /// useful in shutdown paths where the host process is about to exit
    /// without a clean `Database::drop` (issue #1631).
    ///
    /// Idempotent and safe to call concurrently with reads. Issues a
    /// `CHECKPOINT;` Cypher statement over a fresh connection. Read-only
    /// handles are a no-op (returns `Ok(())`).
    pub fn checkpoint(&self) -> SimardResult<()> {
        if self.read_only {
            return Ok(());
        }
        // CHECKPOINT is the lbug/Kuzu Cypher statement that flushes the WAL.
        // Some lbug versions may parse it as `CALL CHECKPOINT()`; try both
        // before propagating an error so the caller doesn't have to know.
        let conn = self.conn()?;
        match conn.query("CHECKPOINT;") {
            Ok(_) => Ok(()),
            Err(first_err) => match conn.query("CALL CHECKPOINT();") {
                Ok(_) => Ok(()),
                Err(_) => Err(SimardError::BridgeCallFailed {
                    bridge: "cognitive-memory-native".into(),
                    method: "checkpoint".into(),
                    reason: format!("CHECKPOINT not accepted by lbug: {first_err}"),
                }),
            },
        }
    }

    /// Per-write fsync barrier (issue #1973, goals G1 + G2 of epic #1972).
    ///
    /// Runs after every successful Cypher mutation on an on-disk
    /// `NativeCognitiveMemory` so that, by the time the calling `Ok(())`
    /// returns to the caller, the written bytes are on stable storage
    /// and the directory entry for the data file is durable.
    ///
    /// Pipeline (Unix; the only platform supported for on-disk writers):
    /// 1. Flush the lbug WAL into the main DB file via `checkpoint()`.
    /// 2. `sync_all()` the data file so kernel page cache is flushed.
    /// 3. `sync_all()` the parent directory so the dirent itself is
    ///    durable on ext4/xfs/btrfs/apfs after any preceding rename.
    ///
    /// No-op when `durable_writes` is `false` (in-memory backend used by
    /// the unit-test suite; read-only handles).
    ///
    /// Errors map to existing typed variants — no new error variants
    /// were introduced for this feature:
    /// - `checkpoint()` failure → `SimardError::BridgeCallFailed` with
    ///   `method = "post-write-checkpoint"`.
    /// - data-file fsync failure → `SimardError::PersistentStoreIo` with
    ///   `action = "fsync-data-file"`.
    /// - parent-dir fsync failure → `SimardError::PersistentStoreIo` with
    ///   `action = "fsync-parent-dir"`.
    ///
    /// The `op` argument is a static string identifying the calling
    /// mutating op (e.g. `"store_fact"`) and is woven into error
    /// `reason` strings so a fsync failure can be attributed in logs.
    pub(crate) fn post_write_barrier(&self, op: &'static str) -> SimardResult<()> {
        if !self.durable_writes {
            return Ok(());
        }

        // Step 1: flush WAL into main DB file.
        self.checkpoint()
            .map_err(|e| SimardError::BridgeCallFailed {
                bridge: "cognitive-memory-native".into(),
                method: "post-write-checkpoint".into(),
                reason: format!("op={op}: {e}"),
            })?;

        // Step 2: fsync the data file. Open read-only — lbug owns the
        // exclusive writer fd; we only need a separate fd to issue
        // sync_all(2) on the underlying inode.
        let data_file = std::fs::OpenOptions::new()
            .read(true)
            .open(&self.path)
            .map_err(|e| SimardError::PersistentStoreIo {
                store: "cognitive-memory".into(),
                action: "fsync-data-file-open".into(),
                path: self.path.clone(),
                reason: format!("op={op}: {e}"),
            })?;
        data_file
            .sync_all()
            .map_err(|e| SimardError::PersistentStoreIo {
                store: "cognitive-memory".into(),
                action: "fsync-data-file".into(),
                path: self.path.clone(),
                reason: format!("op={op}: {e}"),
            })?;

        // Step 3: fsync the parent directory so the dirent for `self.path`
        // (and any rename'd .wal sibling that lbug published during the
        // checkpoint) is itself crash-durable.
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let dir = std::fs::File::open(parent).map_err(|e| SimardError::PersistentStoreIo {
            store: "cognitive-memory".into(),
            action: "fsync-parent-dir-open".into(),
            path: parent.to_path_buf(),
            reason: format!("op={op}: {e}"),
        })?;
        dir.sync_all().map_err(|e| SimardError::PersistentStoreIo {
            store: "cognitive-memory".into(),
            action: "fsync-parent-dir".into(),
            path: parent.to_path_buf(),
            reason: format!("op={op}: {e}"),
        })?;

        Ok(())
    }
}

pub(crate) fn as_str(val: &lbug::Value) -> Option<&str> {
    match val {
        lbug::Value::String(s) => Some(s.as_str()),
        _ => None,
    }
}

pub(crate) fn as_i64(val: &lbug::Value) -> Option<i64> {
    match val {
        lbug::Value::Int64(n) => Some(*n),
        _ => None,
    }
}

pub(crate) fn as_f64(val: &lbug::Value) -> Option<f64> {
    match val {
        lbug::Value::Double(d) => Some(*d),
        lbug::Value::Int64(n) => Some(*n as f64),
        _ => None,
    }
}

// re-exported for cfg(test) consumers in cognitive_memory/tests_mod.rs (false-positive of clippy unused_imports on lib pass — see #1405)
#[allow(unused_imports)]
pub(crate) use ops::escape_cypher;

#[cfg(test)]
mod tests_mod;

#[cfg(test)]
mod tests_lock_vs_corruption_1967;
