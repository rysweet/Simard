//! Native cognitive memory backed by LadybugDB.
//!
//! Replaces the Python bridge (`CognitiveMemoryBridge`) with a direct
//! Rust wrapper around `lbug::Database`. Uses flock serialization for
//! multi-writer safety (matching the skwaq pattern).

mod schema;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

/// Canonical filename for the LadybugDB cognitive-memory database.
const COGNITIVE_MEMORY_DB: &str = "cognitive_memory.ladybug";

/// Return the canonical path to the LadybugDB cognitive-memory database.
pub fn cognitive_memory_db_path(state_root: &Path) -> PathBuf {
    state_root.join(COGNITIVE_MEMORY_DB)
}

// ============================================================================
// Trait
// ============================================================================

/// Backend-agnostic cognitive memory operations.
///
/// Implemented by both [`NativeCognitiveMemory`] (LadybugDB, in-process) and
/// [`CognitiveMemoryBridge`](crate::memory_bridge::CognitiveMemoryBridge)
/// (Python subprocess) so that callers are backend-agnostic.
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
// Native implementation
// ============================================================================

/// Native cognitive memory store backed by LadybugDB.
///
/// Wraps an `Arc<lbug::Database>` and provides the same operations as the
/// Python bridge, but runs entirely in-process. Uses flock serialization
/// on `<db_path>.open.lock` to prevent concurrent buffer-pool initialization
/// races across processes.
pub struct NativeCognitiveMemory {
    db: Arc<lbug::Database>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl NativeCognitiveMemory {
    /// Open or create a LadybugDB cognitive memory database.
    ///
    /// The `state_root` is the Simard state directory; the database is stored
    /// at `<state_root>/cognitive_memory.ladybug/`.
    pub fn open(state_root: &Path) -> SimardResult<Self> {
        let db_path = cognitive_memory_db_path(state_root);
        std::fs::create_dir_all(&db_path).map_err(|e| SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: format!("failed to create DB dir {}: {e}", db_path.display()),
        })?;

        let db = Self::with_open_lock(&db_path, || {
            lbug::Database::new(&db_path, lbug::SystemConfig::default()).map_err(|e| {
                SimardError::RuntimeInitFailed {
                    component: "cognitive-memory".into(),
                    reason: format!("failed to open LadybugDB: {e}"),
                }
            })
        })?;

        let mem = Self {
            db: Arc::new(db),
            path: db_path,
        };
        mem.ensure_schema()?;
        eprintln!("[simard] native cognitive memory: schema initialized");
        Ok(mem)
    }

