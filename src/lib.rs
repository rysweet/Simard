pub mod agent_program;
pub mod base_types;
pub mod bootstrap;
pub mod engineer_loop;
pub mod error;
pub mod evidence;
pub mod goals;
pub mod gym;
pub mod handoff;
pub mod identity;
pub mod memory;
pub mod metadata;
mod persistence;
pub mod prompt_assets;
pub mod reflection;
pub mod review;
pub mod runtime;
mod sanitization;
pub mod session;
mod terminal_session;

pub use agent_program::{
    AgentProgram, AgentProgramContext, AgentProgramMemoryRecord, MeetingFacilitatorProgram,
    ObjectiveRelayProgram,
};
pub use base_types::{
    BaseTypeCapability, BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeOutcome,
    BaseTypeSession, BaseTypeSessionRequest, BaseTypeTurnInput, LocalProcessHarnessAdapter,
    RustyClawdAdapter, TerminalShellAdapter, capability_set,
};
pub use bootstrap::{
    BootstrapConfig, BootstrapInputs, BootstrapMode, ConfigValue, ConfigValueSource,
    LocalSessionExecution, assemble_local_runtime, assemble_local_runtime_from_handoff,
    bootstrap_entrypoint, builtin_base_type_registry_for_manifest, latest_local_handoff,
    run_local_session,
};
pub use engineer_loop::{
    EngineerLoopRun, ExecutedEngineerAction, RepoInspection, SelectedEngineerAction,
    VerificationReport, run_local_engineer_loop,
};
pub use error::{SimardError, SimardResult};
pub use evidence::{
    EvidenceRecord, EvidenceSource, EvidenceStore, FileBackedEvidenceStore, InMemoryEvidenceStore,
};
pub use goals::{
    FileBackedGoalStore, GoalRecord, GoalStatus, GoalStore, GoalUpdate, InMemoryGoalStore,
};
pub use gym::{
    BenchmarkArtifactPaths, BenchmarkCheckResult, BenchmarkRunReport, BenchmarkScenario,
    BenchmarkSuiteReport, BenchmarkSuiteScenarioSummary, benchmark_scenarios, default_output_root,
    run_benchmark_scenario, run_benchmark_suite,
};
pub use handoff::{
    FileBackedHandoffStore, InMemoryHandoffStore, RuntimeHandoffSnapshot, RuntimeHandoffStore,
};
pub use identity::{
    BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader, IdentityManifest, ManifestContract,
    MemoryPolicy, OperatingMode,
};
pub use memory::{
    FileBackedMemoryStore, InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore,
};
pub use metadata::{BackendDescriptor, Freshness, FreshnessState, Provenance};
pub use prompt_assets::{
    FilePromptAssetStore, InMemoryPromptAssetStore, PromptAsset, PromptAssetId, PromptAssetRef,
    PromptAssetStore,
};
pub use reflection::{ReflectionReport, ReflectionSnapshot, ReflectiveRuntime};
pub use review::{
    ImprovementProposal, ReviewArtifact, ReviewRequest, ReviewSignal, ReviewTargetKind,
    build_review_artifact, latest_review_artifact, load_review_artifact, persist_review_artifact,
    render_review_text, review_artifacts_dir,
};
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
