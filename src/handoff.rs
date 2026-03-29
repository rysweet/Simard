use std::sync::Mutex;

use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::evidence::EvidenceRecord;
use crate::memory::MemoryRecord;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology};
use crate::session::SessionRecord;

#[derive(Clone, Debug, Eq, PartialEq)]
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
