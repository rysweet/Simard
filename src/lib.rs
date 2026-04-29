pub mod agent_goal_assignment;
pub mod agent_program;
pub mod agent_registry;
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
pub mod build_lock;
pub mod cmd_cleanup;
pub mod cmd_ensure_deps;
pub mod cmd_install;
pub mod cmd_self_update;
pub mod cognitive_memory;
mod copilot_status_probe;
mod copilot_task_submit;
pub mod cost_tracking;
pub mod engineer_loop;
pub mod engineer_plan;
pub mod engineer_worktree;
pub mod error;
pub mod eval_watchdog;
pub mod evidence;
pub mod git_guardrails;
pub mod goal_curation;
pub mod goals;
pub mod greeting_banner;
pub mod gym;
pub mod gym_bridge;
pub mod gym_history;
pub mod gym_scoring;
pub mod handoff;
pub mod hive_event_bus;
pub mod identity;
pub mod identity_auth;
pub mod identity_composition;
pub mod identity_precedence;
pub mod improvements;
pub mod knowledge_bridge;
pub mod knowledge_context;
pub mod meeting_backend;
pub mod meeting_facilitator;
pub mod meeting_repl;
pub mod meetings;
pub mod memory;
pub mod memory_backup;
pub mod memory_bridge;
pub mod memory_bridge_adapter;
pub mod memory_cognitive;
pub mod memory_consolidation;
pub mod memory_hive;
pub mod memory_ipc;
pub mod memory_snapshot;
pub mod metadata;
pub mod ooda_actions;
pub mod ooda_brain;
pub mod ooda_loop;
pub mod ooda_scheduler;
pub mod operator_cli;
pub mod operator_commands;
mod operator_commands_dashboard;
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
pub mod review_pipeline;
pub mod runtime;
pub mod runtime_config;
pub mod runtime_ipc;
pub mod runtime_reflection;
mod sanitization;
pub mod self_improve;
pub mod self_improve_executor;
pub mod self_metrics;
pub mod self_relaunch;
pub mod self_relaunch_semaphore;
pub mod session;
pub mod session_builder;
pub mod skill_builder;
pub mod stewardship;
pub mod subagent_sessions;
pub mod terminal_engineer_bridge;
mod terminal_session;
#[doc(hidden)]
pub mod test_support;
#[cfg(test)]
mod tests_base_type_copilot;
#[cfg(test)]
mod tests_memory_ipc;
pub mod trace_collector;

