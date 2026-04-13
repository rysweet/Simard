//! Memory snapshot replication for remote agent sessions.
//!
//! # Deprecated
//!
//! This module's JSON-snapshot replication approach is superseded by the
//! amplihack hive-mind DHT+bloom gossip protocol. The memory bridge now
//! uses `Memory('simard', topology='distributed')` which handles cross-agent
//! replication automatically via the `DistributedHiveGraph`.
//!
//! Prefer the hive-mind approach for new code. This module is retained for
//! backward compatibility with existing snapshot files and one-shot migration
//! scenarios where the hive network is unavailable.
//!
//! ## Original design
//!
//! When an agent migrates to a remote VM, it needs to carry its cognitive
//! memory state. This module exports facts and procedures from a local
//! `CognitiveMemoryBridge`, serializes them into a `MemorySnapshot`, and
//! can import that snapshot into a remote bridge.
//!
//! Only facts and procedures are replicated. Sensory and working memory
//! are ephemeral and session-local. Episodes are too large for migration
//! and can be re-derived from facts. Prospective memories are local triggers.

use std::fmt::{self, Display, Formatter};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{CognitiveFact, CognitiveProcedure};

/// Maximum number of facts to export in a single snapshot.
const MAX_EXPORT_FACTS: u32 = 1000;

/// Maximum number of procedures to export in a single snapshot.
const MAX_EXPORT_PROCEDURES: u32 = 200;

/// A portable snapshot of cognitive memory for replication.
///
/// Contains the subset of memory types that are worth migrating:
/// semantic facts (durable knowledge) and procedures (reusable workflows).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemorySnapshot {
    /// Semantic facts exported from the source bridge.
    pub facts: Vec<CognitiveFact>,
    /// Procedural memories exported from the source bridge.
    pub procedures: Vec<CognitiveProcedure>,
    /// Unix epoch seconds when this snapshot was created.
    pub exported_at: u64,
    /// The agent name that produced this snapshot.
    pub source_agent: String,
}

impl Display for MemorySnapshot {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MemorySnapshot(facts={}, procedures={}, agent={}, at={})",
            self.facts.len(),
            self.procedures.len(),
            self.source_agent,
            self.exported_at
        )
    }
}

impl MemorySnapshot {
    /// Total number of items in the snapshot.
    pub fn total_items(&self) -> usize {
        self.facts.len() + self.procedures.len()
    }

    /// Whether the snapshot is empty (no facts or procedures).
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty() && self.procedures.is_empty()
    }
}

/// Export a memory snapshot from a cognitive memory bridge.
///
/// # Deprecated
/// Use the hive-mind distributed topology instead. The memory bridge now
/// replicates facts automatically via DHT+bloom gossip.
#[deprecated(
    since = "0.13.0",
    note = "Use Memory('simard', topology='distributed') hive-mind replication instead of JSON snapshots"
)]
pub fn export_memory_snapshot(
    bridge: &dyn CognitiveMemoryOps,
    agent_name: &str,
    path: Option<&Path>,
) -> SimardResult<MemorySnapshot> {
    if agent_name.is_empty() {
        return Err(SimardError::InvalidConfigValue {
            key: "agent_name".to_string(),
            value: String::new(),
            help: "agent name cannot be empty for memory export".to_string(),
        });
    }

    // Query all facts with minimum confidence threshold of 0.0 to get everything.
    let facts = bridge.search_facts("*", MAX_EXPORT_FACTS, 0.0)?;
    let procedures = bridge.recall_procedure("*", MAX_EXPORT_PROCEDURES)?;

    let now = current_epoch_seconds()?;

    let snapshot = MemorySnapshot {
        facts,
        procedures,
        exported_at: now,
        source_agent: agent_name.to_string(),
    };

    if let Some(path) = path {
        let json = serde_json::to_string_pretty(&snapshot).map_err(|e| {
            SimardError::PersistentStoreIo {
                store: "memory-snapshot".to_string(),
                action: "serialize".to_string(),
                path: path.to_path_buf(),
                reason: e.to_string(),
            }
        })?;
        std::fs::write(path, json).map_err(|e| SimardError::PersistentStoreIo {
            store: "memory-snapshot".to_string(),
            action: "write".to_string(),
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
    }

    Ok(snapshot)
}

/// Import a memory snapshot into a cognitive memory bridge.
///
/// # Deprecated
/// Use the hive-mind distributed topology instead.
#[deprecated(
    since = "0.13.0",
    note = "Use Memory('simard', topology='distributed') hive-mind replication instead of JSON snapshots"
)]
pub fn import_memory_snapshot(
    bridge: &dyn CognitiveMemoryOps,
    snapshot: &MemorySnapshot,
) -> SimardResult<usize> {
    let mut imported = 0;

    for fact in &snapshot.facts {
        bridge.store_fact(
            &fact.concept,
            &fact.content,
            fact.confidence,
            &fact.tags,
            &fact.source_id,
        )?;
        imported += 1;
    }

    for proc in &snapshot.procedures {
        bridge.store_procedure(&proc.name, &proc.steps, &proc.prerequisites)?;
        imported += 1;
    }

    Ok(imported)
}

