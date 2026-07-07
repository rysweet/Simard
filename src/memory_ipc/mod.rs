//! IPC bridge between clients (meeting, engineer, etc.) and the
//! OODA daemon's cognitive memory.
//!
//! The daemon holds an exclusive lock on the cognitive-memory store. To let
//! other processes read/write memory while the daemon is running, the daemon
//! publishes a Unix-domain socket at `{socket_dir}/memory.sock` and dispatches
//! [`MemoryRequest`] messages to its in-process [`LibraryCognitiveMemory`](crate::cognitive_memory::LibraryCognitiveMemory).
//!
//! Clients use [`RemoteCognitiveMemory`] which implements
//! [`CognitiveMemoryOps`] by sending framed JSON messages to the socket.
//!
//! Framing: 4-byte big-endian length prefix, then JSON payload. Same wire
//! format as [`crate::runtime_ipc::UnixSocketTransport`].
//!
//! Fallback: if no daemon is running (socket absent or connect fails), the
//! caller should open the DB directly.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(test)]
mod tests_client_isolation;
#[cfg(test)]
mod tests_default_state_root_1967;
#[cfg(test)]
mod tests_launcher;
#[cfg(test)]
mod tests_launcher_fail_closed_2896;
// TDD (RED) for issue #2679: additive gated-write protocol (StoreFactGated /
// StoreProcedureProvenance / FactWriteOutcome), the MAX_FRAME cap, and the
// RemoteCognitiveMemory remember_* client wrappers. Symbols are added in the
// implementation step; until then the unresolved paths are the red signal.
#[cfg(test)]
mod tests_gated_write_2679;
#[cfg(test)]
mod tests_shared_store_2320;
#[cfg(test)]
mod tests_socket_path;
#[cfg(test)]
mod tests_transport_roundtrip;

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot, GraphStats,
};

/// Standard socket path used by both server and clients.
///
/// We intentionally put the socket under `~/.simard/` (independent of any
/// `SIMARD_STATE_ROOT` override) so meeting and daemon discover each other
/// even when they disagree about the DB directory.
///
/// **Soft-deprecated** by the issues
/// [#1923](https://github.com/rysweet/Simard/issues/1923) /
/// [#1925](https://github.com/rysweet/Simard/issues/1925) fix in favour of
/// [`socket_path_for`], which follows the resolved state root and lets
/// `SIMARD_STATE_ROOT` actually be hermetic. New call sites must use
/// `socket_path_for(state_root)`. This helper is retained unchanged for
/// the legacy call sites scheduled for migration in the same PR.
pub fn default_socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    PathBuf::from(home).join(".simard").join("memory.sock")
}

