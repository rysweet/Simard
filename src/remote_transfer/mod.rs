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
//! `CognitiveMemoryClient`, serializes them into a `MemorySnapshot`, and
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
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective,
};

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

/// A **complete** portable snapshot of cognitive memory (issue #2550).
///
/// [`MemorySnapshot`] carries only the migration-worthy subset (facts +
/// procedures). A verified backup that a corruption-reset must be recoverable
/// from has to capture **every** durable memory type — the incident that
/// motivated this type (a WAL corruption reset the store to empty) was
/// unrecoverable precisely because the on-disk `cognitive_snapshot.json` held
/// only facts + procedures, so episodes and prospective triggers were lost for
/// good.
///
/// It is a strict superset of [`MemorySnapshot`]: `episodes` and `prospective`
/// are `#[serde(default)]`, so a bare `MemorySnapshot` JSON (the legacy backup
/// shape) deserializes into a `FullMemorySnapshot` with empty
/// episodes/prospective, and a `FullMemorySnapshot` JSON deserializes into a
/// [`MemorySnapshot`] (the extra keys are ignored) — the count-verification
/// gate keeps working unchanged.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FullMemorySnapshot {
    /// Semantic facts (durable knowledge).
    pub facts: Vec<CognitiveFact>,
    /// Procedural memories (reusable workflows).
    pub procedures: Vec<CognitiveProcedure>,
    /// Autobiographical episodes. `#[serde(default)]` so a legacy bare-snapshot
    /// file (facts + procedures only) still loads.
    #[serde(default)]
    pub episodes: Vec<CognitiveEpisode>,
    /// Prospective (trigger → action) memories. `#[serde(default)]` for the
    /// same back-compat reason as `episodes`.
    #[serde(default)]
    pub prospective: Vec<CognitiveProspective>,
    /// Unix epoch seconds when this snapshot was created.
    pub exported_at: u64,
    /// The agent name that produced this snapshot.
    pub source_agent: String,
}

impl FullMemorySnapshot {
    /// Total number of durable items across all captured types.
    pub fn total_items(&self) -> usize {
        self.facts.len() + self.procedures.len() + self.episodes.len() + self.prospective.len()
    }

    /// Whether the snapshot holds no durable memories at all.
    pub fn is_empty(&self) -> bool {
        self.total_items() == 0
    }
}

impl Display for FullMemorySnapshot {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "FullMemorySnapshot(facts={}, procedures={}, episodes={}, prospective={}, agent={}, at={})",
            self.facts.len(),
            self.procedures.len(),
            self.episodes.len(),
            self.prospective.len(),
            self.source_agent,
            self.exported_at
        )
    }
}

/// Export the **complete** cognitive memory — every fact, procedure, episode,
/// and prospective trigger — for a verified backup of the live store (issues
/// #2420, #2550).
///
/// Unlike [`export_memory_snapshot`] — which caps at [`MAX_EXPORT_FACTS`] /
/// [`MAX_EXPORT_PROCEDURES`] to keep replication/migration payloads bounded — a
/// verified backup must capture the *entire* store so a restore round-trips the
/// **current** memory count and every type. With the live store already past
/// 10k memories and growing daily, a fixed export cap would silently drop the
/// tail and make the count-verification gate
/// ([`crate::memory_backup::verify_backup_memory_count`]) fail forever — exactly
/// the silent-rot failure class this path exists to kill. It therefore requests
/// with the maximum limit (`u32::MAX`), the same unbounded retrieval
/// [`CognitiveMemoryOps::graph_stats`] already performs safely, so the snapshot
/// can never be capped below the store size.
///
/// Issue #2550 extends the capture beyond facts + procedures to **all** durable
/// memory types: episodes ([`CognitiveMemoryOps::list_all_episodes`]) and
/// prospective triggers ([`CognitiveMemoryOps::list_all_prospective`]). Backends
/// without an enumerator for a type (the IPC client, test stubs) return an empty
/// list via the trait default, so the export degrades to the facts+procedures
/// subset rather than failing.
pub fn export_full_memory_snapshot(
    memory: &dyn CognitiveMemoryOps,
    agent_name: &str,
) -> SimardResult<FullMemorySnapshot> {
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
    // records, so this can never truncate.
    let facts = memory.search_facts("*", u32::MAX, 0.0)?;
    let procedures = memory.recall_procedure("*", u32::MAX)?;
    let episodes = memory.list_all_episodes(u32::MAX)?;
    let prospective = memory.list_all_prospective(u32::MAX)?;

    Ok(FullMemorySnapshot {
        facts,
        procedures,
        episodes,
        prospective,
        exported_at: current_epoch_seconds()?,
        source_agent: agent_name.to_string(),
    })
}

