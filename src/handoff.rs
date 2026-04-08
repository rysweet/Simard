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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snapshot() -> RuntimeHandoffSnapshot {
        RuntimeHandoffSnapshot {
            exported_state: RuntimeState::Active,
            identity_name: "test-identity".to_string(),
            selected_base_type: BaseTypeId::new("local-harness"),
            topology: RuntimeTopology::SingleProcess,
            source_runtime_node: RuntimeNodeId::local(),
            source_mailbox_address: RuntimeAddress::new("inmemory://node-local"),
            session: None,
            memory_records: Vec::new(),
            evidence_records: Vec::new(),
            copilot_submit_audit: None,
        }
    }

    #[test]
    fn test_in_memory_store_initially_empty() {
        let store = InMemoryHandoffStore::try_default().unwrap();
        let latest = store.latest().unwrap();
        assert!(latest.is_none());
    }

    #[test]
    fn test_in_memory_store_save_and_latest() {
        let store = InMemoryHandoffStore::try_default().unwrap();
        let snapshot = make_snapshot();
        store.save(snapshot.clone()).unwrap();
        let latest = store.latest().unwrap().unwrap();
        assert_eq!(latest.identity_name, "test-identity");
        assert_eq!(latest.exported_state, RuntimeState::Active);
    }

    #[test]
    fn test_in_memory_store_overwrite() {
        let store = InMemoryHandoffStore::try_default().unwrap();
        let mut snap1 = make_snapshot();
        snap1.identity_name = "first".to_string();
        store.save(snap1).unwrap();

        let mut snap2 = make_snapshot();
        snap2.identity_name = "second".to_string();
        store.save(snap2).unwrap();

        let latest = store.latest().unwrap().unwrap();
        assert_eq!(latest.identity_name, "second");
    }

    #[test]
    fn test_in_memory_store_descriptor() {
        let store = InMemoryHandoffStore::try_default().unwrap();
        let desc = store.descriptor();
        assert_eq!(desc.identity, "handoff::in-memory");
    }

    #[test]
    fn test_copilot_submit_audit_default() {
        let audit = CopilotSubmitAudit::default();
        assert!(audit.flow_asset.is_empty());
        assert!(audit.payload_id.is_empty());
        assert!(audit.outcome.is_empty());
        assert!(audit.reason_code.is_none());
        assert!(audit.ordered_steps.is_empty());
        assert!(audit.observed_checkpoints.is_empty());
        assert!(audit.last_meaningful_output_line.is_none());
        assert!(audit.transcript_preview.is_empty());
    }

    #[test]
    fn test_copilot_submit_audit_serialize_deserialize() {
        let audit = CopilotSubmitAudit {
            flow_asset: "flow.json".to_string(),
            payload_id: "p-123".to_string(),
            outcome: "success".to_string(),
            reason_code: Some("RC001".to_string()),
            ordered_steps: vec!["step1".to_string(), "step2".to_string()],
            observed_checkpoints: vec!["cp1".to_string()],
            last_meaningful_output_line: Some("done".to_string()),
            transcript_preview: "preview text".to_string(),
        };
        let json = serde_json::to_string(&audit).unwrap();
        let deserialized: CopilotSubmitAudit = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.flow_asset, "flow.json");
        assert_eq!(deserialized.outcome, "success");
        assert_eq!(deserialized.ordered_steps.len(), 2);
    }

    #[test]
    fn test_snapshot_serialize_deserialize() {
        let snapshot = make_snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        let deserialized: RuntimeHandoffSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.identity_name, snapshot.identity_name);
        assert_eq!(deserialized.exported_state, snapshot.exported_state);
        assert!(deserialized.session.is_none());
        assert!(deserialized.copilot_submit_audit.is_none());
    }

    #[test]
    fn test_snapshot_with_copilot_audit() {
        let mut snapshot = make_snapshot();
        snapshot.copilot_submit_audit = Some(CopilotSubmitAudit {
            flow_asset: "flow.json".to_string(),
            payload_id: "p-1".to_string(),
            outcome: "ok".to_string(),
            ..CopilotSubmitAudit::default()
        });
        let json = serde_json::to_string(&snapshot).unwrap();
        let deserialized: RuntimeHandoffSnapshot = serde_json::from_str(&json).unwrap();
        assert!(deserialized.copilot_submit_audit.is_some());
        assert_eq!(
            deserialized.copilot_submit_audit.unwrap().flow_asset,
            "flow.json"
        );
    }

    #[test]
    fn test_snapshot_missing_copilot_audit_field_deserializes_as_none() {
        // Simulate old format without copilot_submit_audit (lowercase for serde rename_all)
        let json = serde_json::json!({
            "exported_state": "active",
            "identity_name": "test",
            "selected_base_type": "local-harness",
            "topology": "single-process",
            "source_runtime_node": "node-local",
            "source_mailbox_address": "inmemory://node-local",
            "session": null,
            "memory_records": [],
            "evidence_records": []
        });
        let snapshot: RuntimeHandoffSnapshot = serde_json::from_value(json).unwrap();
        assert!(snapshot.copilot_submit_audit.is_none());
    }

    #[test]
    fn test_file_backed_store_path() {
        let store = FileBackedHandoffStore::try_new("test_handoff_path_check.json").unwrap();
        assert_eq!(store.path(), Path::new("test_handoff_path_check.json"));
        // Clean up if file was created
        let _ = std::fs::remove_file("test_handoff_path_check.json");
    }
}
