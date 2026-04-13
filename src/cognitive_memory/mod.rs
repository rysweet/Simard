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

        let db = Self::with_open_lock(&db_path, || {
            lbug::Database::new(&db_path, lbug::SystemConfig::default()).map_err(|e| {
                SimardError::RuntimeInitFailed {
                    component: "cognitive-memory".into(),
                    reason: format!("Failed to open LadybugDB at {}: {e}", db_path.display()),
                }
            })
        })?;
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
        let summary = format!(
            "[consolidated {} episodes]: {}",
            contents.len(),
            contents.join(" | ")
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
}
