//! Integration contract for deterministic recipe composition around a typed
//! goal-session actor.
//!
//! TDD status: RED until the typed route, scoped terminal tools, route graph,
//! and durable terminal verification are implemented.

use std::collections::BTreeSet;

use simard::typed_ooda::{
    Action, AdmissionSnapshot, AuthenticatedToolContext, CapabilityGrant, CapabilityHandler,
    CapabilityPolicy, CycleErrorCode, EffectExecutionError, EffectExecutor, EffectJob,
    EffectResult, GoalSessionExecutor, GoalSessionInvocation, OpaqueBytes, RecipeProcessError,
    RepositoryRef, RouteNodeKind, SpawnEngineerAction, TerminalKind, TypedGoalSessionRoute,
};

struct SucceedingEffects;

impl EffectExecutor for SucceedingEffects {
    fn execute(&self, _job: &EffectJob) -> Result<EffectResult, EffectExecutionError> {
        Ok(EffectResult::Succeeded {
            evidence: Vec::new(),
        })
    }
}

struct PermanentlyFailingEffects;

impl EffectExecutor for PermanentlyFailingEffects {
    fn execute(&self, _job: &EffectJob) -> Result<EffectResult, EffectExecutionError> {
        Err(EffectExecutionError::permanent("typed spawn failure"))
    }
}

fn invocation(cycle_id: &str) -> GoalSessionInvocation {
    GoalSessionInvocation {
        session_id: "session-workflow".to_string(),
        cycle_id: cycle_id.to_string(),
        goal_id: "goal-4052".to_string(),
        task: OpaqueBytes::from(b"\nTASK:\0\xffraw task bytes\n".to_vec()),
        reason: OpaqueBytes::from(b"{\"reason\":\"raw\"}\n".to_vec()),
        observe_output: OpaqueBytes::from(b"observe\nNO ACTION\n".to_vec()),
        orient_output: OpaqueBytes::from(vec![0xff, 0xfe, b'O']),
        decide_output: OpaqueBytes::from(b"ACTION: SPAWN_ENGINEER is just prose".to_vec()),
    }
}

fn executor() -> (tempfile::TempDir, GoalSessionExecutor) {
    let dir = tempfile::tempdir().expect("tempdir");
    let handler = CapabilityHandler::open(
        dir.path().join("outcomes.sqlite3"),
        CapabilityPolicy::goal_session_default("goal-session-policy-v1"),
    )
    .expect("handler");
    let actor = AuthenticatedToolContext::new(
        "goal-session-actor",
        "session-workflow",
        [
            CapabilityGrant::RecordAction(simard::typed_ooda::ActionKind::SpawnEngineer),
            CapabilityGrant::RecordNoAction,
            CapabilityGrant::RecordBlocked,
            CapabilityGrant::RecordCompleted,
        ],
    );
    let admission = AdmissionSnapshot {
        concurrent_engineers: 0,
        disk_used_percent: 5,
        active_claims: BTreeSet::new(),
        policy_revision: "admission-v1".to_string(),
    };
    (
        dir,
        GoalSessionExecutor::new(handler, actor, admission, Box::new(SucceedingEffects)),
    )
}

#[test]
fn raw_outputs_flow_to_the_actor_byte_for_byte_and_action_finishes_from_durable_truth() {
    let (_dir, executor) = executor();
    let invocation = invocation("cycle-workflow-action");
    let expected = invocation.clone();

    let execution = executor
        .execute(&invocation, |received, tools| {
            assert_eq!(received.task.as_bytes(), expected.task.as_bytes());
            assert_eq!(received.reason.as_bytes(), expected.reason.as_bytes());
            assert_eq!(
                received.observe_output.as_bytes(),
                expected.observe_output.as_bytes()
            );
            assert_eq!(
                received.orient_output.as_bytes(),
                expected.orient_output.as_bytes()
            );
            assert_eq!(
                received.decide_output.as_bytes(),
                expected.decide_output.as_bytes()
            );

            tools.record_action(
                "request-workflow-action",
                Action::SpawnEngineer(SpawnEngineerAction {
                    task: received.task.clone(),
                    repository: RepositoryRef::new("rysweet", "Simard"),
                    base_type: simard::typed_ooda::BaseType::Copilot,
                    requested_permissions: BTreeSet::from(["repo_read".to_string()]),
                    claim_key: "rysweet/Simard:goal-4052".to_string(),
                }),
                received.decide_output.clone(),
                Vec::new(),
            )?;
            Ok(())
        })
        .expect("recipe plus typed terminal");

    assert_eq!(execution.outcome.kind, TerminalKind::Action);
    assert_eq!(
        executor
            .handler()
            .effect_for_outcome(&execution.outcome.outcome_id)
            .expect("effect query")
            .expect("action effect")
            .state
            .as_str(),
        "succeeded",
        "an intended machine-action cycle is complete only after its typed effect succeeds"
    );
    assert_eq!(
        executor
            .handler()
            .terminal_count("session-workflow", "cycle-workflow-action")
            .expect("terminal count"),
        1
    );
}