    /// Create a temporary in-memory database (for tests).
    #[cfg(test)]
    pub fn in_memory() -> SimardResult<Self> {
        let tmp = tempfile::tempdir().map_err(|e| SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: format!("failed to create temp dir: {e}"),
        })?;
        let db_path = tmp.path().join("cognitive_test");
        let db = lbug::Database::new(
            &db_path,
            lbug::SystemConfig::default()
                .buffer_pool_size(64 * 1024 * 1024)
                .max_db_size(1 << 28)
                .max_num_threads(1),
        )
        .map_err(|e| SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: format!("failed to open test LadybugDB: {e}"),
        })?;

        let mem = Self {
            db: Arc::new(db),
            path: db_path,
        };
        mem.ensure_schema()?;
        // Keep the tempdir alive for the lifetime of the test.
        std::mem::forget(tmp);
        Ok(mem)
    }

    /// Acquire an exclusive flock on `<db_path>.open.lock`, call `f()`, then
    /// release. Serializes `Database::new()` across processes.
    #[cfg(unix)]
    fn with_open_lock<T>(db_path: &Path, f: impl FnOnce() -> SimardResult<T>) -> SimardResult<T> {
        use std::fs::File;
        use std::os::unix::io::AsRawFd;

        let lock_path = db_path.with_extension("open.lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SimardError::RuntimeInitFailed {
                component: "cognitive-memory".into(),
                reason: format!("failed to create lock parent dir {}: {e}", parent.display()),
            })?;
        }
        let lock_file = File::create(&lock_path).map_err(|e| SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: format!("failed to create lock file {}: {e}", lock_path.display()),
        })?;
        let fd = lock_file.as_raw_fd();

        // SAFETY: flock is safe for file descriptors we own.
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            return Err(SimardError::RuntimeInitFailed {
                component: "cognitive-memory".into(),
                reason: format!("flock(LOCK_EX) failed on {}: {err}", lock_path.display()),
            });
        }

        let result = f();

        unsafe { libc::flock(fd, libc::LOCK_UN) };
        drop(lock_file);

        result
    }

    /// Non-Unix fallback: no flock serialization available.
    #[cfg(not(unix))]
    fn with_open_lock<T>(_db_path: &Path, f: impl FnOnce() -> SimardResult<T>) -> SimardResult<T> {
        f()
    }

    fn conn(&self) -> SimardResult<lbug::Connection<'_>> {
        lbug::Connection::new(&self.db).map_err(|e| SimardError::RuntimeInitFailed {
            component: "cognitive-memory".into(),
            reason: format!("failed to create connection: {e}"),
        })
    }

    fn execute(&self, cypher: &str) -> SimardResult<()> {
        let conn = self.conn()?;
        conn.query(cypher)
            .map_err(|e| SimardError::BridgeCallFailed {
                bridge: "cognitive-memory-native".into(),
                method: "execute".into(),
                reason: format!("{e}\nCypher: {cypher}"),
            })?;
        Ok(())
    }

    #[allow(dead_code)]
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

    fn ensure_schema(&self) -> SimardResult<()> {
        for ddl in schema::DDL_STATEMENTS {
            if let Err(e) = self.execute(ddl) {
                let msg = format!("{e}");
                if !msg.contains("already exists") {
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    fn new_node_id(prefix: &str) -> String {
        format!("{prefix}_{}", uuid::Uuid::now_v7())
    }
}

/// Escape a string value for embedding in a Cypher single-quoted literal.
fn escape_cypher(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

impl CognitiveMemoryOps for NativeCognitiveMemory {
    fn record_sensory(
        &self,
        modality: &str,
        raw_data: &str,
        ttl_seconds: u64,
    ) -> SimardResult<String> {
        let node_id = Self::new_node_id("sen");
        let now = chrono::Utc::now().timestamp();
        let expires_at = now + ttl_seconds as i64;
        self.execute(&format!(
            "CREATE (n:SensoryMemory {{node_id: '{id}', modality: '{m}', raw_data: '{d}', \
             observation_order: {now}, expires_at: {expires_at}}})",
            id = escape_cypher(&node_id),
            m = escape_cypher(modality),
            d = escape_cypher(raw_data),
        ))?;
        Ok(node_id)
    }

    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        let now = chrono::Utc::now().timestamp();
        // LadybugDB does not support DELETE with RETURN count in all versions,
        // so we count first, then delete.
        let rows = self.query(&format!(
            "MATCH (n:SensoryMemory) WHERE n.expires_at > 0 AND n.expires_at < {now} RETURN n.node_id"
        ))?;
        let count = rows.len();
        if count > 0 {
            self.execute(&format!(
                "MATCH (n:SensoryMemory) WHERE n.expires_at > 0 AND n.expires_at < {now} DELETE n"
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
        let node_id = Self::new_node_id("wrk");
        self.execute(&format!(
            "CREATE (n:WorkingMemory {{node_id: '{id}', slot_type: '{st}', content: '{c}', \
             task_id: '{t}', relevance: {r}}})",
            id = escape_cypher(&node_id),
            st = escape_cypher(slot_type),
            c = escape_cypher(content),
            t = escape_cypher(task_id),
            r = relevance,
        ))?;
        Ok(node_id)
    }

    fn get_working(&self, task_id: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        let rows = self.query(&format!(
            "MATCH (n:WorkingMemory) WHERE n.task_id = '{t}' RETURN n.node_id, n.slot_type, n.content, n.relevance, n.task_id",
            t = escape_cypher(task_id),
        ))?;
        let mut slots = Vec::with_capacity(rows.len());
        for row in &rows {
            slots.push(CognitiveWorkingSlot {
                node_id: value_as_string(&row[0]),
                slot_type: value_as_string(&row[1]),
                content: value_as_string(&row[2]),
                relevance: value_as_f64(&row[3]),
                task_id: value_as_string(&row[4]),
            });
        }
        Ok(slots)
    }

    fn clear_working(&self, task_id: &str) -> SimardResult<usize> {
        let rows = self.query(&format!(
            "MATCH (n:WorkingMemory) WHERE n.task_id = '{t}' RETURN n.node_id",
            t = escape_cypher(task_id),
        ))?;
        let count = rows.len();
        if count > 0 {
            self.execute(&format!(
                "MATCH (n:WorkingMemory) WHERE n.task_id = '{t}' DELETE n",
                t = escape_cypher(task_id),
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
        let node_id = Self::new_node_id("epi");
        let now = chrono::Utc::now().timestamp();
        self.execute(&format!(
            "CREATE (n:EpisodicMemory {{node_id: '{id}', content: '{c}', \
             source_label: '{s}', temporal_index: {now}, compressed: 0}})",
            id = escape_cypher(&node_id),
            c = escape_cypher(content),
            s = escape_cypher(source_label),
        ))?;
        Ok(node_id)
    }

    fn consolidate_episodes(&self, batch_size: u32) -> SimardResult<Option<String>> {
        let rows = self.query(&format!(
            "MATCH (n:EpisodicMemory) WHERE n.compressed = 0 \
             RETURN n.node_id, n.content ORDER BY n.temporal_index LIMIT {batch_size}"
        ))?;
        if rows.len() < 2 {
            return Ok(None);
        }
        let summaries: Vec<String> = rows.iter().map(|r| value_as_string(&r[1])).collect();
        let episode_ids: Vec<String> = rows.iter().map(|r| value_as_string(&r[0])).collect();
        let summary = format!("Consolidated {} episodes", summaries.len());
        let node_id = Self::new_node_id("con");

        self.execute(&format!(
            "CREATE (n:ConsolidatedEpisode {{node_id: '{id}', summary: '{s}', \
             original_count: {c}}})",
            id = escape_cypher(&node_id),
            s = escape_cypher(&summary),
            c = episode_ids.len(),
        ))?;

        // Mark originals as compressed.
        for eid in &episode_ids {
            let _ = self.execute(&format!(
                "MATCH (n:EpisodicMemory) WHERE n.node_id = '{id}' SET n.compressed = 1",
                id = escape_cypher(eid),
            ));
        }

        Ok(Some(node_id))
    }

    fn store_fact(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        let node_id = Self::new_node_id("sem");
        let tags_str = tags.join(",");
        self.execute(&format!(
            "CREATE (n:SemanticMemory {{node_id: '{id}', concept: '{co}', content: '{ct}', \
             confidence: {cf}, source_id: '{si}', tags: '{tg}'}})",
            id = escape_cypher(&node_id),
            co = escape_cypher(concept),
            ct = escape_cypher(content),
            cf = confidence,
            si = escape_cypher(source_id),
            tg = escape_cypher(&tags_str),
        ))?;
        Ok(node_id)
    }

    fn search_facts(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        let rows = self.query(&format!(
            "MATCH (n:SemanticMemory) WHERE n.confidence >= {min_confidence} \
             AND contains(n.concept, '{q}') \
             RETURN n.node_id, n.concept, n.content, n.confidence, n.source_id, n.tags \
             LIMIT {limit}",
            q = escape_cypher(query),
        ))?;
        let mut facts = Vec::with_capacity(rows.len());
        for row in &rows {
            facts.push(CognitiveFact {
                node_id: value_as_string(&row[0]),
                concept: value_as_string(&row[1]),
                content: value_as_string(&row[2]),
                confidence: value_as_f64(&row[3]),
                source_id: value_as_string(&row[4]),
                tags: value_as_string(&row[5])
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect(),
            });
        }
        Ok(facts)
    }

    fn store_procedure(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
    ) -> SimardResult<String> {
        let node_id = Self::new_node_id("proc");
        let steps_str = serde_json::to_string(steps).unwrap_or_default();
        let prereqs_str = serde_json::to_string(prerequisites).unwrap_or_default();
        self.execute(&format!(
            "CREATE (n:ProceduralMemory {{node_id: '{id}', name: '{n}', steps: '{s}', \
             prerequisites: '{p}', usage_count: 0}})",
            id = escape_cypher(&node_id),
            n = escape_cypher(name),
            s = escape_cypher(&steps_str),
            p = escape_cypher(&prereqs_str),
        ))?;
        Ok(node_id)
    }

    fn recall_procedure(&self, query: &str, limit: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        let rows = self.query(&format!(
            "MATCH (n:ProceduralMemory) WHERE contains(n.name, '{q}') \
             RETURN n.node_id, n.name, n.steps, n.prerequisites, n.usage_count \
             LIMIT {limit}",
            q = escape_cypher(query),
        ))?;
        let mut procs = Vec::with_capacity(rows.len());
        for row in &rows {
            let steps_json = value_as_string(&row[2]);
            let prereqs_json = value_as_string(&row[3]);
            procs.push(CognitiveProcedure {
                node_id: value_as_string(&row[0]),
                name: value_as_string(&row[1]),
                steps: serde_json::from_str(&steps_json).unwrap_or_default(),
                prerequisites: serde_json::from_str(&prereqs_json).unwrap_or_default(),
                usage_count: value_as_i64(&row[4]),
            });
        }
        Ok(procs)
    }

    fn store_prospective(
        &self,
        description: &str,
        trigger_condition: &str,
        action_on_trigger: &str,
        priority: i64,
    ) -> SimardResult<String> {
        let node_id = Self::new_node_id("pro");
        self.execute(&format!(
            "CREATE (n:ProspectiveMemory {{node_id: '{id}', desc_text: '{d}', \
             trigger_condition: '{tc}', action_on_trigger: '{a}', \
             status: 'pending', priority: {p}}})",
            id = escape_cypher(&node_id),
            d = escape_cypher(description),
            tc = escape_cypher(trigger_condition),
            a = escape_cypher(action_on_trigger),
            p = priority,
        ))?;
        Ok(node_id)
    }

    fn check_triggers(&self, content: &str) -> SimardResult<Vec<CognitiveProspective>> {
        let rows = self.query(&format!(
            "MATCH (n:ProspectiveMemory) WHERE n.status = 'pending' \
             AND contains('{c}', n.trigger_condition) \
             RETURN n.node_id, n.desc_text, n.trigger_condition, n.action_on_trigger, \
             n.status, n.priority",
            c = escape_cypher(content),
        ))?;
        let mut prospectives = Vec::with_capacity(rows.len());
        for row in &rows {
            prospectives.push(CognitiveProspective {
                node_id: value_as_string(&row[0]),
                description: value_as_string(&row[1]),
                trigger_condition: value_as_string(&row[2]),
                action_on_trigger: value_as_string(&row[3]),
                status: value_as_string(&row[4]),
                priority: value_as_i64(&row[5]),
            });
        }
        Ok(prospectives)
    }

    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        let count = |table: &str| -> usize {
            self.query(&format!("MATCH (n:{table}) RETURN count(n)"))
                .ok()
                .and_then(|rows| rows.first().and_then(|r| r.first().cloned()))
                .and_then(|v| match v {
                    lbug::Value::Int64(n) => Some(n as usize),
                    _ => None,
                })
                .unwrap_or(0)
        };
        Ok(CognitiveStatistics {
            sensory_count: count("SensoryMemory") as u64,
            working_count: count("WorkingMemory") as u64,
            episodic_count: count("EpisodicMemory") as u64,
            semantic_count: count("SemanticMemory") as u64,
            procedural_count: count("ProceduralMemory") as u64,
            prospective_count: count("ProspectiveMemory") as u64,
        })
    }
}

// ── Value extraction helpers ──

fn value_as_string(val: &lbug::Value) -> String {
    match val {
        lbug::Value::String(s) => s.clone(),
        lbug::Value::Int64(n) => n.to_string(),
        lbug::Value::Double(d) => d.to_string(),
        _ => String::new(),
    }
}

fn value_as_f64(val: &lbug::Value) -> f64 {
    match val {
        lbug::Value::Double(d) => *d,
        lbug::Value::Int64(n) => *n as f64,
        _ => 0.0,
    }
}

fn value_as_i64(val: &lbug::Value) -> i64 {
    match val {
        lbug::Value::Int64(n) => *n,
        lbug::Value::Double(d) => *d as i64,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cognitive_memory_db_path_joins_correctly() {
        let path = cognitive_memory_db_path(std::path::Path::new("/state"));
        assert_eq!(path, PathBuf::from("/state/cognitive_memory.ladybug"));
    }

    #[test]
    fn escape_cypher_handles_quotes() {
        assert_eq!(escape_cypher("it's"), "it\\'s");
    }

    #[test]
    fn escape_cypher_handles_backslash() {
        assert_eq!(escape_cypher("a\\b"), "a\\\\b");
    }

    #[test]
    fn in_memory_opens_and_creates_schema() {
        let mem = NativeCognitiveMemory::in_memory().expect("in_memory should succeed");
        let stats = mem.get_statistics().expect("stats should work");
        assert_eq!(stats.total(), 0);
    }

    #[test]
    fn record_and_get_sensory() {
        let mem = NativeCognitiveMemory::in_memory().unwrap();
        let id = mem.record_sensory("objective", "test goal", 300).unwrap();
        assert!(id.starts_with("sen_"));
        let stats = mem.get_statistics().unwrap();
        assert_eq!(stats.sensory_count, 1);
    }

    #[test]
    fn push_and_clear_working() {
        let mem = NativeCognitiveMemory::in_memory().unwrap();
        mem.push_working("goal", "do something", "task-1", 1.0)
            .unwrap();
        let slots = mem.get_working("task-1").unwrap();
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].slot_type, "goal");

        let cleared = mem.clear_working("task-1").unwrap();
        assert_eq!(cleared, 1);
        assert!(mem.get_working("task-1").unwrap().is_empty());
    }

    #[test]
    fn store_and_search_fact() {
        let mem = NativeCognitiveMemory::in_memory().unwrap();
        mem.store_fact("rust", "systems language", 0.9, &[], "test")
            .unwrap();
        let facts = mem.search_facts("rust", 10, 0.0).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "rust");
        assert_eq!(facts[0].content, "systems language");
    }

    #[test]
    fn store_episode_and_consolidate() {
        let mem = NativeCognitiveMemory::in_memory().unwrap();
        for i in 0..5 {
            mem.store_episode(&format!("event {i}"), "test", None)
                .unwrap();
        }
        let stats = mem.get_statistics().unwrap();
        assert_eq!(stats.episodic_count, 5);

        let consolidated = mem.consolidate_episodes(5).unwrap();
        assert!(consolidated.is_some());
    }

    #[test]
    fn store_and_recall_procedure() {
        let mem = NativeCognitiveMemory::in_memory().unwrap();
        mem.store_procedure("build", &["compile".into(), "test".into()], &[])
            .unwrap();
        let procs = mem.recall_procedure("build", 5).unwrap();
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].name, "build");
    }

    #[test]
    fn prospective_store_and_trigger() {
        let mem = NativeCognitiveMemory::in_memory().unwrap();
        mem.store_prospective("run gym after improve", "improve_done", "run_gym", 1)
            .unwrap();
        let triggered = mem.check_triggers("improve_done happened").unwrap();
        assert_eq!(triggered.len(), 1);
        assert_eq!(triggered[0].action_on_trigger, "run_gym");
    }
}