/// Resolve the IPC socket path for a given `state_root`.
///
/// Resolution ladder (priority order):
///
/// 1. `SIMARD_MEMORY_SOCKET` env var — explicit operator override; returned
///    verbatim as a `PathBuf`. Used when daemon and clients must agree on
///    a path independent of either's state root (rare; primarily test
///    harnesses that pre-spawn a daemon).
/// 2. `<state_root>/memory.sock` — the socket lives next to the DB it
///    fronts. This is the default and what makes `SIMARD_STATE_ROOT`
///    actually hermetic: pointing the env var at a `TempDir` is sufficient
///    to keep tests off the live daemon's socket.
///
/// See issues [#1923](https://github.com/rysweet/Simard/issues/1923) /
/// [#1925](https://github.com/rysweet/Simard/issues/1925) for the
/// fixture-leak failure mode this resolution prevents, and
/// `docs/reference/cognitive-memory-client-helpers.md` for the client-
/// helper integration.
///
/// Implementation: env-var override (when non-empty) → `state_root.join("memory.sock")`.
pub fn socket_path_for(state_root: &Path) -> PathBuf {
    if let Some(raw) = std::env::var_os(MEMORY_SOCKET_ENV) {
        let s = raw.to_string_lossy();
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    state_root.join("memory.sock")
}

/// Environment variable that overrides the IPC socket path independent of
/// the state root.
///
/// When set + non-empty, [`socket_path_for`] returns this value verbatim.
/// Useful when daemon and clients intentionally target different state
/// roots (e.g. cross-mount probes) or for harnesses that pre-spawn a
/// daemon at a known path.
pub const MEMORY_SOCKET_ENV: &str = "SIMARD_MEMORY_SOCKET";

/// Environment variable that carries the distillation pass id to every
/// `simard memory remember` subprocess the distiller agent spawns (issue
/// #2679).
///
/// The distill runner exports this so a fact write tags the daemon's per-pass
/// write ledger even though the agent invokes the CLI with only the scalar
/// content flags (`--concept` / `--content` / `--source-episode-id`) and NO
/// explicit `--pass-id`. The `remember` CLI reads it as a fallback when
/// `--pass-id` is absent; that fallback is what lets `drain_pass_ledger`
/// report a real accepted-fact count on the production
/// distill path. Without it the pass id resolved empty, the server's ledger
/// no-op'd the write, and every pass reported `fact_count = 0` /
/// `reduction_pct = 100%` while facts were in fact stored (the silent
/// metrics-degradation regression fixed in the #2679 follow-up).
pub const DISTILL_PASS_ID_ENV: &str = "SIMARD_DISTILL_PASS_ID";

/// Environment variable that opts a test out of the hermetic-state-root
/// guard. Read by the cfg(test)-only assertion sites
/// (`save_goal_board` / `save_goal_board_with_removals`, the cognitive-memory
/// writer, `launch_writer_client`). The
/// only legitimate consumer is the npm install-real / install-fake
/// harness; new uses require code-review acknowledgement.
pub const TEST_ALLOW_LIVE_STATE_ENV: &str = "SIMARD_TEST_ALLOW_LIVE_STATE";

/// Default state-root directory used by daemon, meeting, and any other client
/// that needs to know where the on-disk cognitive-memory DB lives.
///
/// Delegates to [`crate::state_root::simard_state_root`] — the single
/// canonical resolver shared by the daemon, `simard goal` CLI, and every
/// other state-root-aware caller. Resolution order is the one documented
/// on that function:
///
///   1. `SIMARD_STATE_ROOT` environment variable (explicit override)
///   2. `$HOME/.simard`
///
/// **Issue #1967 regression pin:** earlier versions of this function
/// appended a `state` subdirectory (`$HOME/.simard/state`), which caused
/// the meeting REPL to talk to a *different* LadybugDB than the daemon,
/// making it impossible to discuss the real goal board in a meeting.
/// Do not reintroduce a subdirectory join here without updating the
/// daemon resolver in lockstep.
pub fn default_state_root() -> PathBuf {
    crate::state_root::simard_state_root()
}

/// Request types mirroring [`CognitiveMemoryOps`] methods.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MemoryRequest {
    Ping,
    RecordSensory {
        modality: String,
        raw_data: String,
        ttl_seconds: u64,
    },
    PruneExpiredSensory,
    PushWorking {
        slot_type: String,
        content: String,
        task_id: String,
        relevance: f64,
    },
    GetWorking {
        task_id: String,
    },
    ClearWorking {
        task_id: String,
    },
    StoreEpisode {
        content: String,
        source_label: String,
        metadata: Option<serde_json::Value>,
    },
    ConsolidateEpisodes {
        batch_size: u32,
    },
    StoreFact {
        concept: String,
        content: String,
        confidence: f64,
        tags: Vec<String>,
        source_id: String,
    },
    /// Gated per-fact write (issue #2679). Carries one distilled fact from the
    /// distiller agentic step (via `simard memory remember`) to the daemon's
    /// authoritative write-boundary gate. The server — NOT the client — grounds
    /// the fact against the store, scores it with the shared
    /// [`crate::fact_reliability`] scorer, quarantines anything below threshold,
    /// dedups against an equal-or-stronger prior, and persists survivors with the
    /// *server-computed* confidence and provenance edges. The client-supplied
    /// `confidence` is only a hint the server ignores; `source_episode_ids` is
    /// the provenance the server verifies. There is no return document to parse —
    /// the disposition flows back as [`MemoryResponse::FactWrite`].
    StoreFactGated {
        concept: String,
        content: String,
        confidence: f64,
        tags: Vec<String>,
        source_id: String,
        source_episode_ids: Vec<String>,
        pass_id: String,
    },
    SearchFacts {
        query: String,
        limit: u32,
        min_confidence: f64,
    },
    StoreProcedure {
        name: String,
        steps: Vec<String>,
        prerequisites: Vec<String>,
    },
    /// Procedure write carrying its source-episode provenance (issue #2679), so
    /// the daemon draws a `PROCEDURE_DERIVES_FROM` edge to each source episode.
    /// The companion of [`StoreFactGated`](MemoryRequest::StoreFactGated) for the
    /// distiller's procedure output. Returns [`MemoryResponse::Id`].
    StoreProcedureProvenance {
        name: String,
        steps: Vec<String>,
        prerequisites: Vec<String>,
        source_episode_ids: Vec<String>,
        pass_id: String,
    },
    RecallProcedure {
        query: String,
        limit: u32,
    },
    StoreProspective {
        description: String,
        trigger_condition: String,
        action_on_trigger: String,
        priority: i64,
    },
    CheckTriggers {
        content: String,
    },
    ResolveProspective {
        node_id: String,
    },
    /// PR-C (issue #2281, problem 4): keyword-overlap episodic search
    /// for `preparation_memory_operations`.
    SearchEpisodesByKeywords {
        keywords: Vec<String>,
        limit: u32,
    },
    /// Drain and return the count of facts the write-boundary gate ACCEPTED for
    /// a given distillation `pass_id` (issue #2679). The distiller subprocess
    /// tags each `StoreFactGated` write with its `pass_id`; after the recipe run
    /// it drains the ledger to report how many facts the gate accepted — the only
    /// way to count facts on a path with no returned document. Returns
    /// [`MemoryResponse::Count`].
    DrainPassLedger {
        pass_id: String,
    },
    GetStatistics,
}

