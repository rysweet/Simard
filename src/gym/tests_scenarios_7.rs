//! Tests for the KnowledgeRecall tools sub-family (issue #1459, second PR).
//!
//! Covers the three new scenarios:
//!   - knowledge-recall-tool-amplihack-recipe
//!   - knowledge-recall-tool-pre-push-skip
//!   - knowledge-recall-tool-redeploy-script
//!
//! For each scenario this file verifies:
//!   (a) the scenario resolves via `resolve_benchmark_scenario` with the
//!       expected class/identity/base_type/expected_min_runtime_evidence; and
//!   (b) `class_specific_checks` dispatches to `checks_for_knowledge_recall`
//!       and produces both `knowledge-recall-evidence-grounded` and
//!       `knowledge-recall-topic-cited` results, passing for grounded answers
//!       and failing for ungrounded ones.

use super::scenarios::*;
use super::types::BenchmarkClass;
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

// -- (a) Resolve coverage: each new scenario is registered with the right shape. --

#[test]
fn knowledge_recall_tool_amplihack_recipe_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-amplihack-recipe")
        .expect("knowledge-recall-tool-amplihack-recipe scenario must be registered");
    assert_eq!(scenario.class, BenchmarkClass::KnowledgeRecall);
    assert_eq!(scenario.identity, "simard-gym");
    assert_eq!(scenario.base_type, "rusty-clawd");
    assert_eq!(scenario.topology, RuntimeTopology::SingleProcess);
    assert_eq!(scenario.expected_min_runtime_evidence, 2);
}

#[test]
fn knowledge_recall_tool_pre_push_skip_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-pre-push-skip")
        .expect("knowledge-recall-tool-pre-push-skip scenario must be registered");
    assert_eq!(scenario.class, BenchmarkClass::KnowledgeRecall);
    assert_eq!(scenario.identity, "simard-gym");
    assert_eq!(scenario.base_type, "rusty-clawd");
    assert_eq!(scenario.topology, RuntimeTopology::SingleProcess);
    assert_eq!(scenario.expected_min_runtime_evidence, 2);
}

#[test]
fn knowledge_recall_tool_redeploy_script_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-redeploy-script")
        .expect("knowledge-recall-tool-redeploy-script scenario must be registered");
    assert_eq!(scenario.class, BenchmarkClass::KnowledgeRecall);
    assert_eq!(scenario.identity, "simard-gym");
    assert_eq!(scenario.base_type, "rusty-clawd");
    assert_eq!(scenario.topology, RuntimeTopology::SingleProcess);
    assert_eq!(scenario.expected_min_runtime_evidence, 2);
}

// -- (b) Dispatch coverage: class_specific_checks routes to KnowledgeRecall and
//        the topic_match arm fires for each new scenario id. --

#[test]
fn class_checks_knowledge_recall_tool_amplihack_recipe_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-amplihack-recipe").unwrap();
    let outcome = dummy_outcome(
        "consult docs/ for amplihack recipe runner invocation patterns",
        "amplihack recipe run smart-orchestrator -c task_description=... requires AMPLIHACK_HOME env var",
        "cited recipe + AMPLIHACK_HOME env var",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "knowledge-recall-evidence-grounded" && c.passed),
        "evidence-grounded must pass when memory records exist or a path is cited"
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "knowledge-recall-topic-cited" && c.passed),
        "topic-cited must pass when amplihack + recipe + AMPLIHACK_HOME are named"
    );
}

#[test]
fn class_checks_knowledge_recall_tool_amplihack_recipe_fails_when_topic_missing() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-amplihack-recipe").unwrap();
    // grounded by a path but missing the canonical tokens
    let outcome = dummy_outcome(
        "look at src/main.rs",
        "I think you run some command somewhere",
        "no specifics recalled",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "knowledge-recall-topic-cited" && !c.passed),
        "topic-cited must fail when amplihack/recipe/AMPLIHACK_HOME tokens are absent"
    );
}

#[test]
fn class_checks_knowledge_recall_tool_pre_push_skip_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-pre-push-skip").unwrap();
    let outcome = dummy_outcome(
        "recall pre-push hook policy from stored user-preference memory",
        "use SKIP=cargo-test for known-flaky local cargo tests; --no-verify is forbidden by user policy",
        "documented preference",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for a fully grounded pre-push-skip answer"
    );
}

#[test]
fn class_checks_knowledge_recall_tool_pre_push_skip_fails_when_topic_missing() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-pre-push-skip").unwrap();
    let outcome = dummy_outcome(
        "look at scripts/ folder",
        "there is some way to skip tests",
        "vague",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "knowledge-recall-topic-cited" && !c.passed),
        "topic-cited must fail when SKIP=cargo-test and --no-verify tokens are absent"
    );
}

#[test]
fn class_checks_knowledge_recall_tool_redeploy_script_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-redeploy-script").unwrap();
    let outcome = dummy_outcome(
        "recall scripts/redeploy-local.sh from prior maintenance sessions",
        "scripts/redeploy-local.sh rebuilds and reinstalls the daemon; CARGO_TARGET_DIR points it at the build cache",
        "cited script + CARGO_TARGET_DIR env var",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "knowledge-recall-evidence-grounded" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "knowledge-recall-topic-cited" && c.passed),
        "topic-cited must pass when redeploy-local.sh + CARGO_TARGET_DIR are named"
    );
}

#[test]
fn class_checks_knowledge_recall_tool_redeploy_script_fails_when_topic_missing() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-redeploy-script").unwrap();
    let outcome = dummy_outcome(
        "look at scripts/",
        "there is a build-and-install flow somewhere",
        "vague",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "knowledge-recall-topic-cited" && !c.passed),
        "topic-cited must fail when redeploy-local.sh and CARGO_TARGET_DIR tokens are absent"
    );
}

// -- All-three-resolve smoke test: handy single assertion for the sub-family. --

#[test]
fn knowledge_recall_tools_subfamily_has_three_scenarios() {
    let ids = [
        "knowledge-recall-tool-amplihack-recipe",
        "knowledge-recall-tool-pre-push-skip",
        "knowledge-recall-tool-redeploy-script",
    ];
    for id in ids {
        let scenario =
            resolve_benchmark_scenario(id).unwrap_or_else(|_| panic!("scenario {id} must resolve"));
        assert_eq!(
            scenario.class,
            BenchmarkClass::KnowledgeRecall,
            "scenario {id} must be KnowledgeRecall"
        );
    }
}
