use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::evidence::EvidenceRecord;
use crate::memory::MemoryRecord;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::persistence::{load_json_or_default, persist_json};
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology};
use crate::session::SessionRecord;

const HANDOFF_STORE_NAME: &str = "handoff";

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CopilotSubmitAudit {
    pub flow_asset: String,
    pub payload_id: String,
    pub outcome: String,
    #[serde(default)]
    pub reason_code: Option<String>,
    #[serde(default)]
    pub ordered_steps: Vec<String>,
    #[serde(default, alias = "satisfied_checkpoints")]
    pub observed_checkpoints: Vec<String>,
    #[serde(default)]
    pub last_meaningful_output_line: Option<String>,
    #[serde(default)]
    pub transcript_preview: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeHandoffSnapshot {
    pub exported_state: RuntimeState,
    pub identity_name: String,
    pub selected_base_type: BaseTypeId,
    pub topology: RuntimeTopology,
    pub source_runtime_node: RuntimeNodeId,
    pub source_mailbox_address: RuntimeAddress,
    pub session: Option<SessionRecord>,
    pub memory_records: Vec<MemoryRecord>,
    pub evidence_records: Vec<EvidenceRecord>,
    #[serde(default)]
    pub copilot_submit_audit: Option<CopilotSubmitAudit>,
}

pub trait RuntimeHandoffStore: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    fn save(&self, snapshot: RuntimeHandoffSnapshot) -> SimardResult<()>;

    fn latest(&self) -> SimardResult<Option<RuntimeHandoffSnapshot>>;
}

#[derive(Debug)]
pub struct InMemoryHandoffStore {
    state: Mutex<Option<RuntimeHandoffSnapshot>>,
    descriptor: BackendDescriptor,
}

impl InMemoryHandoffStore {
    pub fn new(descriptor: BackendDescriptor) -> Self {
        Self {
            state: Mutex::new(None),
            descriptor,
        }
    }

    pub fn try_default() -> SimardResult<Self> {
        Ok(Self::new(BackendDescriptor::for_runtime_type::<Self>(
            "handoff::in-memory",
            "runtime-port:handoff-store",
            Freshness::now()?,
        )))
    }
}

#[derive(Debug)]
pub struct FileBackedHandoffStore {
    state: Mutex<Option<RuntimeHandoffSnapshot>>,
    path: PathBuf,
    descriptor: BackendDescriptor,
}

impl FileBackedHandoffStore {
    pub fn new(path: impl Into<PathBuf>, descriptor: BackendDescriptor) -> SimardResult<Self> {
        let path = path.into();
        Ok(Self {
            state: Mutex::new(load_json_or_default(HANDOFF_STORE_NAME, &path)?),
            path,
            descriptor,
        })
    }

    pub fn try_new(path: impl Into<PathBuf>) -> SimardResult<Self> {
        let path = path.into();
        Self::new(
            path,
            BackendDescriptor::for_runtime_type::<Self>(
                "handoff::json-file-store",
                "runtime-port:handoff-store:file-json",
                Freshness::now()?,
            ),
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn persist(&self, snapshot: &Option<RuntimeHandoffSnapshot>) -> SimardResult<()> {
        persist_json(HANDOFF_STORE_NAME, &self.path, snapshot)
    }
}

impl RuntimeHandoffStore for InMemoryHandoffStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn save(&self, snapshot: RuntimeHandoffSnapshot) -> SimardResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "handoff".to_string(),
            })?;
        *state = Some(snapshot);
        Ok(())
    }

    fn latest(&self) -> SimardResult<Option<RuntimeHandoffSnapshot>> {
        let state = self
            .state
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: "handoff".to_string(),
            })?;
        Ok(state.clone())
    }
}

impl RuntimeHandoffStore for FileBackedHandoffStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn save(&self, snapshot: RuntimeHandoffSnapshot) -> SimardResult<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: HANDOFF_STORE_NAME.to_string(),
            })?;
        *state = Some(snapshot);
        self.persist(&state)
    }

    fn latest(&self) -> SimardResult<Option<RuntimeHandoffSnapshot>> {
        let state = self
            .state
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: HANDOFF_STORE_NAME.to_string(),
            })?;
        Ok(state.clone())
    }
}
