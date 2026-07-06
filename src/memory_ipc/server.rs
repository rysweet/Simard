//! Server: spawn_server + ServerHandle + serve_connection + dispatch.

use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::{MemoryRequest, MemoryResponse, ipc_err, read_frame, write_frame};
// TDD (RED) for issue #2679: the authoritative server-side write-boundary gate
// (StoreFactGated dispatch arm). The gate + response variants are added in the
// implementation step; until then the unresolved symbols in this module are the
// red signal. `#[cfg(test)]` so production builds never compile it.
#[cfg(test)]
mod server_gate_tests;

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
        // Restrict the socket's parent directory to the owner (0700) so no other
        // local user can traverse to the memory socket (issue #2679 hardening).
        // Best-effort: a permissions failure must not prevent the daemon from
        // serving memory.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
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
    let listener = UnixListener::bind(&socket_path).map_err(|e| SimardError::RpcSpawnFailed {
        bridge: "memory-ipc".into(),
        reason: format!("bind {}: {e}", socket_path.display()),
    })?;
    // Restrict the socket file to owner read/write (0600) so only this user's
    // processes (daemon, meeting, engineer, distill) can send memory writes
    // (issue #2679 hardening). Best-effort.
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));
    }

    let socket_clone = socket_path.clone();
    let mem = Arc::clone(&memory);
    let join = thread::Builder::new()
        .name("memory-ipc-server".into())
        .spawn(move || {
            for conn in listener.incoming() {
                match conn {
                    Ok(stream) => {
                        let m = Arc::clone(&mem);
                        if let Err(e) =
                            thread::Builder::new()
                                .name("memory-ipc-conn".into())
                                .spawn(move || {
                                    if let Err(e) = serve_connection(stream, m) {
                                        eprintln!("[simard] memory-ipc: connection error: {e}");
                                    }
                                })
                        {
                            crate::cognitive_memory::metrics::increment(
                                "ipc_spawn_failed",
                                "spawn_server:per_conn",
                            );
                            eprintln!("[simard] memory-ipc: failed to spawn handler thread: {e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("[simard] memory-ipc: accept failed: {e}");
                        break;
                    }
                }
            }
        })
        .map_err(|e| SimardError::RpcSpawnFailed {
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
        MemoryRequest::StoreFactGated {
            concept,
            content,
            // The client's confidence is a hint the server must NOT trust; the
            // gate re-derives it from the shared reliability scorer below.
            confidence: _client_hint,
            tags,
            source_id,
            source_episode_ids,
            pass_id,
        } => gated_fact_write(
            memory,
            &concept,
            &content,
            &tags,
            &source_id,
            &source_episode_ids,
            &pass_id,
        ),
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
        MemoryRequest::StoreProcedureProvenance {
            name,
            steps,
            prerequisites,
            source_episode_ids,
            pass_id: _,
        } => match memory.store_procedure_with_provenance(
            &name,
            &steps,
            &prerequisites,
            &source_episode_ids,
        ) {
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
        MemoryRequest::ResolveProspective { node_id } => {
            match memory.resolve_prospective(&node_id) {
                Ok(()) => MemoryResponse::Ack,
                Err(e) => MemoryResponse::Error(e.to_string()),
            }
        }
        MemoryRequest::SearchEpisodesByKeywords { keywords, limit } => {
            match memory.search_episodes_by_keywords(&keywords, limit) {
                Ok(v) => MemoryResponse::Episodes(v),
                Err(e) => MemoryResponse::Error(e.to_string()),
            }
        }
        MemoryRequest::DrainPassLedger { pass_id } => {
            MemoryResponse::Count(drain_pass_ledger(&pass_id))
        }
        MemoryRequest::GetStatistics => match memory.get_statistics() {
            Ok(s) => MemoryResponse::Statistics(s),
            Err(e) => MemoryResponse::Error(e.to_string()),
        },
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Distillation write ledger (issue #2679)
// ───────────────────────────────────────────────────────────────────────────
//
// A per-`pass_id` count of facts the write-boundary gate ACCEPTED, so the
// distiller subprocess — which gets NO returned document (facts are agent
// writes) — can report how many facts a pass committed. Best-effort telemetry
// state: bounded by drain-on-read and never surfaced as an error.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn pass_ledger() -> &'static Mutex<HashMap<String, u32>> {
    static LEDGER: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    LEDGER.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record one gate-accepted fact for `pass_id`. No-op for an empty `pass_id`
/// (a caller that does not participate in the ledger).
fn ledger_record_stored(pass_id: &str) {
    if pass_id.is_empty() {
        return;
    }
    if let Ok(mut guard) = pass_ledger().lock() {
        *guard.entry(pass_id.to_string()).or_insert(0) += 1;
    }
}

/// Remove and return the accepted-fact count for `pass_id` (0 if unknown).
fn drain_pass_ledger(pass_id: &str) -> usize {
    pass_ledger()
        .lock()
        .ok()
        .and_then(|mut g| g.remove(pass_id))
        .unwrap_or(0) as usize
}

/// The single authoritative distillation write-boundary gate (issue #2679).
///
/// Applied per fact when the distiller agentic step commits a fact through the
/// daemon socket. The server — NOT the client, NOT Simard's distillation module —
/// decides every fact's disposition here, in order:
///
///   1. **Ground** the fact by confirming at least one `source_episode_id`
///      resolves to a real episode node in the store (store-existence check).
///   2. **Score** it with the shared [`crate::fact_reliability`] scorer using that
///      resolved `grounded` flag — NEVER the client's `confidence` hint.
///   3. **Quarantine** anything below `RELIABILITY_THRESHOLD` (ungrounded,
///      empty-content, off-spec) — writing nothing.
///   4. **Dedup**: never let a weaker-or-equal restatement clobber an existing
///      equal-or-stronger fact of the same identity (concept + content).
///   5. **Persist** survivors via `store_fact_with_provenance` with the
///      *server-computed* confidence and the source-episode provenance edges.
///
/// The disposition flows back as [`MemoryResponse::FactWrite`] — there is no
/// document for Simard to deserialize anywhere in the path.
fn gated_fact_write(
    memory: &dyn CognitiveMemoryOps,
    concept: &str,
    content: &str,
    tags: &[String],
    source_id: &str,
    source_episode_ids: &[String],
    pass_id: &str,
) -> MemoryResponse {
    use crate::fact_reliability::{RELIABILITY_THRESHOLD, score_fact_reliability};

    let quarantine = |confidence: f64| {
        MemoryResponse::FactWrite(super::FactWriteOutcome {
            stored: false,
            quarantined: true,
            confidence,
            node_id: None,
        })
    };

    // (0) Input validation at the boundary (issue #2679 hardening). Every field
    // is opaque data; a required field that is empty, or a field that exceeds its
    // length cap, is rejected (quarantined — nothing stored) rather than
    // truncated silently. `MAX_FRAME` already bounds the whole request; these
    // per-field caps bound each value within it.
    const MAX_CONCEPT_LEN: usize = 256;
    const MAX_CONTENT_LEN: usize = 64 * 1024;
    if concept.trim().is_empty()
        || content.trim().is_empty()
        || concept.len() > MAX_CONCEPT_LEN
        || content.len() > MAX_CONTENT_LEN
    {
        return quarantine(0.0);
    }

    // (1) Grounding — the fact is grounded iff at least one cited episode id
    // resolves to a real node in this store. A lookup error is treated as
    // "does not resolve" (fail-closed), so a backend hiccup can never
    // accidentally promote an ungrounded fact.
    let grounded = source_episode_ids
        .iter()
        .any(|id| memory.episode_exists(id).unwrap_or(false));

    // (2) Score with the shared scorer — the client's confidence is ignored.
    let confidence = score_fact_reliability(concept, content, grounded);

    // (3) Threshold quarantine.
    if confidence < RELIABILITY_THRESHOLD {
        return quarantine(confidence);
    }

    // (4) Identity dedup: do not downgrade/duplicate an equal-or-stronger prior
    // version of the *same* fact (concept + content). `search_facts` is queried
    // with the new confidence as `min_confidence` so it returns only priors
    // strong enough to block; the explicit `>=` is belt-and-suspenders against a
    // backend that ignores the filter.
    let new_content = content.trim();
    let existing = memory
        .search_facts(concept, 5, confidence)
        .unwrap_or_default();
    if existing
        .iter()
        .any(|f| f.content.trim() == new_content && f.confidence >= confidence)
    {
        return quarantine(confidence);
    }

    // (5) Persist with the server-computed confidence and provenance edges.
    match memory.store_fact_with_provenance(
        concept,
        content,
        confidence,
        source_id,
        Some(tags),
        None,
        source_episode_ids,
    ) {
        Ok(node_id) => {
            // Record the gate-accepted fact against the pass ledger so the
            // distiller can report how many facts a pass committed.
            ledger_record_stored(pass_id);
            MemoryResponse::FactWrite(super::FactWriteOutcome {
                stored: true,
                quarantined: false,
                confidence,
                node_id: Some(node_id),
            })
        }
        Err(e) => MemoryResponse::Error(e.to_string()),
    }
}
