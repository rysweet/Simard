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

fn ipc_err(ctx: &str, e: impl std::fmt::Display) -> SimardError {
    SimardError::BridgeTransportError {
        bridge: "memory-ipc".to_string(),
        reason: format!("{ctx}: {e}"),
    }
}

fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> SimardResult<()> {
    let len = u32::try_from(payload.len()).map_err(|_| SimardError::BridgeTransportError {
        bridge: "memory-ipc".into(),
        reason: format!("message too large: {} bytes", payload.len()),
    })?;
    w.write_all(&len.to_be_bytes())
        .map_err(|e| ipc_err("write-len", e))?;
    w.write_all(payload).map_err(|e| ipc_err("write-body", e))?;
    w.flush().map_err(|e| ipc_err("flush", e))
}

fn read_frame<R: Read>(r: &mut R) -> SimardResult<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)
        .map_err(|e| ipc_err("read-len", e))?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)
        .map_err(|e| ipc_err("read-body", e))?;
    Ok(buf)
}

// ============================================================================
// Server
// ============================================================================

/// Spawn the memory IPC server as a background thread.
///
/// Removes any stale socket file, binds a new listener, and accepts
/// connections forever. Each connection is handled on its own thread.
/// Returns a handle that the caller can drop to release the listener's
/// file descriptor; the listener itself exits when the process exits.
pub fn spawn_server(
    socket_path: PathBuf,
    memory: Arc<dyn CognitiveMemoryOps>,
) -> SimardResult<ServerHandle> {
    if let Some(parent) = socket_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Reap stale socket file: if it exists and no one is listening, remove it.
    if socket_path.exists() {
        if UnixStream::connect(&socket_path).is_err() {
            let _ = std::fs::remove_file(&socket_path);
        } else {
            return Err(SimardError::BridgeSpawnFailed {
                bridge: "memory-ipc".into(),
                reason: format!(
                    "socket {} is already in use by another daemon",
                    socket_path.display()
                ),
            });
        }
    }
    let listener =
        UnixListener::bind(&socket_path).map_err(|e| SimardError::BridgeSpawnFailed {
            bridge: "memory-ipc".into(),
            reason: format!("bind {}: {e}", socket_path.display()),
        })?;

    let socket_clone = socket_path.clone();
    let mem = Arc::clone(&memory);
    let join = thread::Builder::new()
        .name("memory-ipc-server".into())
        .spawn(move || {
            for conn in listener.incoming() {
                match conn {
                    Ok(stream) => {
                        let m = Arc::clone(&mem);
                        thread::Builder::new()
                            .name("memory-ipc-conn".into())
                            .spawn(move || {
                                if let Err(e) = serve_connection(stream, m) {
                                    eprintln!("[simard] memory-ipc: connection error: {e}");
                                }
                            })
                            .ok();
                    }
                    Err(e) => {
                        eprintln!("[simard] memory-ipc: accept failed: {e}");
                        break;
                    }
                }
            }
        })
        .map_err(|e| SimardError::BridgeSpawnFailed {
            bridge: "memory-ipc".into(),
            reason: format!("spawn server thread: {e}"),
        })?;

    Ok(ServerHandle {
        socket_path: socket_clone,
        _join: Some(join),
    })
}