/// Import a [`FullMemorySnapshot`] into a store, **idempotently** (issue #2550).
///
/// Every item is deduplicated by content before it is written, so re-running a
/// restore — or auto-restoring a snapshot the store already partially holds —
/// never duplicates memories. Dedup keys per type:
///
/// * facts — `(concept, content)`
/// * procedures — `name`
/// * episodes — `content`
/// * prospective — `(trigger_condition, description)`
///
/// Returns the number of items actually written (skipped duplicates are not
/// counted). This is the restore/import counterpart of
/// [`export_full_memory_snapshot`]; it is *not* deprecated (unlike the legacy
/// [`import_memory_snapshot`] replication path, which does not dedup).
///
/// **Prospective status is preserved across the round-trip (issue #2562).**
/// [`CognitiveMemoryOps::store_prospective`] always creates a fresh `"pending"`
/// record, so a snapshot trigger that was already `"triggered"` or `"resolved"`
/// would otherwise come back `"pending"` — and
/// [`CognitiveMemoryOps::check_triggers`] would **re-fire a goal the daemon had
/// already handled** after an auto-restore or `simard memory import`. To close
/// that, any non-`pending` prospective is re-resolved to the terminal,
/// non-firing `"resolved"` status immediately after it is written. Genuinely
/// `pending` records restore as `pending` and stay eligible to fire. The
/// library exposes no arbitrary status setter and `"resolved"` is the correct
/// terminal state for both prior `"triggered"` and `"resolved"` records —
/// neither should fire again on a recovery restore, where no in-flight action
/// remains to resume.
///
/// Because the restore is idempotent, this also **self-heals** a store that a
/// pre-#2562 restore left with a stale `"pending"` copy of an already-handled
/// trigger: re-running the import resolves that pre-existing record when the
/// snapshot marks the same `(trigger_condition, description)` as terminal.
///
/// [`CognitiveMemoryOps::store_prospective`]: crate::cognitive_memory::CognitiveMemoryOps::store_prospective
/// [`CognitiveMemoryOps::check_triggers`]: crate::cognitive_memory::CognitiveMemoryOps::check_triggers
pub fn import_full_snapshot(
    memory: &dyn CognitiveMemoryOps,
    snapshot: &FullMemorySnapshot,
) -> SimardResult<usize> {
    use std::collections::{HashMap, HashSet};

    let mut imported = 0;

    // Facts: dedup by (concept, content).
    let mut seen_facts: HashSet<(String, String)> = memory
        .search_facts("*", u32::MAX, 0.0)?
        .into_iter()
        .map(|f| (f.concept, f.content))
        .collect();
    for fact in &snapshot.facts {
        let key = (fact.concept.clone(), fact.content.clone());
        if seen_facts.insert(key) {
            memory.store_fact(
                &fact.concept,
                &fact.content,
                fact.confidence,
                &fact.tags,
                &fact.source_id,
            )?;
            imported += 1;
        }
    }

    // Procedures: dedup by name.
    let mut seen_procs: HashSet<String> = memory
        .recall_procedure("*", u32::MAX)?
        .into_iter()
        .map(|p| p.name)
        .collect();
    for proc in &snapshot.procedures {
        if seen_procs.insert(proc.name.clone()) {
            memory.store_procedure(&proc.name, &proc.steps, &proc.prerequisites)?;
            imported += 1;
        }
    }

    // Episodes: dedup by content.
    let mut seen_eps: HashSet<String> = memory
        .list_all_episodes(u32::MAX)?
        .into_iter()
        .map(|e| e.content)
        .collect();
    for ep in &snapshot.episodes {
        if seen_eps.insert(ep.content.clone()) {
            memory.store_episode(&ep.content, &ep.source_label, None)?;
            imported += 1;
        }
    }

    // Prospective: dedup by (trigger_condition, description). Track each
    // existing record's (node_id, status) — not just its key — so a snapshot
    // record that is already terminal can also correct a *pre-existing* stale
    // "pending" duplicate (e.g. a store left half-restored by the pre-#2562
    // bug), keeping the restore idempotent with respect to status (issue #2562).
    let mut seen_pros: HashMap<(String, String), (String, String)> = memory
        .list_all_prospective(u32::MAX)?
        .into_iter()
        .map(|p| ((p.trigger_condition, p.description), (p.node_id, p.status)))
        .collect();
    for pm in &snapshot.prospective {
        let key = (pm.trigger_condition.clone(), pm.description.clone());
        // Any status other than "pending" is a trigger the daemon already
        // handled; it must restore non-firing so `check_triggers` (which fires
        // only "pending" records) cannot resurrect a completed goal.
        let snapshot_is_terminal = !pm.status.eq_ignore_ascii_case("pending");

        if let Some((existing_node_id, existing_status)) = seen_pros.get(&key) {
            // Already present in the target. Only act on the stale-bug case:
            // the snapshot marks this trigger as already-handled but the live
            // record is still "pending". Resolve it so a re-run of the restore
            // cannot leave a completed trigger eligible to re-fire. A live
            // record that is already terminal is left untouched.
            if snapshot_is_terminal && existing_status.eq_ignore_ascii_case("pending") {
                memory.resolve_prospective(existing_node_id)?;
            }
            continue;
        }

        let node_id = memory.store_prospective(
            &pm.description,
            &pm.trigger_condition,
            &pm.action_on_trigger,
            pm.priority,
        )?;
        // `store_prospective` always creates a "pending" record; re-resolve any
        // non-pending snapshot record to the terminal, non-firing "resolved"
        // status so a recovery restore cannot resurrect a completed trigger.
        if snapshot_is_terminal {
            memory.resolve_prospective(&node_id)?;
        }
        // Register the freshly written record so a later duplicate of this key
        // within the same snapshot is treated as already-present (mirrors the
        // dedup for the other memory types).
        seen_pros.insert(
            key,
            (
                node_id,
                if snapshot_is_terminal {
                    "resolved"
                } else {
                    "pending"
                }
                .to_string(),
            ),
        );
        imported += 1;
    }

    Ok(imported)
}

