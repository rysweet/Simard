//! Tests for the wave-12 gym scenarios added to address issue #1611.
//!
//! Covers the six scenarios in `data_10.rs` that extend the existing
//! `KnowledgeRecall` and `SelfIntrospection` families with tool-fluency,
//! user-preference, repo-knowledge, calibration/abstain, sha256 prefix
//! citation, and cycle-skip detection scenarios.

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

fn assert_knowledge_recall_shape(scenario: &BenchmarkScenario) {
    assert_eq!(scenario.class, BenchmarkClass::KnowledgeRecall);
    assert_eq!(scenario.identity, "simard-gym");
    assert_eq!(scenario.base_type, "rusty-clawd");
    assert_eq!(scenario.topology, RuntimeTopology::SingleProcess);
    assert_eq!(scenario.expected_min_runtime_evidence, 2);
}

fn assert_self_introspection_shape(scenario: &BenchmarkScenario) {
    assert_eq!(scenario.class, BenchmarkClass::SelfIntrospection);
    assert_eq!(scenario.identity, "simard-gym");
    assert_eq!(scenario.base_type, "rusty-clawd");
    assert_eq!(scenario.topology, RuntimeTopology::SingleProcess);
}

// --- Registration tests ---------------------------------------------------

#[test]
fn knowledge_recall_tool_cargo_clippy_strict_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-cargo-clippy-strict")
        .expect("knowledge-recall-tool-cargo-clippy-strict scenario must be registered");
    assert_knowledge_recall_shape(&scenario);
}

#[test]
fn knowledge_recall_user_pref_no_sycophancy_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-user-pref-no-sycophancy")
        .expect("knowledge-recall-user-pref-no-sycophancy scenario must be registered");
    assert_knowledge_recall_shape(&scenario);
}

#[test]
fn knowledge_recall_repo_cycle_reports_dir_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-repo-cycle-reports-dir")
        .expect("knowledge-recall-repo-cycle-reports-dir scenario must be registered");
    assert_knowledge_recall_shape(&scenario);
}

#[test]
fn self_introspection_l9_abstain_resolves() {
    let scenario = resolve_benchmark_scenario("self-introspection-l9-abstain-on-missing-cycle")
        .expect("self-introspection-l9-abstain-on-missing-cycle scenario must be registered");
    assert_self_introspection_shape(&scenario);
    assert_eq!(scenario.expected_min_runtime_evidence, 1);
}

#[test]
fn self_introspection_l10_prefix_cited_resolves() {
    let scenario = resolve_benchmark_scenario("self-introspection-l10-prompt-version-prefix-cited")
        .expect("self-introspection-l10-prompt-version-prefix-cited scenario must be registered");
    assert_self_introspection_shape(&scenario);
    assert_eq!(scenario.expected_min_runtime_evidence, 2);
}

#[test]
fn self_introspection_l11_skipped_cycle_resolves() {
    let scenario = resolve_benchmark_scenario("self-introspection-l11-skipped-cycle-detection")
        .expect("self-introspection-l11-skipped-cycle-detection scenario must be registered");
    assert_self_introspection_shape(&scenario);
    assert_eq!(scenario.expected_min_runtime_evidence, 2);
}

// --- Check passes when fully grounded ------------------------------------

#[test]
fn class_checks_cargo_clippy_strict_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-tool-cargo-clippy-strict").unwrap();
    let outcome = dummy_outcome(
        "consult docs/development.md for the pre-push lint command",
        "the pre-push hook runs cargo clippy --all-targets --all-features --locked -- -D warnings; the -D warnings flag promotes lints to errors so the gate fails on any new warning",
        "verified in src/ pre-push hook script",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for grounded clippy answer: {checks:?}"
    );
}

#[test]
fn class_checks_no_sycophancy_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-user-pref-no-sycophancy").unwrap();
    let outcome = dummy_outcome(
        "recall the user's no-sycophancy stance from TRUST.md",
        "the user prohibits sycophantic openers like 'Great idea!' or 'Excellent point!'; this stance is codified in docs/TRUST.md as the no-sycophancy guideline",
        "documented in docs/",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for grounded sycophancy answer: {checks:?}"
    );
}

#[test]
fn class_checks_cycle_reports_dir_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-repo-cycle-reports-dir").unwrap();
    let outcome = dummy_outcome(
        "recall cycle_reports/ on-disk layout",
        "Simard writes one JSON file per cycle to cycle_reports/, named cycle-NNNN.json; the persist_cycle_report function in src/operator_commands_ooda/persistence.rs hand-rolls the BrainJudgmentRecord serialisation",
        "verified in src/ operator commands ooda persistence",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for grounded cycle_reports answer: {checks:?}"
    );
}

#[test]
fn class_checks_l9_abstain_passes_with_calibrated_answer() {
    let scenario =
        resolve_benchmark_scenario("self-introspection-l9-abstain-on-missing-cycle").unwrap();
    let outcome = dummy_outcome(
        "no cycle-report data is embedded; refuse to confabulate",
        "I cannot answer this question without the cycle 99 cycle-report; please provide the cycle_reports/ entry or I must refuse to invent a decision verb",
        "calibration documented in src/",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all SelfIntrospection checks should pass for calibrated abstain answer: {checks:?}"
    );
}

#[test]
fn class_checks_l9_abstain_fails_when_confabulating() {
    let scenario =
        resolve_benchmark_scenario("self-introspection-l9-abstain-on-missing-cycle").unwrap();
    let outcome = dummy_outcome(
        "answer cycle 99 question",
        "in cycle 99 the decide brain chose dispatch_engineer for the calibration-canary goal",
        "confabulated reflection",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    let topic_check = checks
        .iter()
        .find(|c| c.id == "self-introspection-canonical-token-cited")
        .unwrap();
    assert!(
        !topic_check.passed,
        "L9 calibration check must fail when the agent confabulates dispatch_engineer instead of abstaining"
    );
}

#[test]
fn class_checks_l10_prefix_passes_with_grounded_answer() {
    let scenario =
        resolve_benchmark_scenario("self-introspection-l10-prompt-version-prefix-cited").unwrap();
    let outcome = dummy_outcome(
        "extract the sha256 prefix from cycle 12 decide judgment",
        "the cycle 12 decide judgment for add-more-gym-benchmark-scenarios carries prompt_version=ggg777000888; the LLM brain produced this judgment because the prefix is non-empty",
        "schema source is src/ooda_brain/judgment_record.rs",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all SelfIntrospection checks should pass for grounded L10 answer: {checks:?}"
    );
}

#[test]
fn class_checks_l11_skipped_cycle_passes_with_grounded_answer() {
    let scenario =
        resolve_benchmark_scenario("self-introspection-l11-skipped-cycle-detection").unwrap();
    let outcome = dummy_outcome(
        "compare cycle 13 and cycle 17 decide reports",
        "cycles 14, 15, and 16 are missing — there is a gap of 3 cycles between cycle 13 and cycle 17 in cycle_reports/; this can indicate a daemon restart or observe-only cycles that wrote no decide-phase judgment",
        "verified in cycle_reports/",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all SelfIntrospection checks should pass for grounded L11 answer: {checks:?}"
    );
}

// --- Negative path: ungrounded answers fail ------------------------------

#[test]
fn class_checks_wave12_fail_when_ungrounded() {
    for id in [
        "knowledge-recall-tool-cargo-clippy-strict",
        "knowledge-recall-user-pref-no-sycophancy",
        "knowledge-recall-repo-cycle-reports-dir",
        "self-introspection-l10-prompt-version-prefix-cited",
        "self-introspection-l11-skipped-cycle-detection",
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