/// Load a memory snapshot from a JSON file on disk.
///
/// # Deprecated
/// Use the hive-mind distributed topology instead.
#[deprecated(
    since = "0.13.0",
    note = "Use Memory('simard', topology='distributed') hive-mind replication instead of JSON snapshots"
)]
pub fn load_snapshot_from_file(path: &Path) -> SimardResult<MemorySnapshot> {
    let content = std::fs::read_to_string(path).map_err(|e| SimardError::PersistentStoreIo {
        store: "memory-snapshot".to_string(),
        action: "read".to_string(),
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    serde_json::from_str(&content).map_err(|e| SimardError::PersistentStoreIo {
        store: "memory-snapshot".to_string(),
        action: "deserialize".to_string(),
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

fn current_epoch_seconds() -> SimardResult<u64> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| {
        SimardError::ClockBeforeUnixEpoch {
            reason: e.to_string(),
        }
    })?;
    Ok(duration.as_secs())
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;
    use serde_json::json;
    use std::sync::Mutex;

    struct MockStore {
        facts: Vec<CognitiveFact>,
        procedures: Vec<CognitiveProcedure>,
    }

    fn mock_bridge() -> CognitiveMemoryBridge {
        let store: &'static Mutex<MockStore> = Box::leak(Box::new(Mutex::new(MockStore {
            facts: vec![],
            procedures: vec![],
        })));

        let transport =
            InMemoryBridgeTransport::new("test-memory", move |method, params| match method {
                "memory.search_facts" => {
                    let s = store.lock().unwrap();
                    let facts: Vec<serde_json::Value> = s
                        .facts
                        .iter()
                        .map(|f| {
                            json!({
                                "node_id": f.node_id, "concept": f.concept,
                                "content": f.content, "confidence": f.confidence,
                                "source_id": f.source_id, "tags": f.tags,
                            })
                        })
                        .collect();
                    Ok(json!({"facts": facts}))
                }
                "memory.recall_procedure" => {
                    let s = store.lock().unwrap();
                    let procs: Vec<serde_json::Value> = s
                        .procedures
                        .iter()
                        .map(|p| {
                            json!({
                                "node_id": p.node_id, "name": p.name,
                                "steps": p.steps, "prerequisites": p.prerequisites,
                                "usage_count": p.usage_count,
                            })
                        })
                        .collect();
                    Ok(json!({"procedures": procs}))
                }
                "memory.store_fact" => {
                    let mut s = store.lock().unwrap();
                    let id = format!("fact-{}", s.facts.len() + 1);
                    s.facts.push(CognitiveFact {
                        node_id: id.clone(),
                        concept: params["concept"].as_str().unwrap_or("").to_string(),
                        content: params["content"].as_str().unwrap_or("").to_string(),
                        confidence: params["confidence"].as_f64().unwrap_or(0.0),
                        source_id: params["source_id"].as_str().unwrap_or("").to_string(),
                        tags: params["tags"]
                            .as_array()
                            .unwrap_or(&vec![])
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect(),
                    });
                    Ok(json!({"id": id}))
                }
                "memory.store_procedure" => {
                    let mut s = store.lock().unwrap();
                    let id = format!("proc-{}", s.procedures.len() + 1);
                    s.procedures.push(CognitiveProcedure {
                        node_id: id.clone(),
                        name: params["name"].as_str().unwrap_or("").to_string(),
                        steps: params["steps"]
                            .as_array()
                            .unwrap_or(&vec![])
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect(),
                        prerequisites: params["prerequisites"]
                            .as_array()
                            .unwrap_or(&vec![])
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect(),
                        usage_count: 0,
                    });
                    Ok(json!({"id": id}))
                }
                _ => Err(crate::bridge::BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown method: {method}"),
                }),
            });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    #[test]
    fn export_empty_bridge_returns_empty_snapshot() {
        let bridge = mock_bridge();
        let snapshot = export_memory_snapshot(&bridge, "test-agent", None).unwrap();
        assert!(snapshot.is_empty());
        assert_eq!(snapshot.total_items(), 0);
        assert_eq!(snapshot.source_agent, "test-agent");
        assert!(snapshot.exported_at > 0);
    }

    #[test]
    fn export_rejects_empty_agent_name() {
        let bridge = mock_bridge();
        let err = export_memory_snapshot(&bridge, "", None).unwrap_err();
        assert!(matches!(err, SimardError::InvalidConfigValue { .. }));
    }

    #[test]
    fn round_trip_export_import() {
        let source = mock_bridge();
        // Store some data in the source bridge.
        source
            .store_fact("rust", "systems language", 0.9, &[], "ep-1")
            .unwrap();
        source
            .store_procedure("build", &["compile".to_string(), "test".to_string()], &[])
            .unwrap();

        let snapshot = export_memory_snapshot(&source, "agent-1", None).unwrap();
        assert_eq!(snapshot.facts.len(), 1);
        assert_eq!(snapshot.procedures.len(), 1);
        assert_eq!(snapshot.total_items(), 2);

        // Import into a fresh target bridge.
        let target = mock_bridge();
        let count = import_memory_snapshot(&target, &snapshot).unwrap();
        assert_eq!(count, 2);

        // Verify the target has the data.
        let target_snapshot = export_memory_snapshot(&target, "agent-2", None).unwrap();
        assert_eq!(target_snapshot.facts.len(), 1);
        assert_eq!(target_snapshot.procedures.len(), 1);
    }

    #[test]
    fn snapshot_serializes_to_json() {
        let snapshot = MemorySnapshot {
            facts: vec![CognitiveFact {
                node_id: "f1".to_string(),
                concept: "test".to_string(),
                content: "test content".to_string(),
                confidence: 0.8,
                source_id: "".to_string(),
                tags: vec![],
            }],
            procedures: vec![],
            exported_at: 1000,
            source_agent: "agent-x".to_string(),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: MemorySnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.facts.len(), 1);
        assert_eq!(parsed.source_agent, "agent-x");
    }

    #[test]
    fn snapshot_display_is_readable() {
        let snapshot = MemorySnapshot {
            facts: vec![],
            procedures: vec![],
            exported_at: 1000,
            source_agent: "agent-x".to_string(),
        };
        let s = snapshot.to_string();
        assert!(s.contains("facts=0"));
        assert!(s.contains("agent-x"));
    }

    #[test]
    fn export_to_file_and_load() {
        let bridge = mock_bridge();
        bridge
            .store_fact("rust", "fast language", 0.95, &[], "")
            .unwrap();

        let dir = std::env::temp_dir().join("simard-test-snapshot");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("snapshot.json");

        let snapshot = export_memory_snapshot(&bridge, "file-agent", Some(&path)).unwrap();
        assert_eq!(snapshot.facts.len(), 1);

        let loaded = load_snapshot_from_file(&path).unwrap();
        assert_eq!(loaded.facts.len(), 1);
        assert_eq!(loaded.source_agent, "file-agent");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
