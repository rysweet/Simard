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
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

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
pub fn default_socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    PathBuf::from(home).join(".simard").join("memory.sock")
}

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

pub(crate) fn read_frame<R: Read>(r: &mut R) -> SimardResult<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)
        .map_err(|e| ipc_err("read-len", e))?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)
        .map_err(|e| ipc_err("read-body", e))?;
    Ok(buf)
}

mod server;
mod client;
pub use client::RemoteCognitiveMemory;
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
    // ESRCH if it doesn't exist.
    let pid_i = pid as i32;
    unsafe { libc::kill(pid_i, 0) == 0 || *libc::__errno_location() != libc::ESRCH }
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
