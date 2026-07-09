pub mod ado_acl_guard;
pub mod agent_goal_assignment;
pub mod agent_program;
pub mod agent_registry;
pub mod agent_roles;
pub mod agent_supervisor;
pub mod amplihack_freshness_gate;
pub mod base_type_claude_agent_sdk;
pub mod base_type_copilot;
pub mod base_type_harness;
pub mod base_type_ms_agent;
pub mod base_type_pending_sdk;
pub mod base_type_rustyclawd;
pub mod base_type_turn;
pub mod base_types;
pub mod bootstrap;
pub mod build_lock;
pub mod cargo_jobs;
pub mod ci_health;
pub mod cmd_cleanup;
pub mod cmd_ensure_deps;
pub mod cmd_install;
pub mod cmd_self_update;
pub mod cognitive_memory;
pub mod rpc;
pub mod rpc_circuit_breaker;
pub mod rpc_subprocess_launcher;
pub mod rpc_transport;
// Issue #2419: cognitive-thread scheduling — a `Mind` runs many
// `CognitiveThread`s (the primary OODA loop + maintenance + engineer-log
// analysis) on their own cadence/trigger. Sibling of `ooda_scheduler` (the
// engineer action-slot scheduler), which is unrelated and untouched. See
// `docs/reference/cognitive-thread-scheduling.md`.
pub mod cognitive_threads;
// Issue #2419: periodic brain self-examination + memory-hygiene pass — a
// higher-level introspection layer that reuses the existing distillation /
// statistics / expired-sensory infra (mirrors `disk_health`). Tests live in a
// `#[cfg(test)]` sibling so release/debug builds never compile them.
pub mod brain_introspection;
#[cfg(test)]
mod brain_introspection_tests;
mod copilot_status_probe;
mod copilot_task_submit;
// Issue #2527: one clearly-named operator↔Simard conversation abstraction. The
// CLI/TUI meeting REPL and the dashboard chat are channels over the same
// `MeetingBackend`; `SignalConversation` (feature-gated below) is a third.
pub mod conversation_channel;
pub mod cost_tracking;
// Issue #2419 (design spike) / #2647 (wiring): the Creative Ideas subsystem — an
// idea-generation cognitive thread + four-reviewer pipeline that primes a pool
// of candidate self-improvement ideas. Wired into the OODA daemon and
// **default-ON, opt-out** via `SIMARD_CREATIVE_IDEAS_ENABLED` (consistent with
// the Overseer/Journal threads, independent of the generic
// `SIMARD_COGNITIVE_THREADS_ENABLED` switch). See
// `docs/design/creative-ideas-thread.md`.
pub mod creative_ideas;
pub mod disk_health;
pub mod disk_pressure;
pub mod disk_reclaim;
pub mod engineer_loop;
pub mod engineer_worktree;
// Issue #2942: the enrichment-observability emit seam — proves recalled memory
// reaches OODA decisions (per-turn attach/degrade INFO/WARN + simard.enrichment.*
// metrics, the per-cycle rollup the dashboard reads, and the recall-on-vs-off
// ablation feeding #2644).
pub mod enrichment_observability;
pub mod error;
// Issue #2679: the shared per-fact reliability scorer, homed here so both
// write-boundary seams (the IPC `StoreFactGated` handler and the in-process
// distill sink) apply the identical store/quarantine decision.
pub mod eval_watchdog;
pub mod evidence;
pub mod fact_reliability;
#[cfg(test)]
mod fact_reliability_tests;
pub mod git_guardrails;
pub mod goal_board_store;
pub mod goal_curation;
pub mod goals;
pub mod greeting_banner;
// Phase 4 of issue #2713: the LOCAL "COIN Gym" harness — runs the COIN
// benchmark shape locally, scores vs. the published leaderboard, and A/Bs a
// single-model baseline against a multi-agent team, mirroring skwaq's
// failure-analysis + overfitting-reviewer gating. The harness executor delegates
// to `coin evaluate` (Docker + instrumented replay is Phase 3/VM); a mock oracle
// exercises the whole pipeline offline. See
// `docs/research/coin-benchmark-and-skwaq-study.md` (design) and
// `docs/howto/run-the-coin-gym-harness.md` (usage).
pub mod coin_gym;
pub mod gym;
pub mod gym_client;
pub mod gym_history;
pub mod gym_runner_client;
pub mod gym_scoring;
pub mod handoff;
pub mod hive_event_bus;
pub mod identity;
pub mod identity_auth;
pub mod identity_composition;
pub mod identity_precedence;
pub mod improvements;
pub mod journal;
pub mod knowledge_client;
pub mod knowledge_context;
pub mod meeting_backend;
pub mod meeting_facilitator;
pub mod meeting_repl;
pub mod meetings;
pub mod memory;
pub mod memory_backup;
pub mod memory_client;
pub mod memory_cognitive;
pub mod memory_consolidation;
pub mod memory_health;
pub mod memory_hive;
pub mod memory_ipc;
pub mod memory_snapshot;
pub mod memory_store_adapter;
pub mod metadata;
pub mod native_knowledge;
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
// Design spike (#2419): additive type/trait sketch for the Overseer
// operator/observer co-process. NOT wired into `main` or the daemon loop —
// nothing here is constructed at runtime. See docs/design/overseer.md.
pub mod overseer;
mod persistence;
pub mod prompt_assets;
pub mod prompt_delivery;
pub mod read_only_guard;
// Issues #2640/#2692: shared "path-in-argv, content-in-file" transport for
// unbounded recipe context values, so a large payload never overflows ARG_MAX
// (the live journal E2BIG recipe-spawn failure).
pub mod recipe_context_file;
pub mod recipe_output;
pub mod reflection;
pub mod remote_azlin;
pub mod remote_session;
pub mod remote_transfer;
pub mod research_tracker;
pub mod review;
pub mod review_pipeline;
pub mod rss_health;
pub mod runtime;
pub mod runtime_config;
pub mod runtime_ipc;
pub mod runtime_reflection;
pub mod rust_expertise;
pub mod safe_update;
mod sanitization;
pub mod self_deploy;
pub mod self_improve;
pub mod self_improve_executor;
pub mod self_metrics;
// Goal (#2419): recurring monthly self-quality-audit periodic task — a pure
// recipe invoker (mirrors `disk_health`) with disk-backed last-run persistence
// so the ~30-day cadence survives daemon restarts.
pub mod self_quality_audit;
#[cfg(test)]
mod self_quality_audit_tests;
pub mod self_relaunch;
pub mod self_relaunch_semaphore;
pub mod session;
pub mod session_builder;
pub mod session_id;
// Issue #2527: the Signal implementation of `conversation_channel`. Feature-gated
// (default off) so the daemon builds and runs fine without signal-cli installed.
#[cfg(feature = "signal")]
pub mod signal_conversation;
pub mod skill_builder;
// Issue #2640: the single large-payload spawn facade. One policy-enforcing
// chokepoint every agent/recipe launch routes a (possibly large) prompt or
// context value through, so a payload >= ARGV_PAYLOAD_MAX_BYTES is always
// delivered out-of-band (copilot prompts on stdin, recipe context on a file)
// and never touches argv/envp — closing the recurring E2BIG class for good.
pub mod spawn_payload;
pub mod state_root;
pub mod stewardship;
pub mod subagent_sessions;
// Issue #2741: proactive RUSTSEC/cargo-deny advisory stewardship. The pure
// remediation-decision reasoner (bump-or-justified-ignore behind a
// deterministic rail) lives here; it reuses `stewardship::dedup` and
// `stewardship::merge_authority`. See
// docs/reference/supply-chain-advisory-stewardship.md.
pub mod supply_chain_steward;
// Issue #2528: unified telemetry facade + one `simard status` snapshot. The
// `telemetry` module is the OpenTelemetry-backed metric facade + in-process
// registry; `status` is the single typed StatusSnapshot the CLI, dashboard, and
// TUI all render.
pub mod engineer_handoff;
pub mod status;
pub mod telemetry;
mod terminal_session;
#[doc(hidden)]
pub mod test_support;
#[cfg(test)]
mod tests_base_type_copilot;
#[cfg(test)]
mod tests_hermetic_guard;
#[cfg(test)]
mod tests_memory_ipc;
pub mod trace_collector;
pub mod update_check;
pub mod util;
pub mod worktree_gc;

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
    EnrichmentClients, ProposedAction, TurnContext, TurnOutput, enrich_turn_input,
    format_turn_input, parse_turn_output, prepare_turn_context,
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
pub use build_lock::{BuildLock, BuildLockGuard};
pub use cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
pub use cost_tracking::{
    CostEntry, CostSummary, daily_summary, estimate_tokens, record_cost, weekly_summary,
};
pub use engineer_loop::{
    AnalyzedAction, EngineerLoopRun, ExecutedEngineerAction, PhaseOutcome, PhaseTrace,
    RepoInspection, SelectedEngineerAction, SessionErrorReflection, VerificationReport,
    analyze_objective, run_local_engineer_loop, spawn_agent_for_goal,
};
pub use error::{SimardError, SimardResult};
pub use evidence::{
    EvidenceRecord, EvidenceSource, EvidenceStore, FileBackedEvidenceStore, InMemoryEvidenceStore,
};
pub use goal_curation::{
    ActiveGoal, BacklogItem, CARRYOVER_CONCEPT, CarryoverVerification, DEFAULT_SEED_GOALS,
    GoalBoard, GoalCarryoverRecord, GoalProgress, MAX_ACTIVE_GOALS, WipRef, add_active_goal,
    add_backlog_item, archive_completed, board_snapshot_hash, load_goal_board, persist_board,
    promote_to_active, read_latest_carryover, seed_default_board, update_goal_progress,
    update_goal_progress_with_evidence, verify_goal_carryover, write_goal_carryover,
};
pub use goals::{
    FileBackedGoalStore, GoalRecord, GoalStatus, GoalStore, GoalUpdate, InMemoryGoalStore,
    migrate_file_backed_goal_store_if_present, seed_default_goals,
};
pub use gym::{
    BenchmarkArtifactPaths, BenchmarkCheckResult, BenchmarkComparisonArtifactPaths,
    BenchmarkComparisonDelta, BenchmarkComparisonReport, BenchmarkComparisonRunSummary,
    BenchmarkComparisonStatus, BenchmarkRunReport, BenchmarkScenario, BenchmarkSuiteReport,
    BenchmarkSuiteScenarioSummary, benchmark_scenarios, compare_latest_benchmark_runs,
    default_output_root, run_benchmark_scenario, run_benchmark_suite,
};
pub use gym_client::{GymClient, GymScenario, GymScenarioResult, GymSuiteResult, ScoreDimensions};
pub use gym_history::{
    GymSignal, ScenarioSignal, ScoreHistory, ScoreRecord, check_promotion, generate_signals,
    record_benchmark_run, score_from_benchmark_report,
};
pub use gym_scoring::{
    DimensionTrend, GymSuiteScore, ImprovementTrend, Regression, RegressionSeverity,
    TrendDirection, aggregate_suite_scores, detect_regression, suite_score_from_benchmark_report,
    suite_score_from_benchmark_reports, suite_score_from_result, track_improvement,
};
pub use handoff::{
    CopilotSubmitAudit, FileBackedHandoffStore, InMemoryHandoffStore, RuntimeHandoffSnapshot,
    RuntimeHandoffStore,
};
pub use identity::{
    BuiltinIdentityLoader, IdentityAuthority, IdentityLoadRequest, IdentityLoader,
    IdentityManifest, ManifestContract, MemoryPolicy, OperatingMode, SeedGoal, WritePosture,
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
    EvidenceRef, ImprovementPromotionPlan, PersistedImprovementApproval,
    PersistedImprovementRecord, render_review_context_directives,
};
pub use knowledge_client::{
    KnowledgeClient, KnowledgePackInfo, KnowledgeQueryResult, KnowledgeSource,
};
pub use knowledge_context::{PlanningContext, enrich_planning_context};
pub use meeting_backend::{
    ConversationMessage, MeetingBackend, MeetingResponse, MeetingSummary, MeetingTranscript, Role,
    SessionStatus,
};
pub use meeting_facilitator::{
    ARTIFACT_KIND_BUNDLE, ARTIFACT_KIND_MARKDOWN_REPORT, ARTIFACT_KIND_OTHER,
    ARTIFACT_KIND_TEMPLATE_AGENDA, ARTIFACT_KIND_TRANSCRIPT, ActionItem, HandoffArtifact,
    MEETING_HANDOFF_FILENAME, MeetingDecision, MeetingHandoff, MeetingSession,
    MeetingSessionStatus, add_note, close_meeting, default_handoff_dir, load_meeting_handoff,
    mark_handoff_processed_in_place, mark_meeting_handoff_processed, record_action_item,
    record_decision, start_meeting, write_meeting_handoff,
};
pub use meeting_repl::{MeetingCommand, parse_meeting_command, run_meeting_repl};
pub use meetings::{
    PersistedMeetingGoalUpdate, PersistedMeetingRecord, build_persisted_meeting_record_value,
    looks_like_persisted_meeting_record,
};
pub use memory::{
    FileBackedMemoryStore, InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore,
};
pub use memory_client::CognitiveMemoryClient;
pub use memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective,
    CognitiveSensoryItem, CognitiveStatistics, CognitiveWorkingSlot,
};
pub use memory_consolidation::{
    FactExtraction, PreparedContext, consolidation_intake, consolidation_persistence,
    execution_memory_operations, intake_memory_operations, persistence_memory_operations,
    preparation_memory_operations, preparation_memory_operations_with_active_slugs,
    recall_procedures_for_objective, reflection_memory_operations,
};
pub use memory_store_adapter::CognitiveClientMemoryStore;
pub use ooda_actions::dispatch_actions;
pub use ooda_loop::{
    ActionKind, ActionOutcome, CycleReport, EnvironmentSnapshot, GoalSnapshot, Observation,
    OodaClients, OodaConfig, OodaPhase, OodaState, PlannedAction, Priority, act,
    check_meeting_handoffs, decide, gather_environment, observe, orient, run_ooda_cycle,
    summarize_cycle_report,
};
pub use ooda_scheduler::{
    CompletedSlot, ScheduledAction, Scheduler, SchedulerSlot, SlotStatus, complete_slot,
    drain_finished, fail_slot, poll_slots, schedule_actions, scheduler_summary, start_slot,
};
pub use rpc::{
    RpcErrorPayload, RpcHealth, RpcId, RpcRequest, RpcResponse, RpcTransport, new_request_id,
    unpack_rpc_response,
};
pub use rpc_circuit_breaker::{CircuitBreakerConfig, CircuitBreakerTransport, CircuitState};
pub use rpc_transport::{InMemoryRpcTransport, SubprocessRpcTransport};
pub use test_support::TestAdapter;

pub use coin_gym::{coin_gym_usage, dispatch_coin_gym_cli};
pub use engineer_handoff::{
    ENGINEER_HANDOFF_FILE_NAME, ENGINEER_MODE_BOUNDARY, EngineerHandoffContext,
    SHARED_DEFAULT_STATE_ROOT_SOURCE, SHARED_EXPLICIT_STATE_ROOT_SOURCE,
    TERMINAL_HANDOFF_FILE_NAME, TERMINAL_MODE_BOUNDARY,
};
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
pub use remote_transfer::{ENVELOPE_SCHEMA_VERSION, MemorySnapshot, PersistedEnvelope};
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
    ImprovementConfig, ImprovementCycle, ImprovementDecision, ImprovementHypothesis,
    ImprovementPhase, ProposedChange, aggregate_hypotheses, apply_improvements,
    form_hypotheses_from_benchmark_reports, form_hypotheses_from_review,
    form_hypotheses_from_session_failures, form_hypotheses_from_signals,
    form_hypotheses_from_weak_dimensions, run_improvement_cycle, summarize_cycle,
};
pub use self_improve_executor::{
    ApplyResult, ApprovalPolicy, ImprovementPatch, apply_and_review, generate_patch,
    run_autonomous_improvement,
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