#[test]
fn explicit_no_action_cycle_succeeds_only_from_its_durable_record() {
    let (_dir, executor) = executor();
    let invocation = invocation("cycle-workflow-no-action");

    let execution = executor
        .execute(&invocation, |received, tools| {
            tools.record_no_action(
                "request-workflow-no-action",
                received.reason.clone(),
                received.decide_output.clone(),
                Vec::new(),
            )?;
            Ok(())
        })
        .expect("durable no-action terminal");

    assert_eq!(execution.outcome.kind, TerminalKind::NoAction);
    assert_eq!(
        execution
            .outcome
            .payload
            .no_action()
            .expect("no-action payload")
            .reason
            .as_bytes(),
        invocation.reason.as_bytes()
    );
}

#[test]
fn recipe_success_without_a_terminal_is_a_missing_terminal_failure() {
    let (_dir, executor) = executor();
    let invocation = invocation("cycle-missing-terminal");

    let error = executor
        .execute(&invocation, |_received, _tools| Ok(()))
        .expect_err("process success is insufficient");

    assert_eq!(error.code(), CycleErrorCode::MissingTerminal);
    assert_eq!(
        executor
            .handler()
            .terminal_count("session-workflow", "cycle-missing-terminal")
            .expect("terminal count"),
        0
    );
}

#[test]
fn a_second_terminal_attempt_fails_the_cycle_and_does_not_replace_the_first() {
    let (_dir, executor) = executor();
    let invocation = invocation("cycle-multiple-terminals");

    let error = executor
        .execute(&invocation, |received, tools| {
            tools.record_no_action(
                "request-multiple-1",
                OpaqueBytes::from(b"first".to_vec()),
                received.decide_output.clone(),
                Vec::new(),
            )?;
            let _ = tools.record_blocked(
                "request-multiple-2",
                OpaqueBytes::from(b"second".to_vec()),
                simard::typed_ooda::BlockerRef::Goal {
                    goal_id: "other-goal".to_string(),
                },
                simard::typed_ooda::RetryPolicy::AfterGoal {
                    goal_id: "other-goal".to_string(),
                },
                received.decide_output.clone(),
                Vec::new(),
            );
            Ok(())
        })
        .expect_err("exactly one terminal invocation is required");

    assert_eq!(error.code(), CycleErrorCode::MultipleTerminalAttempts);
    assert_eq!(
        executor
            .handler()
            .terminal_count("session-workflow", "cycle-multiple-terminals")
            .expect("terminal count"),
        1
    );
}

#[test]
fn tool_failure_propagates_even_if_the_actor_tries_to_return_success() {
    let (_dir, executor) = executor();
    let invocation = invocation("cycle-tool-failure");

    let error = executor
        .execute(&invocation, |received, tools| {
            let _ = tools.record_action(
                "request-invalid-action",
                Action::SpawnEngineer(SpawnEngineerAction {
                    task: OpaqueBytes::from(Vec::new()),
                    repository: RepositoryRef::new("rysweet", "Simard"),
                    base_type: simard::typed_ooda::BaseType::Copilot,
                    requested_permissions: BTreeSet::new(),
                    claim_key: "rysweet/Simard:goal-4052".to_string(),
                }),
                received.decide_output.clone(),
                Vec::new(),
            );
            Ok(())
        })
        .expect_err("ignored tool error still fails the cycle");

    assert_eq!(error.code(), CycleErrorCode::ToolFailed);
    assert_eq!(
        executor
            .handler()
            .terminal_count("session-workflow", "cycle-tool-failure")
            .expect("terminal count"),
        0
    );
}