/// Response types matching each request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "ok", content = "value", rename_all = "snake_case")]
pub enum MemoryResponse {
    Pong,
    Id(String),
    Count(usize),
    MaybeId(Option<String>),
    WorkingSlots(Vec<CognitiveWorkingSlot>),
    Facts(Vec<CognitiveFact>),
    Procedures(Vec<CognitiveProcedure>),
    Prospectives(Vec<CognitiveProspective>),
    /// PR-C (issue #2281, problem 4): response variant for
    /// [`MemoryRequest::SearchEpisodesByKeywords`].
    Episodes(Vec<CognitiveEpisode>),
    Statistics(CognitiveStatistics),
    /// Server-side disposition of a [`MemoryRequest::StoreFactGated`] write
    /// (issue #2679): whether the fact was stored or quarantined and the
    /// confidence the server *computed* (never the client's hint). Reported so a
    /// caller — and the `simard memory remember` CLI — can surface the result
    /// WITHOUT any document to deserialize.
    FactWrite(FactWriteOutcome),
    Ack,
    Error(String),
}

/// The server's decision for one gated fact write (issue #2679).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FactWriteOutcome {
    /// The fact cleared the gate and was persisted.
    pub stored: bool,
    /// The fact was blocked by the gate (ungrounded, empty content, below
    /// threshold, or an equal-or-stronger prior already exists) and NOT stored.
    pub quarantined: bool,
    /// The confidence the server computed from the shared reliability scorer
    /// (NOT the client-supplied hint).
    pub confidence: f64,
    /// The `node_id` of the persisted fact, present only when `stored`.
    pub node_id: Option<String>,
}

pub(crate) fn ipc_err(ctx: &str, e: impl std::fmt::Display) -> SimardError {
    SimardError::RpcTransportError {
        bridge: "memory-ipc".to_string(),
        reason: format!("{ctx}: {e}"),
    }
}

/// Maximum accepted framed-message body size (issue #2679 socket hardening).
///
/// A hostile or corrupt client can send a 4-byte length prefix claiming a
/// multi-gigabyte body; the frame reader rejects any length exceeding this cap
/// BEFORE allocating or reading the body, so a bad prefix can never trigger a
/// giant allocation. 8 MiB is far larger than any legitimate per-fact write (a
/// single fact is a few hundred bytes) yet small enough to bound abuse.
pub(crate) const MAX_FRAME: usize = 8 * 1024 * 1024;

pub(crate) fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> SimardResult<()> {
    let len = u32::try_from(payload.len()).map_err(|_| SimardError::RpcTransportError {
        bridge: "memory-ipc".into(),
        reason: format!("message too large: {} bytes", payload.len()),
    })?;
    w.write_all(&len.to_be_bytes())
        .map_err(|e| ipc_err("write-len", e))?;
    w.write_all(payload).map_err(|e| ipc_err("write-body", e))?;
    w.flush().map_err(|e| ipc_err("flush", e))
}

