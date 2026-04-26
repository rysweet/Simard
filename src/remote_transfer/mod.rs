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
mod tests;
