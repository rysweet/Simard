pub mod agent_program;
pub mod base_types;
pub mod bootstrap;
pub mod error;
pub mod evidence;
pub mod handoff;
pub mod identity;
pub mod memory;
pub mod metadata;
pub mod prompt_assets;
pub mod reflection;
pub mod runtime;
mod sanitization;
pub mod session;

pub use agent_program::{AgentProgram, AgentProgramContext, ObjectiveRelayProgram};
pub use base_types::{
    BaseTypeCapability, BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeOutcome,
    BaseTypeSession, BaseTypeSessionRequest, BaseTypeTurnInput, LocalProcessHarnessAdapter,
    RustyClawdAdapter, capability_set,
};
pub use bootstrap::{
    BootstrapConfig, BootstrapInputs, BootstrapMode, ConfigValue, ConfigValueSource,
    LocalSessionExecution, assemble_local_runtime, bootstrap_entrypoint, run_local_session,
};
pub use error::{SimardError, SimardResult};
pub use evidence::{EvidenceRecord, EvidenceSource, EvidenceStore, InMemoryEvidenceStore};
pub use handoff::{InMemoryHandoffStore, RuntimeHandoffSnapshot, RuntimeHandoffStore};
pub use identity::{
    BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader, IdentityManifest, ManifestContract,
    MemoryPolicy, OperatingMode,
};
pub use memory::{InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
pub use metadata::{BackendDescriptor, Freshness, FreshnessState, Provenance};
pub use prompt_assets::{
    FilePromptAssetStore, InMemoryPromptAssetStore, PromptAsset, PromptAssetId, PromptAssetRef,
    PromptAssetStore,
};
pub use reflection::{ReflectionReport, ReflectionSnapshot, ReflectiveRuntime};
pub use runtime::{
    BaseTypeRegistry, CoordinatedSupervisor, InMemoryMailboxTransport, InProcessSupervisor,
    InProcessTopologyDriver, LocalRuntime, LoopbackMailboxTransport, LoopbackMeshTopologyDriver,
    RuntimeAddress, RuntimeKernel, RuntimeMailboxTransport, RuntimeNodeId, RuntimePorts,
    RuntimeRequest, RuntimeState, RuntimeSupervisor, RuntimeTopology, RuntimeTopologyDriver,
    SessionOutcome,
};
pub use session::{
    SessionId, SessionIdGenerator, SessionPhase, SessionRecord, UuidSessionIdGenerator,
};