pub(crate) fn read_frame<R: Read>(r: &mut R) -> SimardResult<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)
        .map_err(|e| ipc_err("read-len", e))?;
    let len = u32::from_be_bytes(len_buf) as usize;
    // Reject an oversized length BEFORE allocating or reading the body, so a
    // corrupt/hostile prefix cannot force a huge `vec![0u8; len]` allocation
    // (issue #2679 socket hardening).
    if len > MAX_FRAME {
        return Err(SimardError::RpcTransportError {
            bridge: "memory-ipc".into(),
            reason: format!("frame length {len} exceeds MAX_FRAME {MAX_FRAME}"),
        });
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)
        .map_err(|e| ipc_err("read-body", e))?;
    Ok(buf)
}

mod client;
mod launcher;
mod server;
pub use client::RemoteCognitiveMemory;
pub use launcher::clear_in_process_writer;
pub use launcher::clear_tier2_store_cache;
pub use launcher::{
    ReaderClient, WriterClient, launch_writer_client, open_reader_client,
    register_in_process_writer,
};
pub use server::{ServerHandle, spawn_server};
// ============================================================================
// Shared-memory adapter: Arc → Box<dyn CognitiveMemoryOps>
// ============================================================================

/// Wraps an `Arc<dyn CognitiveMemoryOps>` as a `Box<dyn CognitiveMemoryOps>`
/// so the same underlying store can be shared by the OODA loop and the IPC
/// server without opening the database twice (which would deadlock on the
/// LadybugDB lock).
pub struct SharedMemory(pub Arc<dyn CognitiveMemoryOps>);

impl CognitiveMemoryOps for SharedMemory {
    fn record_sensory(&self, modality: &str, raw_data: &str, ttl: u64) -> SimardResult<String> {
        self.0.record_sensory(modality, raw_data, ttl)
    }
    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        self.0.prune_expired_sensory()
    }
    fn push_working(
        &self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> SimardResult<String> {
        self.0.push_working(slot_type, content, task_id, relevance)
    }
    fn get_working(&self, task_id: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        self.0.get_working(task_id)
    }
    fn clear_working(&self, task_id: &str) -> SimardResult<usize> {
        self.0.clear_working(task_id)
    }
    fn store_episode(
        &self,
        content: &str,
        source_label: &str,
        metadata: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        self.0.store_episode(content, source_label, metadata)
    }
    fn consolidate_episodes(&self, batch_size: u32) -> SimardResult<Option<String>> {
        self.0.consolidate_episodes(batch_size)
    }
    fn store_fact(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        self.0
            .store_fact(concept, content, confidence, tags, source_id)
    }
    fn search_facts(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        self.0.search_facts(query, limit, min_confidence)
    }
    fn store_procedure(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
    ) -> SimardResult<String> {
        self.0.store_procedure(name, steps, prerequisites)
    }
    fn recall_procedure(&self, query: &str, limit: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        self.0.recall_procedure(query, limit)
    }
    fn store_prospective(
        &self,
        description: &str,
        trigger_condition: &str,
        action_on_trigger: &str,
        priority: i64,
    ) -> SimardResult<String> {
        self.0
            .store_prospective(description, trigger_condition, action_on_trigger, priority)
    }
    fn check_triggers(&self, content: &str) -> SimardResult<Vec<CognitiveProspective>> {
        self.0.check_triggers(content)
    }
    fn resolve_prospective(&self, node_id: &str) -> SimardResult<()> {
        self.0.resolve_prospective(node_id)
    }
    fn list_all_prospective(&self, limit: u32) -> SimardResult<Vec<CognitiveProspective>> {
        self.0.list_all_prospective(limit)
    }
    fn list_all_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        self.0.list_all_episodes(limit)
    }
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        self.0.get_statistics()
    }
    fn is_read_only(&self) -> bool {
        self.0.is_read_only()
    }
    fn checkpoint(&self) -> SimardResult<()> {
        self.0.checkpoint()
    }
    fn mark_episode_distilled(&self, node_id: &str) -> SimardResult<()> {
        self.0.mark_episode_distilled(node_id)
    }
    fn episode_exists(&self, node_id: &str) -> SimardResult<bool> {
        self.0.episode_exists(node_id)
    }
    fn any_episode_exists(&self, node_ids: &[String]) -> SimardResult<bool> {
        self.0.any_episode_exists(node_ids)
    }
    fn list_undistilled_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        self.0.list_undistilled_episodes(limit)
    }
    fn procedure_exists(&self, name: &str) -> SimardResult<bool> {
        self.0.procedure_exists(name)
    }
    fn search_episodes_by_keywords(
        &self,
        keywords: &[String],
        limit: u32,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        self.0.search_episodes_by_keywords(keywords, limit)
    }
    fn search_episodes_starting_with(
        &self,
        prefix: &str,
        limit: u32,
    ) -> SimardResult<Vec<(String, chrono::DateTime<chrono::Utc>)>> {
        self.0.search_episodes_starting_with(prefix, limit)
    }
    fn recall_facts_ranked(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
        weights: crate::cognitive_memory::RecallWeightSet,
    ) -> SimardResult<Vec<CognitiveFact>> {
        self.0
            .recall_facts_ranked(query, limit, min_confidence, weights)
    }
    fn prune_superseded(&self) -> SimardResult<usize> {
        self.0.prune_superseded()
    }
    fn episodes_for_fact(&self, fact_id: &str) -> SimardResult<Vec<String>> {
        self.0.episodes_for_fact(fact_id)
    }
    fn store_fact_with_caller_key(
        &self,
        caller_key: &str,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        self.0
            .store_fact_with_caller_key(caller_key, concept, content, confidence, tags, source_id)
    }
    fn store_fact_with_provenance(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        source_id: &str,
        tags: Option<&[String]>,
        metadata: Option<&std::collections::HashMap<String, serde_json::Value>>,
        source_episode_ids: &[String],
    ) -> SimardResult<String> {
        self.0.store_fact_with_provenance(
            concept,
            content,
            confidence,
            source_id,
            tags,
            metadata,
            source_episode_ids,
        )
    }
    fn store_procedure_with_provenance(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
        source_episode_ids: &[String],
    ) -> SimardResult<String> {
        self.0
            .store_procedure_with_provenance(name, steps, prerequisites, source_episode_ids)
    }
    fn graph_stats(&self) -> SimardResult<GraphStats> {
        // Issue #2331: forward to the wrapped in-process store so a tier-0
        // `open_reader_client` reader (same-process daemon writer) reports the
        // real edge / dedup counts instead of the all-zero trait default.
        self.0.graph_stats()
    }
}

