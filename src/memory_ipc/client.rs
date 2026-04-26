//! Client: RemoteCognitiveMemory implementing CognitiveMemoryOps over Unix socket.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

use super::{MemoryRequest, MemoryResponse, ipc_err, read_frame, write_frame};

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