#[test]
fn nonzero_recipe_exit_propagates_without_synthetic_no_action_or_progress() {
    let (_dir, executor) = executor();
    let invocation = invocation("cycle-recipe-failure");

    let error = executor
        .execute(&invocation, |_received, _tools| {
            Err(RecipeProcessError::nonzero_exit(17))
        })
        .expect_err("recipe failure must terminate the cycle");

    assert_eq!(error.code(), CycleErrorCode::RecipeFailed);
    assert_eq!(
        executor
            .handler()
            .terminal_count("session-workflow", "cycle-recipe-failure")
            .expect("terminal count"),
        0
    );
    assert!(
        executor
            .handler()
            .progress_for_cycle("session-workflow", "cycle-recipe-failure")
            .expect("progress query")
            .is_empty(),
        "recipe failure cannot become progress"
    );
}

#[test]
fn permanent_downstream_failure_fails_the_cycle_but_keeps_the_action_terminal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let handler = CapabilityHandler::open(
        dir.path().join("outcomes.sqlite3"),
        CapabilityPolicy::goal_session_default("goal-session-policy-v1"),
    )
    .expect("handler");
    let actor = AuthenticatedToolContext::new(
        "goal-session-actor",
        "session-workflow",
        [CapabilityGrant::RecordAction(
            simard::typed_ooda::ActionKind::SpawnEngineer,
        )],
    );
    let executor = GoalSessionExecutor::new(
        handler,
        actor,
        AdmissionSnapshot {
            concurrent_engineers: 0,
            disk_used_percent: 5,
            active_claims: BTreeSet::new(),
            policy_revision: "admission-v1".to_string(),
        },
        Box::new(PermanentlyFailingEffects),
    );
    let invocation = invocation("cycle-downstream-failure");

    let error = executor
        .execute(&invocation, |received, tools| {
            tools.record_action(
                "request-downstream-failure",
                Action::SpawnEngineer(SpawnEngineerAction {
                    task: received.task.clone(),
                    repository: RepositoryRef::new("rysweet", "Simard"),
                    base_type: simard::typed_ooda::BaseType::Copilot,
                    requested_permissions: BTreeSet::from(["repo_read".to_string()]),
                    claim_key: "rysweet/Simard:goal-4052".to_string(),
                }),
                received.decide_output.clone(),
                Vec::new(),
            )?;
            Ok(())
        })
        .expect_err("permanent effect failure must fail cycle execution");

    assert_eq!(error.code(), CycleErrorCode::DownstreamFailed);
    let terminal = executor
        .handler()
        .terminal_for_cycle("session-workflow", "cycle-downstream-failure")
        .expect("terminal query")
        .expect("semantic action remains authoritative");
    assert_eq!(terminal.kind, TerminalKind::Action);
    assert_eq!(
        executor
            .handler()
            .effect_for_outcome(&terminal.outcome_id)
            .expect("effect query")
            .expect("failed effect")
            .state
            .as_str(),
        "failed"
    );
}

#[test]
fn migrated_route_graph_has_no_path_to_a_prose_parser_or_legacy_fallback() {
    let graph = TypedGoalSessionRoute::dependency_graph();
    let reachable = graph.reachable_from(graph.goal_session_entry());

    assert!(
        reachable
            .iter()
            .all(|node| node.kind != RouteNodeKind::ProseParser),
        "typed goal-session route reached a prose parser: {reachable:#?}"
    );
    assert!(
        reachable
            .iter()
            .all(|node| node.kind != RouteNodeKind::LegacyFallback),
        "typed failures must never fall back to the parser route: {reachable:#?}"
    );
    for required in [
        RouteNodeKind::RecipeRunner,
        RouteNodeKind::ScopedCapabilityTools,
        RouteNodeKind::TerminalHandler,
        RouteNodeKind::OutcomeLedger,
        RouteNodeKind::EffectOutbox,
    ] {
        assert!(
            reachable.iter().any(|node| node.kind == required),
            "typed route must include {required:?}: {reachable:#?}"
        );
    }
}