// ============================================================================
// Stale lock reaping
// ============================================================================

/// Remove `cognitive_memory.open.lock` if no running process holds it.
///
/// This lock file is a legacy artifact of the deleted native backend's
/// open path. The library backend manages its own locking, so on current
/// installs this file generally does not exist and the function is a no-op;
/// it is retained to clean up stale locks left by pre-de-fork daemons.
pub fn reap_stale_open_lock(state_root: &Path) -> SimardResult<bool> {
    let lock_path = state_root.join("cognitive_memory.ladybug.open.lock");
    if !lock_path.exists() {
        return Ok(false);
    }
    // If the file is empty, we can't tell who owns it. Check if anyone can
    // acquire an exclusive flock on it — if yes, no-one else owns it and we
    // can safely delete.
    let contents = std::fs::read_to_string(&lock_path).unwrap_or_default();
    let recorded_pid: Option<u32> = contents.trim().parse().ok();

    let can_remove = match recorded_pid {
        Some(pid) => !is_pid_alive(pid),
        // Unknown pid: try to probe via non-blocking flock.
        None => !flock_held(&lock_path),
    };

    if can_remove {
        let _ = std::fs::remove_file(&lock_path);
        eprintln!(
            "[simard] reaped stale {} (no live owner)",
            lock_path.display()
        );
        Ok(true)
    } else {
        Ok(false)
    }
}

fn is_pid_alive(pid: u32) -> bool {
    // kill(pid, 0) returns 0 if the process exists and we can signal it,
    // ESRCH if it doesn't exist. Use std::io::Error::last_os_error() to read
    // errno portably (macOS exposes __error(), Linux exposes __errno_location()).
    let pid_i = pid as i32;
    if unsafe { libc::kill(pid_i, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn flock_held(path: &Path) -> bool {
    use std::os::unix::io::AsRawFd;
    let Ok(f) = std::fs::File::open(path) else {
        return false;
    };
    let fd = f.as_raw_fd();
    // Try a non-blocking exclusive lock. If we get it, nobody holds it; release.
    let got = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if got == 0 {
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
        false
    } else {
        true
    }
}