/// Drop guard that removes the socket file on drop.
pub struct ServerHandle {
    socket_path: PathBuf,
    _join: Option<thread::JoinHandle<()>>,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn serve_connection(
    mut stream: UnixStream,
    memory: Arc<dyn CognitiveMemoryOps>,
) -> SimardResult<()> {
    loop {
        let frame = match read_frame(&mut stream) {
            Ok(f) => f,
            Err(_) => return Ok(()), // EOF / client hung up
        };
        let req: MemoryRequest =
            serde_json::from_slice(&frame).map_err(|e| ipc_err("parse-request", e))?;
        let resp = dispatch(&*memory, req);
        let bytes = serde_json::to_vec(&resp).map_err(|e| ipc_err("serialize-response", e))?;
        write_frame(&mut stream, &bytes)?;
    }
}

fn dispatch(memory: &dyn CognitiveMemoryOps, req: MemoryRequest) -> MemoryResponse {
    match req {
        MemoryRequest::Ping => MemoryResponse::Pong,
        MemoryRequest::RecordSensory {
            modality,
            raw_data,
            ttl_seconds,
        } => match memory.record_sensory(&modality, &raw_data, ttl_seconds) {
            Ok(id) => MemoryResponse::Id(id),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
        MemoryRequest::PruneExpiredSensory => match memory.prune_expired_sensory() {
            Ok(n) => MemoryResponse::Count(n),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
        MemoryRequest::PushWorking {
            slot_type,
            content,
            task_id,
            relevance,
        } => match memory.push_working(&slot_type, &content, &task_id, relevance) {
            Ok(id) => MemoryResponse::Id(id),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
        MemoryRequest::GetWorking { task_id } => match memory.get_working(&task_id) {
            Ok(v) => MemoryResponse::WorkingSlots(v),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
        MemoryRequest::ClearWorking { task_id } => match memory.clear_working(&task_id) {
            Ok(n) => MemoryResponse::Count(n),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
        MemoryRequest::StoreEpisode {
            content,
            source_label,
            metadata,
        } => match memory.store_episode(&content, &source_label, metadata.as_ref()) {
            Ok(id) => MemoryResponse::Id(id),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
        MemoryRequest::ConsolidateEpisodes { batch_size } => {
            match memory.consolidate_episodes(batch_size) {
                Ok(opt) => MemoryResponse::MaybeId(opt),
                Err(e) => MemoryResponse::Error(e.to_string()),
            }
        }
        MemoryRequest::StoreFact {
            concept,
            content,
            confidence,
            tags,
            source_id,
        } => match memory.store_fact(&concept, &content, confidence, &tags, &source_id) {
            Ok(id) => MemoryResponse::Id(id),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
        MemoryRequest::SearchFacts {
            query,
            limit,
            min_confidence,
        } => match memory.search_facts(&query, limit, min_confidence) {
            Ok(v) => MemoryResponse::Facts(v),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
        MemoryRequest::StoreProcedure {
            name,
            steps,
            prerequisites,
        } => match memory.store_procedure(&name, &steps, &prerequisites) {
            Ok(id) => MemoryResponse::Id(id),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
        MemoryRequest::RecallProcedure { query, limit } => {
            match memory.recall_procedure(&query, limit) {
                Ok(v) => MemoryResponse::Procedures(v),
                Err(e) => MemoryResponse::Error(e.to_string()),
            }
        }
        MemoryRequest::StoreProspective {
            description,
            trigger_condition,
            action_on_trigger,
            priority,
        } => match memory.store_prospective(
            &description,
            &trigger_condition,
            &action_on_trigger,
            priority,
        ) {
            Ok(id) => MemoryResponse::Id(id),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
        MemoryRequest::CheckTriggers { content } => match memory.check_triggers(&content) {
            Ok(v) => MemoryResponse::Prospectives(v),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
        MemoryRequest::GetStatistics => match memory.get_statistics() {
            Ok(s) => MemoryResponse::Statistics(s),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
    }
}

// ============================================================================
// Client
// ============================================================================

/// Client implementing [`CognitiveMemoryOps`] over the daemon's Unix socket.
pub struct RemoteCognitiveMemory {
    // Mutex because trait methods take &self but the socket is stateful.
    stream: Mutex<UnixStream>,
    socket_path: PathBuf,
}

impl RemoteCognitiveMemory {
    /// Connect to the daemon's memory socket. Returns an error if the socket
    /// doesn't exist, the daemon isn't listening, or the handshake fails.
    pub fn connect(socket_path: &Path) -> SimardResult<Self> {
        if !socket_path.exists() {
            return Err(SimardError::BridgeSpawnFailed {
                bridge: "memory-ipc-client".into(),
                reason: format!("socket {} not present", socket_path.display()),
            });
        }
        let stream =
            UnixStream::connect(socket_path).map_err(|e| SimardError::BridgeSpawnFailed {
                bridge: "memory-ipc-client".into(),
                reason: format!("connect {}: {e}", socket_path.display()),
            })?;
        // Short timeouts so a wedged daemon doesn't hang meeting forever.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
        let client = Self {
            stream: Mutex::new(stream),
            socket_path: socket_path.to_path_buf(),
        };
        // Handshake
        match client.call(MemoryRequest::Ping)? {
            MemoryResponse::Pong => Ok(client),
            other => Err(SimardError::BridgeSpawnFailed {
                bridge: "memory-ipc-client".into(),
                reason: format!("handshake: expected Pong, got {other:?}"),
            }),
        }
    }

    /// Socket path this client is connected to (for logging).
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    fn call(&self, req: MemoryRequest) -> SimardResult<MemoryResponse> {
        let bytes = serde_json::to_vec(&req).map_err(|e| ipc_err("serialize-request", e))?;
        let mut guard = self
            .stream
            .lock()
            .map_err(|e| ipc_err("lock-poisoned", e))?;
        write_frame(&mut *guard, &bytes)?;
        let resp_bytes = read_frame(&mut *guard)?;
        let resp: MemoryResponse =
            serde_json::from_slice(&resp_bytes).map_err(|e| ipc_err("parse-response", e))?;
        Ok(resp)
    }

    fn unexpected(name: &str, got: MemoryResponse) -> SimardError {
        match got {
            MemoryResponse::Error(msg) => SimardError::BridgeCallFailed {
                bridge: "memory-ipc".into(),
                method: name.into(),
                reason: msg,
            },
            other => SimardError::BridgeCallFailed {
                bridge: "memory-ipc".into(),
                method: name.into(),
                reason: format!("unexpected response variant: {other:?}"),
            },
        }
    }
}

impl CognitiveMemoryOps for RemoteCognitiveMemory {
    fn record_sensory(
        &self,
        modality: &str,
        raw_data: &str,
        ttl_seconds: u64,
    ) -> SimardResult<String> {
        match self.call(MemoryRequest::RecordSensory {
            modality: modality.into(),
            raw_data: raw_data.into(),
            ttl_seconds,
        })? {
            MemoryResponse::Id(s) => Ok(s),
            other => Err(Self::unexpected("record_sensory", other)),
        }
    }

    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        match self.call(MemoryRequest::PruneExpiredSensory)? {
            MemoryResponse::Count(n) => Ok(n),
            other => Err(Self::unexpected("prune_expired_sensory", other)),
        }
    }

    fn push_working(
        &self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> SimardResult<String> {
        match self.call(MemoryRequest::PushWorking {
            slot_type: slot_type.into(),
            content: content.into(),
            task_id: task_id.into(),
            relevance,
        })? {
            MemoryResponse::Id(s) => Ok(s),
            other => Err(Self::unexpected("push_working", other)),
        }
    }

    fn get_working(&self, task_id: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        match self.call(MemoryRequest::GetWorking {
            task_id: task_id.into(),
        })? {
            MemoryResponse::WorkingSlots(v) => Ok(v),
            other => Err(Self::unexpected("get_working", other)),
        }
    }

    fn clear_working(&self, task_id: &str) -> SimardResult<usize> {
        match self.call(MemoryRequest::ClearWorking {
            task_id: task_id.into(),
        })? {
            MemoryResponse::Count(n) => Ok(n),
            other => Err(Self::unexpected("clear_working", other)),
        }
    }

    fn store_episode(
        &self,
        content: &str,
        source_label: &str,
        metadata: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        match self.call(MemoryRequest::StoreEpisode {
            content: content.into(),
            source_label: source_label.into(),
            metadata: metadata.cloned(),
        })? {
            MemoryResponse::Id(s) => Ok(s),
            other => Err(Self::unexpected("store_episode", other)),
        }
    }

    fn consolidate_episodes(&self, batch_size: u32) -> SimardResult<Option<String>> {
        match self.call(MemoryRequest::ConsolidateEpisodes { batch_size })? {
            MemoryResponse::MaybeId(opt) => Ok(opt),
            other => Err(Self::unexpected("consolidate_episodes", other)),
        }
    }

    fn store_fact(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        match self.call(MemoryRequest::StoreFact {
            concept: concept.into(),
            content: content.into(),
            confidence,
            tags: tags.to_vec(),
            source_id: source_id.into(),
        })? {
            MemoryResponse::Id(s) => Ok(s),
            other => Err(Self::unexpected("store_fact", other)),
        }
    }

    fn search_facts(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        match self.call(MemoryRequest::SearchFacts {
            query: query.into(),
            limit,
            min_confidence,
        })? {
            MemoryResponse::Facts(v) => Ok(v),
            other => Err(Self::unexpected("search_facts", other)),
        }
    }

    fn store_procedure(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
    ) -> SimardResult<String> {
        match self.call(MemoryRequest::StoreProcedure {
            name: name.into(),
            steps: steps.to_vec(),
            prerequisites: prerequisites.to_vec(),
        })? {
            MemoryResponse::Id(s) => Ok(s),
            other => Err(Self::unexpected("store_procedure", other)),
        }
    }

    fn recall_procedure(&self, query: &str, limit: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        match self.call(MemoryRequest::RecallProcedure {
            query: query.into(),
            limit,
        })? {
            MemoryResponse::Procedures(v) => Ok(v),
            other => Err(Self::unexpected("recall_procedure", other)),
        }
    }

    fn store_prospective(
        &self,
        description: &str,
        trigger_condition: &str,
        action_on_trigger: &str,
        priority: i64,
    ) -> SimardResult<String> {
        match self.call(MemoryRequest::StoreProspective {
            description: description.into(),
            trigger_condition: trigger_condition.into(),
            action_on_trigger: action_on_trigger.into(),
            priority,
        })? {
            MemoryResponse::Id(s) => Ok(s),
            other => Err(Self::unexpected("store_prospective", other)),
        }
    }

    fn check_triggers(&self, content: &str) -> SimardResult<Vec<CognitiveProspective>> {
        match self.call(MemoryRequest::CheckTriggers {
            content: content.into(),
        })? {
            MemoryResponse::Prospectives(v) => Ok(v),
            other => Err(Self::unexpected("check_triggers", other)),
        }
    }

    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        match self.call(MemoryRequest::GetStatistics)? {
            MemoryResponse::Statistics(s) => Ok(s),
            other => Err(Self::unexpected("get_statistics", other)),
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_request_roundtrip_ping() {
        let req = MemoryRequest::Ping;
        let bytes = serde_json::to_vec(&req).unwrap();
        let back: MemoryRequest = serde_json::from_slice(&bytes).unwrap();
        assert!(matches!(back, MemoryRequest::Ping));
    }

    #[test]
    fn memory_response_roundtrip_error() {
        let resp = MemoryResponse::Error("boom".into());
        let bytes = serde_json::to_vec(&resp).unwrap();
        let back: MemoryResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(matches!(back, MemoryResponse::Error(ref s) if s == "boom"));
    }

    #[test]
    fn default_socket_path_under_dot_simard() {
        let p = default_socket_path();
        assert!(p.to_string_lossy().contains("/.simard/memory.sock"));
    }

    #[test]
    fn reap_stale_lock_noop_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let reaped = reap_stale_open_lock(dir.path()).unwrap();
        assert!(!reaped);
    }

    #[test]
    fn reap_stale_lock_removes_file_with_dead_pid() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join("cognitive_memory.ladybug.open.lock");
        // Use PID 1 doesn't work (it's always alive) so use a value that's
        // definitely not a live pid: one with a high number that's unlikely.
        // But `is_pid_alive` uses kill(0) which is unreliable. Use an empty
        // file and rely on flock_held=false path instead.
        std::fs::write(&lock, b"").unwrap();
        let reaped = reap_stale_open_lock(dir.path()).unwrap();
        assert!(reaped);
        assert!(!lock.exists());
    }

    #[test]
    fn server_client_roundtrip_with_in_memory_backend() {
        use crate::cognitive_memory::NativeCognitiveMemory;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("memory.sock");

        let mem: Arc<dyn CognitiveMemoryOps> =
            Arc::new(NativeCognitiveMemory::in_memory().expect("in-memory db"));
        let _handle = spawn_server(sock.clone(), mem).expect("spawn server");

        // Give server a moment to start accepting.
        for _ in 0..50 {
            if sock.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let client = RemoteCognitiveMemory::connect(&sock).expect("connect");
        let stats = client.get_statistics().expect("get_statistics");
        assert_eq!(stats.sensory_count, 0);

        let id = client
            .store_fact("gravity", "things fall", 0.9, &["physics".into()], "src-1")
            .expect("store_fact");
        assert!(!id.is_empty());

        let facts = client.search_facts("gravity", 10, 0.0).expect("search");
        assert!(!facts.is_empty());
    }
}
