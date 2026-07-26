//! Client: RemoteCognitiveMemory implementing CognitiveMemoryOps over Unix socket.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use tracing::{debug, error, warn};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::cognitive_memory::metrics;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

use super::{
    FactWriteOutcome, MemoryRequest, MemoryResponse, ipc_err, is_broken_pipe, read_frame,
    write_frame, write_frame_raw,
};

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
        let stream = Self::connect_stream(socket_path)?;
        let client = Self {
            stream: Mutex::new(stream),
            socket_path: socket_path.to_path_buf(),
        };
        // Handshake
        match client.call(MemoryRequest::Ping)? {
            MemoryResponse::Pong => Ok(client),
            other => Err(SimardError::RpcSpawnFailed {
                endpoint: "memory-ipc-client".into(),
                reason: format!("handshake: expected Pong, got {other:?}"),
            }),
        }
    }

    /// Open a fresh timeout-configured stream to `socket_path` WITHOUT a Ping
    /// handshake.
    ///
    /// Factored out so the write-path reconnect ([`Self::reconnect`]) can reuse
    /// the exact connect + timeout setup while performing its handshake inline
    /// — breaking the `connect() -> call(Ping)` recursion that would otherwise
    /// occur if reconnect went back through [`Self::connect`]. The stored,
    /// immutable `socket_path` is the only source for reconnects, so a
    /// reconnect can never be redirected to a different (possibly more
    /// permissive) socket.
    fn connect_stream(socket_path: &Path) -> SimardResult<UnixStream> {
        if !socket_path.exists() {
            return Err(SimardError::RpcSpawnFailed {
                endpoint: "memory-ipc-client".into(),
                reason: format!("socket {} not present", socket_path.display()),
            });
        }
        let stream = UnixStream::connect(socket_path).map_err(|e| SimardError::RpcSpawnFailed {
            endpoint: "memory-ipc-client".into(),
            reason: format!("connect {}: {e}", socket_path.display()),
        })?;
        // Short timeouts so a wedged daemon doesn't hang meeting forever.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
        Ok(stream)
    }

    /// Re-establish the connection in place after a mid-write peer reset (issue
    /// #4731). Opens a fresh stream via [`Self::connect_stream`], performs an
    /// INLINE Ping/Pong handshake on it (never via [`Self::call`], to avoid
    /// recursion), and only on a verified `Pong` swaps the new stream into the
    /// held guard — deterministically dropping the stale stream.
    ///
    /// If the handshake returns anything other than `Pong`, the peer is
    /// unverified and this returns an error WITHOUT swapping, so the caller
    /// must not resend the real payload onto it.
    fn reconnect(&self, stream: &mut UnixStream) -> SimardResult<()> {
        let mut fresh = Self::connect_stream(&self.socket_path)?;
        let ping =
            serde_json::to_vec(&MemoryRequest::Ping).map_err(|e| ipc_err("serialize-ping", e))?;
        write_frame(&mut fresh, &ping)?;
        let resp_bytes = read_frame(&mut fresh)?;
        let resp: MemoryResponse =
            serde_json::from_slice(&resp_bytes).map_err(|e| ipc_err("parse-pong", e))?;
        match resp {
            MemoryResponse::Pong => {
                *stream = fresh;
                Ok(())
            }
            other => Err(SimardError::RpcTransportError {
                endpoint: "memory-ipc".into(),
                reason: format!("reconnect handshake: expected Pong, got {other:?}"),
            }),
        }
    }

    /// Socket path this client is connected to (for logging).
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    fn call(&self, req: MemoryRequest) -> SimardResult<MemoryResponse> {
        // Bounded write-half reconnect+retry (issue #4731). The peer can close
        // the socket mid-write under load, so a single large frame write hits
        // EPIPE. Because a write-half EPIPE means the server never received or
        // committed the request, it is safe to reconnect and idempotently
        // re-send. Read-half failures are NEVER retried (the server may have
        // already persisted). On exhaustion we surface `RpcTransportError` —
        // never a silent `Ok`, never a dropped write.
        const MAX_ATTEMPTS: usize = 3;
        const BACKOFF: Duration = Duration::from_millis(50);

        let bytes = serde_json::to_vec(&req).map_err(|e| ipc_err("serialize-request", e))?;
        let mut guard = self
            .stream
            .lock()
            .map_err(|e| ipc_err("lock-poisoned", e))?;

        let mut attempt = 1usize;
        loop {
            match write_frame_raw(&mut *guard, &bytes) {
                Ok(()) => {
                    // Write committed to the peer; read the response. Read-half
                    // errors propagate as-is (no retry — server may have
                    // persisted, retrying could duplicate the mutation).
                    let resp_bytes = read_frame(&mut *guard)?;
                    let resp: MemoryResponse = serde_json::from_slice(&resp_bytes)
                        .map_err(|e| ipc_err("parse-response", e))?;
                    return Ok(resp);
                }
                Err((phase, e)) => {
                    let broken = is_broken_pipe(&e);
                    if !(broken && attempt < MAX_ATTEMPTS) {
                        // Fail-closed: not a retriable broken pipe, or attempts
                        // exhausted. Surface the transport error; never drop
                        // the write silently. Diagnostics carry only transport
                        // metadata (endpoint, phase, errno) — never payload.
                        error!(
                            endpoint = "memory-ipc",
                            attempt,
                            max_attempts = MAX_ATTEMPTS,
                            phase,
                            error_kind = ?e.kind(),
                            errno = e.raw_os_error(),
                            "memory-ipc write failed terminally; surfacing transport error (no silent drop)"
                        );
                        if broken {
                            metrics::increment("epipe_exhausted", "memory-ipc");
                        }
                        return Err(ipc_err(phase, e));
                    }

                    warn!(
                        endpoint = "memory-ipc",
                        attempt,
                        max_attempts = MAX_ATTEMPTS,
                        phase,
                        error_kind = ?e.kind(),
                        errno = e.raw_os_error(),
                        "memory-ipc write hit broken pipe; reconnecting to retry"
                    );
                    metrics::increment("epipe_reconnect", "memory-ipc");

                    // Reconnect + verified handshake, swapping the fresh stream
                    // into the held guard. A failed/unverified reconnect is
                    // surfaced immediately and the payload is NOT resent.
                    if let Err(reconnect_err) = self.reconnect(&mut guard) {
                        error!(
                            endpoint = "memory-ipc",
                            attempt,
                            "memory-ipc reconnect failed; surfacing transport error (no silent drop)"
                        );
                        return Err(reconnect_err);
                    }

                    debug!(
                        endpoint = "memory-ipc",
                        attempt,
                        next_attempt = attempt + 1,
                        "memory-ipc reconnected; retrying write after backoff"
                    );
                    thread::sleep(BACKOFF);
                    attempt += 1;
                }
            }
        }
    }

    fn unexpected(name: &str, got: MemoryResponse) -> SimardError {
        match got {
            MemoryResponse::Error(msg) => SimardError::RpcCallFailed {
                endpoint: "memory-ipc".into(),
                method: name.into(),
                reason: msg,
            },
            other => SimardError::RpcCallFailed {
                endpoint: "memory-ipc".into(),
                method: name.into(),
                reason: format!("unexpected response variant: {other:?}"),
            },
        }
    }

    /// Commit ONE distilled fact through the daemon's authoritative
    /// write-boundary gate (issue #2679). The server grounds, scores,
    /// quarantines, dedups, and persists the fact; the returned
    /// [`FactWriteOutcome`] reports the server's disposition and the confidence
    /// it computed (never the `confidence` hint passed here). This is the wire
    /// the distiller agentic step uses via `simard memory remember` — there is
    /// no return document for anyone to deserialize.
    #[allow(clippy::too_many_arguments)]
    pub fn remember_fact_gated(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
        source_episode_ids: &[String],
        pass_id: &str,
    ) -> SimardResult<FactWriteOutcome> {
        match self.call(MemoryRequest::StoreFactGated {
            concept: concept.into(),
            content: content.into(),
            confidence,
            tags: tags.to_vec(),
            source_id: source_id.into(),
            source_episode_ids: source_episode_ids.to_vec(),
            pass_id: pass_id.into(),
        })? {
            MemoryResponse::FactWrite(o) => Ok(o),
            other => Err(Self::unexpected("remember_fact_gated", other)),
        }
    }

    /// Commit ONE distilled procedure with its source-episode provenance (issue
    /// #2679), returning the new node id. The companion of
    /// [`remember_fact_gated`](Self::remember_fact_gated) for the distiller's
    /// procedure output.
    pub fn remember_procedure_provenance(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
        source_episode_ids: &[String],
        pass_id: &str,
    ) -> SimardResult<String> {
        match self.call(MemoryRequest::StoreProcedureProvenance {
            name: name.into(),
            steps: steps.to_vec(),
            prerequisites: prerequisites.to_vec(),
            source_episode_ids: source_episode_ids.to_vec(),
            pass_id: pass_id.into(),
        })? {
            MemoryResponse::Id(id) => Ok(id),
            other => Err(Self::unexpected("remember_procedure_provenance", other)),
        }
    }

    /// Drain and return the count of facts the write-boundary gate ACCEPTED for
    /// `pass_id` (issue #2679). Used by the distiller subprocess to report a
    /// pass's committed-fact count when there is no returned document to count.
    pub fn drain_pass_ledger(&self, pass_id: &str) -> SimardResult<usize> {
        match self.call(MemoryRequest::DrainPassLedger {
            pass_id: pass_id.into(),
        })? {
            MemoryResponse::Count(n) => Ok(n),
            other => Err(Self::unexpected("drain_pass_ledger", other)),
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

    fn recall_facts_ranked(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
        weights: crate::cognitive_memory::RecallWeightSet,
    ) -> SimardResult<Vec<CognitiveFact>> {
        // Additive socket forward (issue #2329, mirroring #2627): forward the
        // library's six-signal ranked recall over the wire instead of inheriting
        // the trait default, which would degrade to gated `search_facts` and
        // silently strip phase-weighted ranking + `recall_precision_at_k` on the
        // production daemon path. The server dispatches this to
        // `LibraryCognitiveMemory::recall_facts_ranked`.
        match self.call(MemoryRequest::RecallFactsRanked {
            query: query.into(),
            limit,
            min_confidence,
            weights,
        })? {
            MemoryResponse::Facts(v) => Ok(v),
            other => Err(Self::unexpected("recall_facts_ranked", other)),
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

    fn list_all_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        // Additive socket forward (issue #2627): mirror the library override so a
        // reader on the daemon-socket tier enumerates live episodes instead of the
        // empty `list_all_episodes` trait default — the dashboard Memory-tab graph
        // relies on this to render per-item episode nodes over the wire.
        match self.call(MemoryRequest::ListAllEpisodes { limit })? {
            MemoryResponse::Episodes(v) => Ok(v),
            other => Err(Self::unexpected("list_all_episodes", other)),
        }
    }

    fn list_all_prospective(&self, limit: u32) -> SimardResult<Vec<CognitiveProspective>> {
        // Additive socket forward (issue #2627): companion of `list_all_episodes`
        // so prospective memories enumerate over the wire for the Memory-tab
        // graph instead of collapsing to the empty trait default.
        match self.call(MemoryRequest::ListAllProspective { limit })? {
            MemoryResponse::Prospectives(v) => Ok(v),
            other => Err(Self::unexpected("list_all_prospective", other)),
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

    fn resolve_prospective(&self, node_id: &str) -> SimardResult<()> {
        match self.call(MemoryRequest::ResolveProspective {
            node_id: node_id.into(),
        })? {
            MemoryResponse::Ack => Ok(()),
            other => Err(Self::unexpected("resolve_prospective", other)),
        }
    }

    fn list_prospective_by_trigger(
        &self,
        trigger: &str,
        limit: u32,
    ) -> SimardResult<Vec<CognitiveProspective>> {
        match self.call(MemoryRequest::ListProspectiveByTrigger {
            trigger: trigger.into(),
            limit,
        })? {
            MemoryResponse::Prospectives(v) => Ok(v),
            other => Err(Self::unexpected("list_prospective_by_trigger", other)),
        }
    }

    fn search_episodes_by_keywords(
        &self,
        keywords: &[String],
        limit: u32,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        match self.call(MemoryRequest::SearchEpisodesByKeywords {
            keywords: keywords.to_vec(),
            limit,
        })? {
            MemoryResponse::Episodes(v) => Ok(v),
            other => Err(Self::unexpected("search_episodes_by_keywords", other)),
        }
    }

    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        match self.call(MemoryRequest::GetStatistics)? {
            MemoryResponse::Statistics(s) => Ok(s),
            other => Err(Self::unexpected("get_statistics", other)),
        }
    }
}
