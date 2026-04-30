//! Tests for the ErrorHandlingDebug sub-family (issue #1461).
//!
//! These cover the four scenarios added by this PR for the
//! `BenchmarkClass::ErrorHandling` debug sub-family proposed by the
//! prompt-driven OODA brain in cycle 2 after the KnowledgeRecall family
//! landed (#1467). Each scenario asks the agent to diagnose a real Simard
//! runtime error and propose the documented remediation.

use super::scenarios::*;
use super::types::{BenchmarkClass, BenchmarkScenario};
use crate::base_types::BaseTypeId;
use crate::handoff::RuntimeHandoffSnapshot;
use crate::identity::ManifestContract;
use crate::identity::OperatingMode;
use crate::memory::{MemoryRecord, MemoryScope};
use crate::metadata::{BackendDescriptor, Freshness, Provenance};
use crate::reflection::{ReflectionReport, ReflectionSnapshot};
use crate::runtime::{
    RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology, SessionOutcome,
};
use crate::session::{SessionId, SessionPhase, SessionRecord};

fn dummy_backend() -> BackendDescriptor {
    BackendDescriptor {
        identity: "test-backend".to_string(),
        provenance: Provenance::new("test-src", "test::loc"),
        freshness: Freshness::now().unwrap(),
    }
}

fn dummy_contract() -> ManifestContract {
    ManifestContract {
        entrypoint: "test::entry".to_string(),
        composition: "a -> b".to_string(),
        precedence: vec!["tag:value".to_string()],
        provenance: Provenance::new("test-src", "test::loc"),
        freshness: Freshness::now().unwrap(),
    }
}

fn dummy_snapshot() -> ReflectionSnapshot {
    let backend = dummy_backend();
    ReflectionSnapshot {
        identity_name: "test".to_string(),
        identity_components: vec![],
        selected_base_type: BaseTypeId::new("test"),
        topology: RuntimeTopology::SingleProcess,
        runtime_state: RuntimeState::Ready,
        runtime_node: RuntimeNodeId::local(),
        mailbox_address: RuntimeAddress::local(&RuntimeNodeId::local()),
        session_phase: Some(SessionPhase::Complete),
        prompt_assets: vec![],
        manifest_contract: dummy_contract(),
        evidence_records: 0,
        memory_records: 0,
        active_goal_count: 0,
        active_goals: vec![],
        proposed_goal_count: 0,
        proposed_goals: vec![],
        agent_program_backend: backend.clone(),
        handoff_backend: backend.clone(),
        adapter_backend: backend.clone(),
        adapter_capabilities: vec![],
        adapter_supported_topologies: vec![],
        topology_backend: backend.clone(),
        transport_backend: backend.clone(),
        supervisor_backend: backend.clone(),
        memory_backend: backend.clone(),
        evidence_backend: backend.clone(),
        goal_backend: backend,
    }
}

fn dummy_outcome(plan: &str, execution_summary: &str, reflection_summary: &str) -> SessionOutcome {
    SessionOutcome {
        session: SessionRecord {
            id: SessionId::parse("session-00000000-0000-0000-0000-000000000001").unwrap(),
            mode: OperatingMode::Gym,
            objective: "test".to_string(),
            phase: SessionPhase::Complete,
            selected_base_type: BaseTypeId::new("test"),
            evidence_ids: vec![],
            memory_keys: vec![],
        },
        plan: plan.to_string(),
        execution_summary: execution_summary.to_string(),
        reflection: ReflectionReport {
            summary: reflection_summary.to_string(),
            snapshot: dummy_snapshot(),
        },
    }
}

fn dummy_handoff(memory_count: usize) -> RuntimeHandoffSnapshot {
    let session_id = SessionId::parse("session-00000000-0000-0000-0000-000000000001").unwrap();
    RuntimeHandoffSnapshot {
        exported_state: RuntimeState::Stopped,
        identity_name: "test".to_string(),
        selected_base_type: BaseTypeId::new("test"),
        topology: RuntimeTopology::SingleProcess,
        source_runtime_node: RuntimeNodeId::local(),
        source_mailbox_address: RuntimeAddress::local(&RuntimeNodeId::local()),
        session: None,
        memory_records: (0..memory_count)
            .map(|i| MemoryRecord {
                key: format!("key-{i}"),
                scope: MemoryScope::Benchmark,
                value: format!("value-{i}"),
                session_id: session_id.clone(),
                recorded_in: SessionPhase::Complete,
                created_at: None,
            })
            .collect(),
        evidence_records: vec![],
        copilot_submit_audit: None,
    }
}

