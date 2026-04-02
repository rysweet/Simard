pub mod agent_goal_assignment;
pub mod agent_program;
pub mod agent_roles;
pub mod agent_supervisor;
pub mod base_type_claude_agent_sdk;
pub mod base_type_copilot;
pub mod base_type_harness;
pub mod base_type_ms_agent;
pub mod base_type_pending_sdk;
pub mod base_type_rustyclawd;
pub mod base_type_turn;
pub mod base_types;
pub mod bootstrap;
pub mod bridge;
pub mod bridge_circuit;
pub mod bridge_launcher;
pub mod bridge_subprocess;
pub mod cmd_install;
pub mod cmd_self_update;
mod copilot_status_probe;
mod copilot_task_submit;
pub mod engineer_loop;
pub mod error;
pub mod evidence;
pub mod goal_curation;
pub mod goals;
pub mod greeting_banner;
pub mod gym;
pub mod gym_bridge;
pub mod gym_scoring;
pub mod handoff;
pub mod identity;
pub mod identity_auth;
pub mod identity_composition;
pub mod improvements;
pub mod knowledge_bridge;
pub mod knowledge_context;
pub mod meeting_facilitator;
pub mod meeting_repl;
pub mod meetings;
pub mod memory;
pub mod memory_bridge;
pub mod memory_bridge_adapter;
pub mod memory_cognitive;
pub mod memory_consolidation;
pub mod memory_hive;
pub mod metadata;
pub mod ooda_actions;
pub mod ooda_loop;
pub mod ooda_scheduler;
pub mod operator_cli;
pub mod operator_commands;
mod operator_commands_engineer;
mod operator_commands_gym;
mod operator_commands_meeting;
mod operator_commands_ooda;
mod operator_commands_review;
mod operator_commands_terminal;
mod persistence;
pub mod prompt_assets;
pub mod reflection;
pub mod remote_azlin;
pub mod remote_session;
pub mod remote_transfer;
pub mod research_tracker;
pub mod review;
pub mod runtime;
mod sanitization;
pub mod self_improve;
pub mod self_relaunch;
pub mod session;
pub mod skill_builder;
pub mod terminal_engineer_bridge;
mod terminal_session;
#[doc(hidden)]
pub mod test_support;