pub use agent_goal_assignment::{
    SubordinateProgress, assign_goal, poll_progress, read_assigned_goal, report_progress,
};
pub use agent_program::{
    AgentProgram, AgentProgramContext, AgentProgramMemoryRecord, ImprovementCuratorProgram,
    MeetingFacilitatorProgram, ObjectiveRelayProgram,
};
pub use agent_registry::{
    AgentEntry, AgentRegistry, AgentState, FileBackedAgentRegistry, ResourceUsage, hostname,
    self_entry, self_resource_usage,
};
pub use agent_roles::{AgentRole, identity_for_role, role_for_objective};
pub use agent_supervisor::{
    HeartbeatStatus, SubordinateConfig, SubordinateHandle, check_heartbeat, kill_subordinate,
    max_retries_per_goal, spawn_subordinate,
};
pub use base_type_claude_agent_sdk::claude_agent_sdk_adapter;
pub use base_type_copilot::{CopilotAdapterConfig, CopilotSdkAdapter, parse_copilot_response};
pub use base_type_harness::{HarnessConfig, RealLocalHarnessAdapter};
pub use base_type_ms_agent::ms_agent_framework_adapter;
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
pub use build_lock::{BuildLock, BuildLockGuard};
pub use cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
pub use cost_tracking::{
    CostEntry, CostSummary, daily_summary, estimate_tokens, record_cost, weekly_summary,
};
pub use engineer_loop::{
    AnalyzedAction, EngineerLoopRun, ExecutedEngineerAction, PhaseOutcome, PhaseTrace,
    RepoInspection, SelectedEngineerAction, VerificationReport, analyze_objective,
    run_local_engineer_loop,
};
pub use engineer_plan::{
    Plan, PlanExecutionResult, PlanStep, PlanStepResult, execute_plan, plan_objective,
};
pub use error::{SimardError, SimardResult};
pub use evidence::{
    EvidenceRecord, EvidenceSource, EvidenceStore, FileBackedEvidenceStore, InMemoryEvidenceStore,
};
pub use goal_curation::{
    ActiveGoal, BacklogItem, DEFAULT_SEED_GOALS, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS,
    add_active_goal, add_backlog_item, archive_completed, load_goal_board, persist_board,
    promote_to_active, seed_default_board, update_goal_progress,
};
pub use goals::{
    FileBackedGoalStore, GoalRecord, GoalStatus, GoalStore, GoalUpdate, InMemoryGoalStore,
    seed_default_goals,
};
pub use gym::{
    BenchmarkArtifactPaths, BenchmarkCheckResult, BenchmarkComparisonArtifactPaths,
    BenchmarkComparisonDelta, BenchmarkComparisonReport, BenchmarkComparisonRunSummary,
    BenchmarkComparisonStatus, BenchmarkRunReport, BenchmarkScenario, BenchmarkSuiteReport,
    BenchmarkSuiteScenarioSummary, benchmark_scenarios, compare_latest_benchmark_runs,
    default_output_root, run_benchmark_scenario, run_benchmark_suite,
};
pub use gym_bridge::{GymBridge, GymScenario, GymScenarioResult, GymSuiteResult, ScoreDimensions};
pub use gym_history::{
    GymSignal, ScenarioSignal, ScoreHistory, ScoreRecord, check_promotion, generate_signals,
};
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
pub use identity_precedence::{ConflictEntry, ConflictLog, PrecedenceResolver, ResolvedIdentity};
pub use improvements::{
    ImprovementPromotionPlan, PersistedImprovementApproval, PersistedImprovementRecord,
    render_review_context_directives,
};
pub use knowledge_bridge::{
    KnowledgeBridge, KnowledgePackInfo, KnowledgeQueryResult, KnowledgeSource,
};
pub use knowledge_context::{PlanningContext, enrich_planning_context};
pub use meeting_backend::{
    ConversationMessage, MeetingBackend, MeetingResponse, MeetingSummary, MeetingTranscript, Role,
    SessionStatus,
};
pub use meeting_facilitator::{
    ActionItem, MEETING_HANDOFF_FILENAME, MeetingDecision, MeetingHandoff, MeetingSession,
    MeetingSessionStatus, add_note, close_meeting, default_handoff_dir, load_meeting_handoff,
    mark_handoff_processed_in_place, mark_meeting_handoff_processed, record_action_item,
    record_decision, start_meeting, write_meeting_handoff,
};
pub use meeting_repl::{MeetingCommand, parse_meeting_command, run_meeting_repl};
pub use meetings::{
    PersistedMeetingGoalUpdate, PersistedMeetingRecord, looks_like_persisted_meeting_record,
};
pub use memory::{
    FileBackedMemoryStore, InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore,
};
pub use memory_bridge::CognitiveMemoryBridge;
pub use memory_bridge_adapter::CognitiveBridgeMemoryStore;
pub use memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective,
    CognitiveSensoryItem, CognitiveStatistics, CognitiveWorkingSlot,
};
pub use memory_consolidation::{
    FactExtraction, PreparedContext, consolidation_intake, consolidation_persistence,
    execution_memory_operations, intake_memory_operations, persistence_memory_operations,
    preparation_memory_operations, reflection_memory_operations,
};
pub use ooda_actions::dispatch_actions;
pub use ooda_loop::{
    ActionKind, ActionOutcome, CycleReport, EnvironmentSnapshot, GoalSnapshot, Observation,
    OodaBridges, OodaConfig, OodaPhase, OodaState, PlannedAction, Priority, act,
    check_meeting_handoffs, decide, gather_environment, observe, orient, run_ooda_cycle,
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
    DeveloperWatch, ExtractionResult, IdeaProposal, ResearchStatus, ResearchTopic, ResearchTracker,
    add_research_topic, extract_ideas, load_research_topics, summarize_extraction, track_developer,
    update_topic_status,
};
pub use review::{
    ImprovementProposal, ReviewArtifact, ReviewRequest, ReviewSignal, ReviewTargetKind,
    build_review_artifact, latest_review_artifact, load_review_artifact, persist_review_artifact,
    render_review_text, review_artifacts_dir,
};
pub use review_pipeline::{
    FindingCategory, ReviewFinding, ReviewSession, Severity, review_diff, should_commit,
    summarize_review,
};
pub use runtime::{
    BaseTypeRegistry, CoordinatedSupervisor, InMemoryMailboxTransport, InProcessSupervisor,
    InProcessTopologyDriver, LocalRuntime, LoopbackMailboxTransport, LoopbackMeshTopologyDriver,
    RuntimeAddress, RuntimeKernel, RuntimeMailboxTransport, RuntimeNodeId, RuntimePorts,
    RuntimeRequest, RuntimeState, RuntimeSupervisor, RuntimeTopology, RuntimeTopologyDriver,
    SessionOutcome,
};
pub use runtime_ipc::{
    IpcMessage, IpcSubprocessHandle, IpcTransport, StdioTransport, shutdown_subprocess,
};
#[cfg(unix)]
pub use runtime_ipc::{UnixSocketTransport, spawn_subprocess};
pub use runtime_reflection::{
    LocalReflector, ResourceSnapshot, RuntimeReflection, RuntimeSnapshot, snapshot,
};
pub use self_improve::{
    ImprovementConfig, ImprovementCycle, ImprovementDecision, ImprovementPhase, ProposedChange,
    apply_improvements, run_improvement_cycle, summarize_cycle,
};
pub use self_improve_executor::{
    ApplyResult, ImprovementPatch, apply_and_review, generate_patch, run_autonomous_improvement,
};
pub use self_metrics::{
    DailyReport, MetricEntry, collect_and_record_all, daily_report, query_metrics, recent_metrics,
    record_metric,
};
pub use self_relaunch::{
    GateResult, RelaunchConfig, RelaunchGate, all_gates_passed, build_canary, coordinated_relaunch,
    default_gates, handover, verify_canary,
};
pub use self_relaunch_semaphore::{
    HandoffConfig, HandoffResult, LeaderSemaphore, LeaderState, coordinated_handoff, signal_ready,
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
