pub mod base_types;
pub mod bootstrap;
pub mod error;
pub mod evidence;
pub mod identity;
pub mod memory;
pub mod metadata;
pub mod prompt_assets;
pub mod reflection;
pub mod runtime;
pub mod session;

pub use base_types::{
    BaseTypeAdapter, BaseTypeCapability, BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome,
    BaseTypeRequest, LocalProcessHarnessAdapter, capability_set,
};
pub use bootstrap::{
    BootstrapConfig, BootstrapInputs, BootstrapMode, ConfigValue, ConfigValueSource,
    LocalSessionExecution, assemble_local_runtime, bootstrap_entrypoint, run_local_session,
};
pub use error::{SimardError, SimardResult};
pub use evidence::{EvidenceRecord, EvidenceSource, EvidenceStore, InMemoryEvidenceStore};
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
    BaseTypeRegistry, LocalRuntime, RuntimePorts, RuntimeRequest, RuntimeState, RuntimeTopology,
    SessionOutcome,
};
pub use session::{
    SessionId, SessionIdGenerator, SessionPhase, SessionRecord, UuidSessionIdGenerator,
};
