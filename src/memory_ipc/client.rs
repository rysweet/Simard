//! Client: RemoteCognitiveMemory implementing CognitiveMemoryOps over Unix socket.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

use super::{FactWriteOutcome, MemoryRequest, MemoryResponse, ipc_err, read_frame, write_frame};

// ============================================================================
// Client
// ============================================================================

/// Client implementing [`CognitiveMemoryOps`] over the daemon's Unix socket.
pub struct RemoteCognitiveMemory {
    // Mutex because trait methods take &self but the socket is stateful.
    stream: Mutex<UnixStream>,
    socket_path: PathBuf,
}

/// A framed exchange failure tagged with the phase it occurred in, so the
/// single-reconnect logic (issue #4929) can tell an unapplied pre-delivery
/// failure (safe to re-send any request) apart from a post-delivery failure
/// (the request may already be applied — only idempotent requests may re-send).
enum ExchangeError {
    /// `write_frame` failed: the request never reached the server intact and was
    /// therefore never dispatched. Safe to reconnect and re-send any request.
    PreDelivery(SimardError),
    /// The request bytes were fully written but reading/parsing the response
    /// failed. The server may have already applied the request.
    PostDelivery(SimardError),
}

impl ExchangeError {
    /// Unwrap to the underlying [`SimardError`] for surfacing to callers.
    fn into_inner(self) -> SimardError {
        match self {
            Self::PreDelivery(e) | Self::PostDelivery(e) => e,
        }
    }
}

impl RemoteCognitiveMemory {
    /// Connect to the daemon's memory socket. Returns an error if the socket
    /// doesn't exist, the daemon isn't listening, or the handshake fails.
    pub fn connect(socket_path: &Path) -> SimardResult<Self> {
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

    /// Socket path this client is connected to (for logging).
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    fn call(&self, req: MemoryRequest) -> SimardResult<MemoryResponse> {
        // Whether re-applying this request after a *possible* prior application
        // is safe (issue #4929 review). Reads, `Ping`, effect-idempotent
        // mutations, and the server-deduped `StoreFactGated` are safe; writes
        // that mint a fresh row, fire-once triggers, and destructive drains are
        // NOT — re-sending them could duplicate/corrupt state.
        let retry_safe = req.is_retry_safe();
        let bytes = serde_json::to_vec(&req).map_err(|e| ipc_err("serialize-request", e))?;
        let mut guard = self
            .stream
            .lock()
            .map_err(|e| ipc_err("lock-poisoned", e))?;

        // At-most-once reconnect on a transport failure (broken pipe, EOF,
        // reset) — issue #4929. The daemon journal used to fill with `write-len:
        // Broken pipe` because a severed `UnixStream` poisoned this client
        // permanently. The retry decision depends on WHERE the failure occurred:
        //
        //   * PRE-delivery (`write_frame` failed): the request never reached the
        //     server intact, so it was NEVER dispatched. Reconnecting and
        //     re-sending is safe for ANY request kind — this is the original
        //     `write-len: Broken pipe` recovery path.
        //
        //   * POST-delivery (bytes written; response read/parse failed): the
        //     server MAY have already applied the request. Re-sending a
        //     non-idempotent write would DUPLICATE it (issue #4929 review), so
        //     only idempotent requests are re-sent. Non-idempotent requests heal
        //     the poisoned stream for subsequent calls but surface THIS call's
        //     error WITHOUT re-sending.
        //
        // Either way it is at-most-once: a failure on the retried exchange
        // surfaces `Err` — no retry loop, no silent fallback.
        match Self::exchange(&mut guard, &bytes) {
            Ok(resp) => Ok(resp),
            Err(ExchangeError::PreDelivery(first_err)) => {
                tracing::warn!(
                    endpoint = "memory-ipc",
                    socket_path = %self.socket_path.display(),
                    error = %first_err,
                    "memory-ipc write failed pre-delivery; reconnecting once and retrying"
                );
                let fresh = Self::reconnect(&self.socket_path)?;
                *guard = fresh;
                Self::exchange(&mut guard, &bytes).map_err(|retry_err| {
                    let retry_err = retry_err.into_inner();
                    tracing::error!(
                        endpoint = "memory-ipc",
                        socket_path = %self.socket_path.display(),
                        error = %retry_err,
                        "memory-ipc reconnect retry also failed; surfacing error"
                    );
                    retry_err
                })
            }
            Err(ExchangeError::PostDelivery(first_err)) if retry_safe => {
                tracing::warn!(
                    endpoint = "memory-ipc",
                    socket_path = %self.socket_path.display(),
                    error = %first_err,
                    "memory-ipc response failed after delivery; retrying idempotent request after reconnect"
                );
                let fresh = Self::reconnect(&self.socket_path)?;
                *guard = fresh;
                Self::exchange(&mut guard, &bytes).map_err(|retry_err| {
                    let retry_err = retry_err.into_inner();
                    tracing::error!(
                        endpoint = "memory-ipc",
                        socket_path = %self.socket_path.display(),
                        error = %retry_err,
                        "memory-ipc reconnect retry also failed; surfacing error"
                    );
                    retry_err
                })
            }
            Err(ExchangeError::PostDelivery(first_err)) => {
                // Non-idempotent request whose bytes were already delivered: the
                // server may have applied it, so we must NOT re-send. Reconnect
                // to heal the stream for the next call, then surface this error.
                tracing::warn!(
                    endpoint = "memory-ipc",
                    socket_path = %self.socket_path.display(),
                    error = %first_err,
                    "memory-ipc response failed after delivering a non-idempotent request; not re-sending to avoid a duplicate write"
                );
                if let Err(heal_err) = Self::reconnect(&self.socket_path).map(|fresh| {
                    *guard = fresh;
                }) {
                    tracing::warn!(
                        endpoint = "memory-ipc",
                        socket_path = %self.socket_path.display(),
                        error = %heal_err,
                        "memory-ipc stream heal after non-idempotent post-delivery failure also failed; next call will reconnect"
                    );
                }
                Err(first_err)
            }
        }
    }

    /// Write one framed request and read one framed response on `stream`. The
    /// failure is tagged with the phase it occurred in so [`call`](Self::call)
    /// can decide whether re-sending is safe (issue #4929): a `write_frame`
    /// failure is pre-delivery (never applied); a response read/parse failure is
    /// post-delivery (may already be applied).
    fn exchange(stream: &mut UnixStream, bytes: &[u8]) -> Result<MemoryResponse, ExchangeError> {
        write_frame(stream, bytes).map_err(ExchangeError::PreDelivery)?;
        let resp_bytes = read_frame(stream).map_err(ExchangeError::PostDelivery)?;
        let resp: MemoryResponse = serde_json::from_slice(&resp_bytes)
            .map_err(|e| ExchangeError::PostDelivery(ipc_err("parse-response", e)))?;
        Ok(resp)
    }

    /// Open a fresh connection to `socket_path` for the single allowed reconnect
    /// (no handshake — the caller either re-sends the request directly or, for a
    /// non-idempotent post-delivery failure, keeps the fresh stream for the next
    /// call). Timeouts mirror [`connect`](Self::connect) so a wedged daemon
    /// cannot hang the retry.
    fn reconnect(socket_path: &Path) -> SimardResult<UnixStream> {
        let stream = UnixStream::connect(socket_path).map_err(|e| SimardError::RpcSpawnFailed {
            endpoint: "memory-ipc-client".into(),
            reason: format!("reconnect {}: {e}", socket_path.display()),
        })?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
        Ok(stream)
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