fn assert_error_handling_debug_scenario_shape(scenario: &BenchmarkScenario) {
    assert_eq!(scenario.class, BenchmarkClass::ErrorHandling);
    assert_eq!(scenario.identity, "simard-gym");
    assert_eq!(scenario.base_type, "rusty-clawd");
    assert_eq!(scenario.topology, RuntimeTopology::SingleProcess);
    assert_eq!(scenario.expected_min_runtime_evidence, 2);
}

#[test]
fn error_handling_debug_stale_engineer_worktree_resolves() {
    let scenario = resolve_benchmark_scenario("error-handling-debug-stale-engineer-worktree")
        .expect("scenario must be registered");
    assert_error_handling_debug_scenario_shape(&scenario);
}

#[test]
fn error_handling_debug_pre_push_clippy_failure_resolves() {
    let scenario = resolve_benchmark_scenario("error-handling-debug-pre-push-clippy-failure")
        .expect("scenario must be registered");
    assert_error_handling_debug_scenario_shape(&scenario);
}

#[test]
fn error_handling_debug_mkdocs_strict_broken_link_resolves() {
    let scenario = resolve_benchmark_scenario("error-handling-debug-mkdocs-strict-broken-link")
        .expect("scenario must be registered");
    assert_error_handling_debug_scenario_shape(&scenario);
}

#[test]
fn error_handling_debug_recipe_runner_hollow_success_resolves() {
    let scenario = resolve_benchmark_scenario("error-handling-debug-recipe-runner-hollow-success")
        .expect("scenario must be registered");
    assert_error_handling_debug_scenario_shape(&scenario);
}

#[test]
fn class_checks_stale_engineer_worktree_passes_with_grounded_answer() {
    let scenario =
        resolve_benchmark_scenario("error-handling-debug-stale-engineer-worktree").unwrap();
    let outcome = dummy_outcome(
        "investigate ~/.simard/engineer-worktrees/ liveness",
        "the OODA dispatch layer's find_live_engineer_for_goal check inspects the engineer-worktrees sentinel; the subagent has exited so the worktree is alive but has no live process",
        "src/ooda_loop dispatch verified",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all ErrorHandlingDebug checks should pass for a fully grounded stale-engineer-worktree answer: {checks:?}"
    );
}

#[test]
fn class_checks_pre_push_clippy_failure_passes_with_grounded_answer() {
    let scenario =
        resolve_benchmark_scenario("error-handling-debug-pre-push-clippy-failure").unwrap();
    let outcome = dummy_outcome(
        "diagnose pre-push clippy failure under -D warnings",
        "clippy reports unused_imports in src/foo.rs; bypassing via --no-verify is forbidden by user policy and is never an acceptable workaround — fix the import",
        "documented in docs/",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all ErrorHandlingDebug checks should pass for a fully grounded clippy-failure answer: {checks:?}"
    );
}

#[test]
fn class_checks_mkdocs_strict_broken_link_passes_with_grounded_answer() {
    let scenario =
        resolve_benchmark_scenario("error-handling-debug-mkdocs-strict-broken-link").unwrap();
    let outcome = dummy_outcome(
        "diagnose docs/build CI failure",
        "mkdocs.yml enables strict mode; a link from docs/ pointing at prompt_assets/ outside the docs/ tree cannot resolve",
        "fix in docs/",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all ErrorHandlingDebug checks should pass for a fully grounded mkdocs-strict answer: {checks:?}"
    );
}

#[test]
fn class_checks_recipe_runner_hollow_success_passes_with_grounded_answer() {
    let scenario =
        resolve_benchmark_scenario("error-handling-debug-recipe-runner-hollow-success").unwrap();
    let outcome = dummy_outcome(
        "diagnose smart-orchestrator hollow success",
        "step-08c-implementation-no-op-guard reported produced no output; symptom is the worktree was never created — apply the documented Opus 4.7 sub-agent fallback pattern",
        "documented in docs/",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all ErrorHandlingDebug checks should pass for a fully grounded hollow-success answer: {checks:?}"
    );
}
