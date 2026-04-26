//! Server: spawn_server + ServerHandle + serve_connection + dispatch.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::{MemoryRequest, MemoryResponse, ipc_err, read_frame, write_frame};

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
    // Always unlink the socket file before binding.
    //
    // Rationale: the caller has just opened the DB with an exclusive flock,
    // so by definition it is the authoritative writer for this state-root.
    // Any socket file left behind belongs to a prior (now-dead) daemon.
    // An earlier version of this code tried to detect a live listener via
    // `UnixStream::connect`; that was racy against systemd-style restarts
    // where the previous process was still draining its listen queue, and
    // would falsely report "socket in use" — leaving the new daemon
    // without an IPC server while meetings kept falling back to direct open.
    let _ = std::fs::remove_file(&socket_path);
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