pub use agent_goal_assignment::{
    SubordinateProgress, assign_goal, poll_progress, read_assigned_goal, report_progress,
};
pub use agent_program::{
    AgentProgram, AgentProgramContext, AgentProgramMemoryRecord, ImprovementCuratorProgram,
    MeetingFacilitatorProgram, ObjectiveRelayProgram,
};
pub use agent_roles::{AgentRole, identity_for_role, role_for_objective};
pub use agent_supervisor::{
    HeartbeatStatus, SubordinateConfig, SubordinateHandle, check_heartbeat, kill_subordinate,
    spawn_subordinate,
};
pub use base_type_claude_agent_sdk::{ClaudeAgentSdkAdapter, claude_agent_sdk_adapter};
pub use base_type_copilot::{CopilotAdapterConfig, CopilotSdkAdapter, parse_copilot_response};
pub use base_type_harness::{HarnessConfig, RealLocalHarnessAdapter};
pub use base_type_ms_agent::{MsAgentFrameworkAdapter, ms_agent_framework_adapter};
pub use base_type_pending_sdk::PendingSdkAdapter;
pub use base_type_rustyclawd::RustyClawdAdapter;
pub use base_type_turn::{
    ProposedAction, TurnContext, TurnOutput, format_turn_input, parse_turn_output,
    prepare_turn_context,
};
pub use base_types::{
    BaseTypeCapability, BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeOutcome,
    BaseTypeSession, BaseTypeSessionRequest, BaseTypeTurnInput, capability_set,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
    joined_prompt_ids, standard_session_capabilities,
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
pub use goal_curation::{
    ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS, add_active_goal,
    add_backlog_item, archive_completed, load_goal_board, persist_board, promote_to_active,
    update_goal_progress,
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
pub use identity_auth::{
    AuthIdentity, DualIdentityConfig, env_for_identity, identity_for_operation,
    validate_identity_for_operation,
};
pub use identity_composition::{
    CompositeIdentity, SubordinateIdentity, compose_identity, max_subordinate_depth,
};
pub use improvements::{
    ImprovementPromotionPlan, PersistedImprovementApproval, PersistedImprovementRecord,
    render_review_context_directives,
};
pub use knowledge_bridge::{
    KnowledgeBridge, KnowledgePackInfo, KnowledgeQueryResult, KnowledgeSource,
};
pub use knowledge_context::{PlanningContext, enrich_planning_context};
pub use meeting_facilitator::{
    ActionItem, MeetingDecision, MeetingSession, MeetingSessionStatus, add_note, close_meeting,
    record_action_item, record_decision, start_meeting,
};
pub use meeting_repl::{MeetingCommand, parse_meeting_command, run_meeting_repl};
pub use meetings::{
    PersistedMeetingGoalUpdate, PersistedMeetingRecord, looks_like_persisted_meeting_record,
};
pub use memory::{
    FileBackedMemoryStore, InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore,
    SqliteMemoryStore,
};
pub use memory_bridge::CognitiveMemoryBridge;
pub use memory_bridge_adapter::CognitiveBridgeMemoryStore;
pub use memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective,
    CognitiveSensoryItem, CognitiveStatistics, CognitiveWorkingSlot,
};
pub use memory_consolidation::{
    FactExtraction, PreparedContext, execution_memory_operations, intake_memory_operations,
    persistence_memory_operations, preparation_memory_operations, reflection_memory_operations,
};
pub use memory_hive::{HiveConfig, hive_config_from_identity};
pub use ooda_actions::dispatch_actions;
pub use ooda_loop::{
    ActionKind, ActionOutcome, CycleReport, GoalSnapshot, Observation, OodaBridges, OodaConfig,
    OodaPhase, OodaState, PlannedAction, Priority, act, decide, observe, orient, run_ooda_cycle,
    summarize_cycle_report,
};
pub use ooda_scheduler::{
    CompletedSlot, ScheduledAction, Scheduler, SchedulerSlot, SlotStatus, complete_slot,
    drain_finished, fail_slot, poll_slots, schedule_actions, scheduler_summary, start_slot,
};
pub use test_support::TestAdapter;

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
pub use remote_azlin::{AzlinConfig, AzlinExecutor, AzlinVm, RealAzlinExecutor};
pub use remote_session::{RemoteConfig, RemoteSession, RemoteStatus};
pub use remote_transfer::MemorySnapshot;
pub use research_tracker::{
    DeveloperWatch, ResearchStatus, ResearchTopic, ResearchTracker, add_research_topic,
    load_research_topics, track_developer, update_topic_status,
};
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
pub use self_improve::{
    ImprovementConfig, ImprovementCycle, ImprovementDecision, ImprovementPhase, ProposedChange,
    run_improvement_cycle, summarize_cycle,
};
pub use self_relaunch::{
    GateResult, RelaunchConfig, RelaunchGate, all_gates_passed, build_canary, default_gates,
    handover, verify_canary,
};
pub use session::{
    SessionId, SessionIdGenerator, SessionPhase, SessionRecord, UuidSessionIdGenerator,
};
pub use skill_builder::{
    SkillTemplate, extract_skill_candidates, generate_skill_definition, install_skill,
    list_installed_skills,
};
pub use terminal_engineer_bridge::{
    ENGINEER_HANDOFF_FILE_NAME, ENGINEER_MODE_BOUNDARY, SHARED_DEFAULT_STATE_ROOT_SOURCE,
    SHARED_EXPLICIT_STATE_ROOT_SOURCE, TERMINAL_HANDOFF_FILE_NAME, TERMINAL_MODE_BOUNDARY,
    TerminalBridgeContext,
};
