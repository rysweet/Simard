//! Memory snapshot replication for remote agent sessions.
//!
//! # Deprecated
//!
//! This module's JSON-snapshot replication approach is superseded by the
//! amplihack hive-mind DHT+bloom gossip protocol. The memory adapter now
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
//! `CognitiveMemoryAdapter`, serializes them into a `MemorySnapshot`, and
//! can import that snapshot into a remote adapter.
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
    /// Semantic facts exported from the source adapter.
    pub facts: Vec<CognitiveFact>,
    /// Procedural memories exported from the source adapter.
    pub procedures: Vec<CognitiveProcedure>,
    /// Unix epoch seconds when this snapshot was created.
    pub exported_at: u64,
    /// The agent name that produced this snapshot.
    pub source_agent: String,
}

/// Current on-disk envelope schema version.
///
/// Bump this when the `PersistedEnvelope` or `MemorySnapshot` wire format
/// changes in a backward-incompatible way. Consumers dispatch on this
/// value to decide whether they can load the file directly or need a
/// migration step (see issue #1941 for the migration policy decision).
pub const ENVELOPE_SCHEMA_VERSION: u32 = 1;

/// Durable on-disk wrapper for [`MemorySnapshot`] (issue #1917).
///
/// Every snapshot written to disk is serialized as a `PersistedEnvelope`
/// rather than a bare `MemorySnapshot`. The top-level `schema_version`
/// field lets future code detect the format without guessing, and
/// enables the migration policy from issue #1941.
///
/// **Reading:** [`load_snapshot_from_file`] transparently handles both
/// legacy (bare `MemorySnapshot`) and enveloped files — if the JSON has
/// a `schema_version` key it's parsed as an envelope; otherwise it's
/// deserialized directly as a `MemorySnapshot`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedEnvelope {
    /// Format version tag (currently [`ENVELOPE_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The actual snapshot payload.
    pub payload: MemorySnapshot,
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

/// Export the **complete** cognitive memory (every fact + procedure) for a
/// verified backup of the live store (issue #2420).
///
/// Unlike [`export_memory_snapshot`] — which caps at [`MAX_EXPORT_FACTS`] /
/// [`MAX_EXPORT_PROCEDURES`] to keep replication/migration payloads bounded — a
/// verified backup must capture the *entire* store so a restore can round-trip
/// the **current** memory count. With the live store already past 10k memories
/// and growing daily, a fixed export cap would silently drop the tail and make
/// the count-verification gate ([`crate::memory_backup::verify_backup_memory_count`])
/// fail forever — exactly the silent-rot failure class this issue exists to
/// kill. It therefore requests with the maximum limit (`u32::MAX`), the same
/// unbounded retrieval [`CognitiveMemoryOps::graph_stats`] already performs
/// safely via `get_all_facts(usize::MAX)`, so the snapshot can never be capped
/// below the store size.
///
/// Selection semantics are otherwise identical to [`export_memory_snapshot`]
/// (wildcard query, `min_confidence = 0.0`); only the truncation is removed.
/// This is *not* deprecated: it is the backup path, distinct from the legacy
/// JSON replication this module otherwise superseded.
pub fn export_full_memory_snapshot(
    adapter: &dyn CognitiveMemoryOps,
    agent_name: &str,
) -> SimardResult<MemorySnapshot> {
    if agent_name.is_empty() {
        return Err(SimardError::InvalidConfigValue {
            key: "agent_name".to_string(),
            value: String::new(),
            help: "agent name cannot be empty for memory export".to_string(),
        });
    }

    // `u32::MAX` is the "return all" limit: the library's `get_all_facts` is
    // already called with `usize::MAX` by `graph_stats`, so an unbounded request
    // is safe (no per-limit pre-allocation). No real store approaches u32::MAX
    // facts, so this can never truncate.
    let facts = adapter.search_facts("*", u32::MAX, 0.0)?;
    let procedures = adapter.recall_procedure("*", u32::MAX)?;

    Ok(MemorySnapshot {
        facts,
        procedures,
        exported_at: current_epoch_seconds()?,
        source_agent: agent_name.to_string(),
    })
}

