pub mod agent_program;
pub mod base_type_copilot;
pub mod base_type_harness;
pub mod base_type_turn;
pub mod base_types;
pub mod bootstrap;
pub mod bridge;
pub mod bridge_circuit;
pub mod bridge_subprocess;
mod copilot_status_probe;
mod copilot_task_submit;
pub mod engineer_loop;
pub mod error;
pub mod evidence;
pub mod goals;
pub mod gym;
pub mod gym_bridge;
pub mod gym_scoring;
pub mod handoff;
pub mod identity;
pub mod improvements;
pub mod knowledge_bridge;
pub mod knowledge_context;
pub mod meetings;
pub mod memory;
pub mod memory_bridge;
pub mod memory_cognitive;
pub mod memory_consolidation;
pub mod memory_hive;
pub mod metadata;
pub mod operator_cli;
pub mod operator_commands;
mod persistence;
pub mod prompt_assets;
pub mod reflection;
pub mod review;
pub mod runtime;
mod sanitization;
pub mod session;
pub mod terminal_engineer_bridge;
mod terminal_session;

pub use agent_program::{
    AgentProgram, AgentProgramContext, AgentProgramMemoryRecord, ImprovementCuratorProgram,
    MeetingFacilitatorProgram, ObjectiveRelayProgram,
};
pub use base_type_copilot::{CopilotAdapterConfig, CopilotSdkAdapter, parse_copilot_response};
pub use base_type_harness::{HarnessConfig, RealLocalHarnessAdapter};
pub use base_type_turn::{
    ProposedAction, TurnContext, TurnOutput, format_turn_input, parse_turn_output,
    prepare_turn_context,
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
pub use bridge::{
    BridgeErrorPayload, BridgeHealth, BridgeId, BridgeRequest, BridgeResponse, BridgeTransport,
    new_request_id, unpack_bridge_response,
};
pub use bridge_circuit::{CircuitBreakerConfig, CircuitBreakerTransport, CircuitState};
pub use bridge_subprocess::{InMemoryBridgeTransport, SubprocessBridgeTransport};
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
    BenchmarkArtifactPaths, BenchmarkCheckResult, BenchmarkComparisonArtifactPaths,
    BenchmarkComparisonDelta, BenchmarkComparisonReport, BenchmarkComparisonRunSummary,
    BenchmarkComparisonStatus, BenchmarkRunReport, BenchmarkScenario, BenchmarkSuiteReport,
    BenchmarkSuiteScenarioSummary, benchmark_scenarios, compare_latest_benchmark_runs,
    default_output_root, run_benchmark_scenario, run_benchmark_suite,
};
pub use gym_bridge::{GymBridge, GymScenario, GymScenarioResult, GymSuiteResult, ScoreDimensions};
pub use gym_scoring::{
    DimensionTrend, GymSuiteScore, ImprovementTrend, Regression, RegressionSeverity,
    TrendDirection, aggregate_suite_scores, detect_regression, suite_score_from_result,
    track_improvement,
};
pub use handoff::{
    CopilotSubmitAudit, FileBackedHandoffStore, InMemoryHandoffStore, RuntimeHandoffSnapshot,
    RuntimeHandoffStore,
};
pub use identity::{
    BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader, IdentityManifest, ManifestContract,
    MemoryPolicy, OperatingMode,
};
pub use improvements::{
    ImprovementPromotionPlan, PersistedImprovementApproval, PersistedImprovementRecord,
    render_review_context_directives,
};
pub use knowledge_bridge::{
    KnowledgeBridge, KnowledgePackInfo, KnowledgeQueryResult, KnowledgeSource,
};
pub use knowledge_context::{PlanningContext, enrich_planning_context};
pub use meetings::{
    PersistedMeetingGoalUpdate, PersistedMeetingRecord, looks_like_persisted_meeting_record,
};
pub use memory::{
    FileBackedMemoryStore, InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore,
};
pub use memory_bridge::CognitiveMemoryBridge;
pub use memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective,
    CognitiveSensoryItem, CognitiveStatistics, CognitiveWorkingSlot,
};
pub use memory_consolidation::{
    FactExtraction, PreparedContext, execution_memory_operations, intake_memory_operations,
    persistence_memory_operations, preparation_memory_operations, reflection_memory_operations,
};
pub use memory_hive::{HiveConfig, hive_config_from_identity};
pub use metadata::{BackendDescriptor, Freshness, FreshnessState, Provenance};
pub use operator_cli::{dispatch_operator_cli, operator_cli_help, operator_cli_usage};
pub use operator_commands::{
    dispatch_legacy_gym_cli, dispatch_operator_probe, gym_usage, run_bootstrap_probe,
    run_copilot_submit_probe, run_engineer_loop_probe, run_engineer_read_probe,
    run_goal_curation_probe, run_goal_curation_read_probe, run_gym_compare, run_gym_list,
    run_gym_scenario, run_gym_suite, run_handoff_probe, run_improvement_curation_probe,
    run_improvement_curation_read_probe, run_meeting_probe, run_meeting_read_probe,
    run_review_probe, run_review_read_probe, run_terminal_probe, run_terminal_read_probe,
};
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
pub use terminal_engineer_bridge::{
    ENGINEER_HANDOFF_FILE_NAME, ENGINEER_MODE_BOUNDARY, SHARED_DEFAULT_STATE_ROOT_SOURCE,
    SHARED_EXPLICIT_STATE_ROOT_SOURCE, TERMINAL_HANDOFF_FILE_NAME, TERMINAL_MODE_BOUNDARY,
    TerminalBridgeContext,
};
