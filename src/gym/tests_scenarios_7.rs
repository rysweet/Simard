//! Tests for the KnowledgeRecall **tools-knowledge** sub-family (issue #1459).
//!
//! These cover the three scenarios added by the second scenario-level PR
//! after #1460: amplihack recipe runner recall, the SKIP=cargo-test pre-push
//! override recall, and the redeploy-local.sh / SIMARD_SHARED_TARGET recall.

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

fn assert_tools_scenario_shape(scenario: &BenchmarkScenario) {
    assert_eq!(scenario.class, BenchmarkClass::KnowledgeRecall);
    assert_eq!(scenario.identity, "simard-gym");
    assert_eq!(scenario.base_type, "rusty-clawd");
    assert_eq!(scenario.topology, RuntimeTopology::SingleProcess);
    assert_eq!(scenario.expected_min_runtime_evidence, 2);
}

#[test]
fn knowledge_recall_tool_amplihack_recipe_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-amplihack-recipe")
        .expect("knowledge-recall-tool-amplihack-recipe scenario must be registered");
    assert_tools_scenario_shape(&scenario);
}

#[test]
fn knowledge_recall_tool_pre_push_skip_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-pre-push-skip")
        .expect("knowledge-recall-tool-pre-push-skip scenario must be registered");
    assert_tools_scenario_shape(&scenario);
}

#[test]
fn knowledge_recall_tool_redeploy_script_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-redeploy-script")
        .expect("knowledge-recall-tool-redeploy-script scenario must be registered");
    assert_tools_scenario_shape(&scenario);
}

#[test]
fn class_checks_knowledge_recall_tool_amplihack_recipe_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-amplihack-recipe").unwrap();
    let outcome = dummy_outcome(
        "consult stored memory on the amplihack recipe runner invocation pattern",
        "amplihack recipe run smart-orchestrator -c repo_path=. — requires AMPLIHACK_HOME and preserves AMPLIHACK_AGENT_BINARY",
        "documented invocation in src/gym/scenarios/data_6.rs",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for a fully grounded amplihack-recipe answer: {checks:?}"
    );
}

#[test]
fn class_checks_knowledge_recall_tool_pre_push_skip_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-pre-push-skip").unwrap();
    let outcome = dummy_outcome(
        "recall the approved pre-push override for known-flaky tests",
        "use SKIP=cargo-test git push to skip the cargo-test stage of the local pre-push hook",
        "--no-verify is forbidden because it bypasses every hook stage; SKIP=cargo-test is the only sanctioned override (cited in docs/)",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for a fully grounded pre-push-skip answer: {checks:?}"
    );
}

#[test]
fn class_checks_knowledge_recall_tool_redeploy_script_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-redeploy-script").unwrap();
    let outcome = dummy_outcome(
        "recall the post-merge redeploy script for the simard daemon",
        "scripts/redeploy-local.sh rebuilds the simard binary using SIMARD_SHARED_TARGET as the target dir and reinstalls to ~/.simard/bin/simard",
        "documented in src/ workflow notes",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for a fully grounded redeploy-script answer: {checks:?}"
    );
}

#[test]
fn class_checks_knowledge_recall_tools_fail_when_ungrounded() {
    for id in [
        "knowledge-recall-tool-amplihack-recipe",
        "knowledge-recall-tool-pre-push-skip",
        "knowledge-recall-tool-redeploy-script",
    ] {
        let scenario = resolve_benchmark_scenario(id).unwrap();
        let outcome = dummy_outcome("nothing", "bland text", "empty");
        let exported = dummy_handoff(0);
        let checks = class_specific_checks(&scenario, &outcome, &exported);
        assert_eq!(checks.len(), 2);
        for check in &checks {
            assert!(
                !check.passed,
                "scenario {id}: check '{}' should have failed",
                check.id
            );
        }
    }
}

// ---- repo-knowledge sub-family (issue #1459) ----

#[test]
fn knowledge_recall_repo_ooda_loop_layout_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-repo-ooda-loop-layout")
        .expect("knowledge-recall-repo-ooda-loop-layout scenario must be registered");
    assert_tools_scenario_shape(&scenario);
}

#[test]
fn knowledge_recall_repo_cognitive_memory_store_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-repo-cognitive-memory-store")
        .expect("knowledge-recall-repo-cognitive-memory-store scenario must be registered");
    assert_tools_scenario_shape(&scenario);
}

#[test]
fn knowledge_recall_repo_engineer_worktree_pattern_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-repo-engineer-worktree-pattern")
        .expect("knowledge-recall-repo-engineer-worktree-pattern scenario must be registered");
    assert_tools_scenario_shape(&scenario);
}

#[test]
fn class_checks_knowledge_recall_repo_ooda_loop_layout_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-repo-ooda-loop-layout").unwrap();
    let outcome = dummy_outcome(
        "consult stored memory on Simard's OODA loop module layout",
        "the OODA loop lives under src/ooda_loop/ with phase modules observe, orient, decide, act and the cycle entry point in cycle.rs",
        "documented module map confirmed via mod.rs",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for a fully grounded ooda-loop-layout answer: {checks:?}"
    );
}

#[test]
fn class_checks_knowledge_recall_repo_cognitive_memory_store_passes_with_grounded_answer() {
    let scenario =
        resolve_benchmark_scenario("knowledge-recall-repo-cognitive-memory-store").unwrap();
    let outcome = dummy_outcome(
        "recall the cognitive memory storage backend",
        "Simard's cognitive memory subsystem uses the ladybug embedded store and persists to ~/.simard/cognitive_memory.ladybug",
        "verified via src/ inspection",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for a fully grounded cognitive-memory-store answer: {checks:?}"
    );
}

#[test]
fn class_checks_knowledge_recall_repo_engineer_worktree_pattern_passes_with_grounded_answer() {
    let scenario =
        resolve_benchmark_scenario("knowledge-recall-repo-engineer-worktree-pattern").unwrap();
    let outcome = dummy_outcome(
        "recall how the OODA daemon spawns engineer subagents into worktrees",
        "engineer worktrees live under ~/.simard/engineer-worktrees/ named engineer-<goal-id>-<timestamp>",
        "documented in src/ daemon notes",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for a fully grounded engineer-worktree-pattern answer: {checks:?}"
    );
}

#[test]
fn class_checks_knowledge_recall_repo_fail_when_ungrounded() {
    for id in [
        "knowledge-recall-repo-ooda-loop-layout",
        "knowledge-recall-repo-cognitive-memory-store",
        "knowledge-recall-repo-engineer-worktree-pattern",
    ] {
        let scenario = resolve_benchmark_scenario(id).unwrap();
        let outcome = dummy_outcome("nothing", "bland text", "empty");
        let exported = dummy_handoff(0);
        let checks = class_specific_checks(&scenario, &outcome, &exported);
        assert_eq!(checks.len(), 2);
        for check in &checks {
            assert!(
                !check.passed,
                "scenario {id}: check '{}' should have failed",
                check.id
            );
        }
    }
}