/// Export a memory snapshot from a cognitive memory adapter.
///
/// # Deprecated
/// Use the hive-mind distributed topology instead. The memory adapter now
/// replicates facts automatically via DHT+bloom gossip.
#[deprecated(
    since = "0.13.0",
    note = "Use Memory('simard', topology='distributed') hive-mind replication instead of JSON snapshots"
)]
pub fn export_memory_snapshot(
    adapter: &dyn CognitiveMemoryOps,
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
    let facts = adapter.search_facts("*", MAX_EXPORT_FACTS, 0.0)?;
    let procedures = adapter.recall_procedure("*", MAX_EXPORT_PROCEDURES)?;

    let now = current_epoch_seconds()?;

    let snapshot = MemorySnapshot {
        facts,
        procedures,
        exported_at: now,
        source_agent: agent_name.to_string(),
    };

    if let Some(path) = path {
        let envelope = PersistedEnvelope {
            schema_version: ENVELOPE_SCHEMA_VERSION,
            payload: snapshot.clone(),
        };
        let json = serde_json::to_string_pretty(&envelope).map_err(|e| {
            SimardError::PersistentStoreIo {
                store: "memory-snapshot".to_string(),
                action: "serialize".to_string(),
                path: path.to_path_buf(),
                reason: e.to_string(),
            }
        })?;
        // Route through the durable persistence pipeline so session-boundary
        // snapshots in ~/.simard/snapshots/ are crash-safe (temp + fsync +
        // rename + parent fsync). Previous behaviour used bare fs::write,
        // which left a window where a power loss could resurrect the
        // pre-rename inode on ext4/xfs (issue #1918).
        crate::persistence::persist_bytes("memory-snapshot", path, json.as_bytes())?;
    }

    Ok(snapshot)
}

/// Import a memory snapshot into a cognitive memory adapter.
///
/// # Deprecated
/// Use the hive-mind distributed topology instead.
#[deprecated(
    since = "0.13.0",
    note = "Use Memory('simard', topology='distributed') hive-mind replication instead of JSON snapshots"
)]
pub fn import_memory_snapshot(
    adapter: &dyn CognitiveMemoryOps,
    snapshot: &MemorySnapshot,
) -> SimardResult<usize> {
    let mut imported = 0;

    for fact in &snapshot.facts {
        adapter.store_fact(
            &fact.concept,
            &fact.content,
            fact.confidence,
            &fact.tags,
            &fact.source_id,
        )?;
        imported += 1;
    }

    for proc in &snapshot.procedures {
        adapter.store_procedure(&proc.name, &proc.steps, &proc.prerequisites)?;
        imported += 1;
    }

    Ok(imported)
}

/// Load a memory snapshot from a JSON file on disk.
///
/// Transparently handles both the legacy bare `MemorySnapshot` format
/// and the newer [`PersistedEnvelope`] format (issue #1917). If the
/// JSON contains a top-level `schema_version` key it is parsed as an
/// envelope; otherwise it falls back to bare `MemorySnapshot` deserialization.
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

    // Try envelope format first (has `schema_version`), fall back to bare snapshot.
    let json: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| SimardError::PersistentStoreIo {
            store: "memory-snapshot".to_string(),
            action: "deserialize".to_string(),
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

    if json.get("schema_version").is_some() {
        let envelope: PersistedEnvelope =
            serde_json::from_value(json).map_err(|e| SimardError::PersistentStoreIo {
                store: "memory-snapshot".to_string(),
                action: "deserialize-envelope".to_string(),
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
        Ok(envelope.payload)
    } else {
        serde_json::from_value(json).map_err(|e| SimardError::PersistentStoreIo {
            store: "memory-snapshot".to_string(),
            action: "deserialize-legacy".to_string(),
            path: path.to_path_buf(),
            reason: e.to_string(),
        })
    }
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