/// Load a [`FullMemorySnapshot`] from a JSON file on disk (issue #2550).
///
/// Transparently accepts every snapshot shape Simard has written:
///
/// * a **full** snapshot (facts + procedures + episodes + prospective),
/// * a legacy **bare** [`MemorySnapshot`] (facts + procedures only — the
///   `episodes`/`prospective` keys default to empty), and
/// * a [`PersistedEnvelope`]-wrapped `MemorySnapshot` (session-boundary
///   snapshots): if a top-level `schema_version` key is present the `payload`
///   is unwrapped first.
///
/// This is what `simard memory import` and the daemon startup auto-restore read
/// `cognitive_snapshot.json` through.
pub fn load_full_snapshot_from_file(path: &Path) -> SimardResult<FullMemorySnapshot> {
    let content = std::fs::read_to_string(path).map_err(|e| SimardError::PersistentStoreIo {
        store: "memory-snapshot".to_string(),
        action: "read".to_string(),
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    // Fast path: a bare or full snapshot object deserializes straight into
    // `FullMemorySnapshot` in a single streaming pass (the `#[serde(default)]`
    // on `episodes`/`prospective` lets a legacy bare snapshot load too). This
    // skips building — and, for an envelope, deep-cloning — an intermediate
    // `serde_json::Value` DOM, which for a large snapshot (the incident held
    // tens of thousands of records) costs several times the final struct in
    // transient allocation on the P0 recovery/restore path. `facts`,
    // `procedures`, `exported_at` and `source_agent` are required fields, so an
    // envelope (which lacks them at top level) can never mis-parse here — it
    // fails and falls through to the byte-for-byte-identical envelope unwrap
    // below. For a plain snapshot object the two routes are equivalent (a direct
    // `from_str` and a `from_value` over the parsed DOM replay the same
    // `Deserialize`), so this is a pure resource-usage win with no behavior
    // change.
    if let Ok(snapshot) = serde_json::from_str::<FullMemorySnapshot>(&content) {
        return Ok(snapshot);
    }

    // Slow path (rare): a PersistedEnvelope-wrapped snapshot (session-boundary
    // files). Only these need the DOM to unwrap the payload before deserializing.
    let json: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| SimardError::PersistentStoreIo {
            store: "memory-snapshot".to_string(),
            action: "deserialize".to_string(),
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

    let payload = if json.get("schema_version").is_some() {
        json.get("payload").cloned().unwrap_or(json)
    } else {
        json
    };

    serde_json::from_value(payload).map_err(|e| SimardError::PersistentStoreIo {
        store: "memory-snapshot".to_string(),
        action: "deserialize-snapshot".to_string(),
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
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
