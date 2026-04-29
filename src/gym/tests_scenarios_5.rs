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

fn repo_exploration_scenario() -> BenchmarkScenario {
    BenchmarkScenario {
        id: "test-repo-exp",
        title: "Test Repo Exploration",
        description: "test",
        class: BenchmarkClass::RepoExploration,
        identity: "test",
        base_type: "test",
        topology: RuntimeTopology::SingleProcess,
        objective: "test",
        expected_min_runtime_evidence: 1,
    }
}

#[test]
fn class_checks_release_management_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::ReleaseManagement,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("nothing", "bland text", "empty");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    for check in &checks {
        assert!(!check.passed, "check '{}' should have failed", check.id);
    }
}

// -- Wave 8: AccessibilityReview / InternationalizationReview / IncidentResponse --

#[test]
fn class_checks_accessibility_review_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::AccessibilityReview,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "audit aria roles, missing alt text, label association, and keyboard focus",
        "cite WCAG 2.1.1 and 4.1.2 success criterion at level AA",
        "remediation: add aria-label, fix focus order, replace contrast pair",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "a11y-issues-identified" && c.passed)
    );
    assert!(checks.iter().any(|c| c.id == "a11y-wcag-cited" && c.passed));
    assert!(
        checks
            .iter()
            .any(|c| c.id == "a11y-remediation-proposed" && c.passed)
    );
}

#[test]
fn class_checks_accessibility_review_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::AccessibilityReview,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("nothing", "bland text", "empty");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    for check in &checks {
        assert!(!check.passed, "check '{}' should have failed", check.id);
    }
}

#[test]
fn class_checks_internationalization_review_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::InternationalizationReview,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "inventory hardcoded string literals and route to message catalog",
        "negotiate locale via Accept-Language with CLDR fallback (en-US, pt-BR)",
        "address plural categories, RTL/bidi mirroring, and date format via ICU MessageFormat",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "i18n-localizable-strings-identified" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "i18n-locale-handling-described" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "i18n-pluralization-or-format-addressed" && c.passed)
    );
}

#[test]
fn class_checks_internationalization_review_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::InternationalizationReview,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("nothing", "bland text", "empty");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    for check in &checks {
        assert!(!check.passed, "check '{}' should have failed", check.id);
    }
}

#[test]
fn class_checks_incident_response_passes_with_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::IncidentResponse,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome(
        "reconstruct timeline: alert paged, mitigation started, resolved at",
        "blameless analysis: root cause distinct from trigger, latent contributing factor",
        "follow-up runbook with on-call escalation and prevention action item",
    );
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "incident-timeline-reconstructed" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "incident-root-cause-or-contributing-identified" && c.passed)
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "incident-mitigation-or-followup-proposed" && c.passed)
    );
}

#[test]
fn class_checks_incident_response_fails_without_keywords() {
    let scenario = BenchmarkScenario {
        class: BenchmarkClass::IncidentResponse,
        ..repo_exploration_scenario()
    };
    let outcome = dummy_outcome("nothing", "bland text", "empty");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 3);
    for check in &checks {
        assert!(!check.passed, "check '{}' should have failed", check.id);
    }
}

// -- Wave 9: KnowledgeRecall (issue #1459) --

#[test]
fn knowledge_recall_self_code_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-self-code")
        .expect("knowledge-recall-self-code scenario must be registered");
    assert_eq!(scenario.class, BenchmarkClass::KnowledgeRecall);
    assert_eq!(scenario.identity, "simard-gym");
    assert_eq!(scenario.base_type, "rusty-clawd");
    assert_eq!(scenario.expected_min_runtime_evidence, 2);
}

#[test]
fn knowledge_recall_user_preference_scenario_resolves() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-user-preference")
        .expect("knowledge-recall-user-preference scenario must be registered");
    assert_eq!(scenario.class, BenchmarkClass::KnowledgeRecall);
    assert_eq!(scenario.identity, "simard-gym");
    assert_eq!(scenario.base_type, "rusty-clawd");
    assert_eq!(scenario.expected_min_runtime_evidence, 2);
}

#[test]
fn class_checks_knowledge_recall_self_code_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-self-code").unwrap();
    let outcome = dummy_outcome(
        "review src/ooda_brain/mod.rs to locate the OodaBrain trait definition",
        "OodaBrain trait lives in src/ooda_brain/mod.rs and is wired in by the OODA action layer",
        "cited single wire-in site; trait is the brain entrypoint",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks
            .iter()
            .any(|c| c.id == "knowledge-recall-evidence-grounded" && c.passed),
        "evidence-grounded check should pass when memory records exist or a path is cited"
    );
    assert!(
        checks
            .iter()
            .any(|c| c.id == "knowledge-recall-topic-cited" && c.passed),
        "topic-cited check should pass when OodaBrain trait is named"
    );
}

#[test]
fn class_checks_knowledge_recall_user_preference_passes_with_grounded_answer() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-user-preference").unwrap();
    let outcome = dummy_outcome(
        "consult stored memory on user stance regarding --no-verify",
        "user mandates never using --no-verify; SKIP=cargo-test is the approved alternative for known-flaky local tests",
        "documented preference in cognitive memory",
    );
    let exported = dummy_handoff(1);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    assert!(
        checks.iter().all(|c| c.passed),
        "all KnowledgeRecall checks should pass for a fully grounded answer"
    );
}

#[test]
fn class_checks_knowledge_recall_fails_when_ungrounded() {
    let scenario = resolve_benchmark_scenario("knowledge-recall-self-code").unwrap();
    let outcome = dummy_outcome("nothing", "bland text", "empty");
    let exported = dummy_handoff(0);
    let checks = class_specific_checks(&scenario, &outcome, &exported);
    assert_eq!(checks.len(), 2);
    for check in &checks {
        assert!(!check.passed, "check '{}' should have failed", check.id);
    }
}
