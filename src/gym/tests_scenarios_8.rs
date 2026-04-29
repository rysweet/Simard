//! Tests for the KnowledgeRecall **cross-session** sub-family (issue #1459).
//!
//! These cover the two scenarios added by the capstone PR for the
//! `KnowledgeRecall` family roadmap from #1459:
//! `knowledge-recall-cross-session-fact` and
//! `knowledge-recall-cross-session-preference`. Each scenario directly
//! stress-tests cognitive memory persistence across session boundaries —
//! the same subsystem the still-wedged `improve-cognitive-memory-persistence`
//! daemon goal is trying to fix.

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

fn assert_cross_session_scenario_shape(scenario: &BenchmarkScenario) {
    assert_eq!(scenario.class, BenchmarkClass::KnowledgeRecall);
    assert_eq!(scenario.identity, "simard-gym");
    assert_eq!(scenario.base_type, "rusty-clawd");
    assert_eq!(scenario.topology, RuntimeTopology::SingleProcess);
    assert_eq!(scenario.expected_min_runtime_evidence, 2);
}

#[test]
fn knowledge_recall_cross_session_fact_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-cross-session-fact")
        .expect("knowledge-recall-cross-session-fact scenario must be registered");
    assert_cross_session_scenario_shape(&scenario);
}

#[test]
fn knowledge_recall_cross_session_preference_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-cross-session-preference")
        .expect("knowledge-recall-cross-session-preference scenario must be registered");
    assert_cross_session_scenario_shape(&scenario);
}

#[test]
fn class_checks_knowledge_recall_cross_session_fact_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-cross-session-fact").unwrap();
    let outcome = dummy_outcome(
        "consult accumulated cognitive memory for the prior session canary",
        "the gym-cross-session-canary memory from the prior session carries token CANARY-42 in cognitive memory",
        "verified accumulated record from previous session in src/ cognitive_memory store",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for a fully grounded cross-session-fact answer: {checks:?}"
    );
}

#[test]
fn class_checks_knowledge_recall_cross_session_preference_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-cross-session-preference").unwrap();
    let outcome = dummy_outcome(
        "recall the prompt-driven brain preference stated in a prior session",
        "the user mandated a prompt-driven brain on Apr 29; the resulting pattern is prompt_assets/simard/*.md plus include_str! plus the OodaBrain LLM trait",
        "documented in src/ prompt assets",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for a fully grounded cross-session-preference answer: {checks:?}"
    );
}

#[test]
fn class_checks_knowledge_recall_cross_session_fail_when_ungrounded() {
    for id in [
        "knowledge-recall-cross-session-fact",
        "knowledge-recall-cross-session-preference",
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
