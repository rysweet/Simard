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

#[cfg(test)]
mod tests_mod;
pub(crate) use ops::escape_cypher;
