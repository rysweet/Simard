//! IPC bridge between clients (meeting, engineer, etc.) and the
//! OODA daemon's cognitive memory.
//!
//! The daemon holds an exclusive lock on `cognitive_memory.ladybug`. To let
//! other processes read/write memory while the daemon is running, the daemon
//! publishes a Unix-domain socket at `{socket_dir}/memory.sock` and dispatches
//! [`MemoryRequest`] messages to its in-process [`NativeCognitiveMemory`].
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
mod tests_bridge_isolation;
#[cfg(test)]
mod tests_launcher;
#[cfg(test)]
mod tests_socket_path;

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
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
/// `docs/reference/cognitive-memory-bridge-helpers.md` for the bridge-
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

/// Environment variable that opts a test out of the hermetic-state-root
/// guard. Read by the cfg(test)-only assertion sites
/// (`save_goal_board` / `save_goal_board_with_removals`,
/// `NativeCognitiveMemory::store_fact`, `launch_writer_bridge`). The
/// only legitimate consumer is the npm install-real / install-fake
/// harness; new uses require code-review acknowledgement.
pub const TEST_ALLOW_LIVE_STATE_ENV: &str = "SIMARD_TEST_ALLOW_LIVE_STATE";

/// Default state-root directory used by daemon, meeting, and any other client
/// that needs to know where the on-disk cognitive-memory DB lives.
///
/// Resolution order:
///   1. `SIMARD_STATE_ROOT` environment variable (explicit override)
///   2. `$HOME/.simard/state`
///
/// Both the OODA daemon and the meeting REPL must agree on this path,
/// otherwise the meeting's direct-open fallback targets a different DB
/// than the one the daemon owns.
pub fn default_state_root() -> PathBuf {
    if let Ok(v) = std::env::var("SIMARD_STATE_ROOT") {
        return PathBuf::from(v);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    PathBuf::from(home).join(".simard").join("state")
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
    Statistics(CognitiveStatistics),
    Error(String),
}

pub(crate) fn ipc_err(ctx: &str, e: impl std::fmt::Display) -> SimardError {
    SimardError::BridgeTransportError {
        bridge: "memory-ipc".to_string(),
        reason: format!("{ctx}: {e}"),
    }
}

pub(crate) fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> SimardResult<()> {
    let len = u32::try_from(payload.len()).map_err(|_| SimardError::BridgeTransportError {
        bridge: "memory-ipc".into(),
        reason: format!("message too large: {} bytes", payload.len()),
    })?;
    w.write_all(&len.to_be_bytes())
        .map_err(|e| ipc_err("write-len", e))?;
    w.write_all(payload).map_err(|e| ipc_err("write-body", e))?;
    w.flush().map_err(|e| ipc_err("flush", e))
}

/// Maximum accepted IPC frame size (16 MiB).
///
/// Caps the per-message allocation triggered by [`read_frame`]. The 4-byte
/// big-endian length prefix could theoretically request a ~4 GiB buffer,
/// which a compromised or buggy peer could use to OOM the daemon or any
/// client. 16 MiB comfortably exceeds every legitimate `MemoryRequest` /
/// `MemoryResponse` (the largest realistic payloads are bulk fact searches
/// or procedure recalls, which are bounded by client-supplied `limit`
/// values measured in entries, not megabytes).
pub(crate) const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn read_frame<R: Read>(r: &mut R) -> SimardResult<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)
        .map_err(|e| ipc_err("read-len", e))?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(SimardError::BridgeTransportError {
            bridge: "memory-ipc".into(),
            reason: format!(
                "frame length {len} exceeds maximum {MAX_FRAME_BYTES} bytes; \
                 refusing to allocate (possible malformed or hostile peer)"
            ),
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
pub use launcher::{
    ReaderBridge, WriterBridge, launch_writer_bridge, open_reader_bridge,
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
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        self.0.get_statistics()
    }
    fn is_read_only(&self) -> bool {
        self.0.is_read_only()
    }
    fn checkpoint(&self) -> SimardResult<()> {
        self.0.checkpoint()
    }
}

// ============================================================================
// Stale lock reaping
// ============================================================================

/// Remove `cognitive_memory.open.lock` if no running process holds it.
///
/// The lock file is created by [`NativeCognitiveMemory::open`] and normally
/// released automatically when the owning process exits. In rare cases
/// (e.g. SIGKILL, OOM) the file can linger with no owner — which has been
/// observed to confuse LadybugDB's own locking. This function writes the
/// current pid into the file on successful open; on startup, if the recorded
/// pid isn't alive, the file is removed.
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
