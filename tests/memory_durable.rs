//! Integration tests for durable cognitive memory bridge.

mod fixtures;
use fixtures::memory_mock::stateful_bridge;

use simard::bridge::BridgeErrorPayload;
use simard::bridge_subprocess::InMemoryBridgeTransport;
use simard::memory_bridge::CognitiveMemoryBridge;
use simard::memory_consolidation::{
    FactExtraction, execution_memory_operations, intake_memory_operations,
    persistence_memory_operations, preparation_memory_operations, reflection_memory_operations,
};
use simard::memory_hive::{DEFAULT_CONFIDENCE_GATE, DEFAULT_QUALITY_THRESHOLD, HiveConfig};
use simard::session::SessionId;

fn test_session_id() -> SessionId {
    SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap()
}

// --- Store fact via bridge, search it back ---

#[test]
fn store_fact_and_search_back() {
    let b = stateful_bridge();
    let id = b
        .store_fact("rust", "Rust is systems", 0.95, &["lang".into()], "man")
        .unwrap();
    assert!(id.starts_with("sem_"));
    let facts = b.search_facts("rust", 10, 0.0).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "rust");
    assert!((facts[0].confidence - 0.95).abs() < f64::EPSILON);
}

#[test]
fn search_facts_respects_min_confidence() {
    let b = stateful_bridge();
    b.store_fact("low", "not sure", 0.2, &[], "").unwrap();
    b.store_fact("high", "very sure", 0.9, &[], "").unwrap();
    let facts = b.search_facts("sure", 10, 0.5).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "high");
}

// --- Store episode, consolidate, verify ---

#[test]
fn store_episodes_and_consolidate() {
    let b = stateful_bridge();
    for i in 0..3 {
        b.store_episode(&format!("ep {i}"), "test", None).unwrap();
    }
    let c = b.consolidate_episodes(3).unwrap();
    assert!(c.is_some() && c.unwrap().starts_with("con_"));
}

#[test]
fn consolidate_returns_none_when_insufficient() {
    let b = stateful_bridge();
    b.store_episode("one", "test", None).unwrap();
    assert!(b.consolidate_episodes(10).unwrap().is_none());
}

// --- Push/get/clear working memory slots ---

#[test]
fn push_get_clear_working_memory() {
    let b = stateful_bridge();
    let i1 = b.push_working("goal", "build", "t1", 1.0).unwrap();
    let i2 = b.push_working("constraint", "fast", "t1", 0.8).unwrap();
    assert_ne!(i1, i2);
    assert_eq!(b.get_working("t1").unwrap().len(), 2);
    assert_eq!(b.clear_working("t1").unwrap(), 2);
    assert!(b.get_working("t1").unwrap().is_empty());
}

#[test]
fn working_memory_isolates_by_task_id() {
    let b = stateful_bridge();
    b.push_working("g", "A", "a", 1.0).unwrap();
    b.push_working("g", "B", "b", 1.0).unwrap();
    assert_eq!(b.get_working("a").unwrap().len(), 1);
    assert_eq!(b.get_working("b").unwrap().len(), 1);
}

// --- Record and prune sensory items ---

#[test]
fn record_and_prune_sensory() {
    let b = stateful_bridge();
    assert!(
        b.record_sensory("text", "hello", 300)
            .unwrap()
            .starts_with("sen_")
    );
    assert_eq!(b.prune_expired_sensory().unwrap(), 0);
}

// --- Store and recall procedures ---

#[test]
fn store_and_recall_procedure() {
    let b = stateful_bridge();
    let id = b
        .store_procedure(
            "deploy",
            &["build".into(), "test".into()],
            &["clean".into()],
        )
        .unwrap();
    assert!(id.starts_with("proc_"));
    let procs = b.recall_procedure("deploy", 5).unwrap();
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0].prerequisites, vec!["clean"]);
}

#[test]
fn recall_procedure_filters_by_query() {
    let b = stateful_bridge();
    b.store_procedure("deploy", &["push".into()], &[]).unwrap();
    b.store_procedure("build", &["compile".into()], &[])
        .unwrap();
    assert_eq!(b.recall_procedure("deploy", 5).unwrap().len(), 1);
}

// --- Store prospective, check triggers ---

#[test]
fn store_prospective_and_check_triggers() {
    let b = stateful_bridge();
    let id = b
        .store_prospective("watch errors", "error compile", "cargo fix", 5)
        .unwrap();
    assert!(id.starts_with("pro_"));
    assert!(b.check_triggers("all tests passed").unwrap().is_empty());
    let t = b.check_triggers("found a compilation error").unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].status, "triggered");
    assert_eq!(t[0].action_on_trigger, "cargo fix");
}

// --- Session lifecycle operations (intake through persistence) ---

#[test]
fn full_session_lifecycle() {
    let b = stateful_bridge();
    let sid = test_session_id();
    intake_memory_operations("build widget", &sid, &b).unwrap();
    assert!(!b.get_working(sid.as_str()).unwrap().is_empty());
    let ctx = preparation_memory_operations("build widget", &sid, &b).unwrap();
    assert!(ctx.relevant_facts.is_empty());
    execution_memory_operations("$ cargo build\nCompiling...", &sid, &b).unwrap();
    let facts = vec![FactExtraction {
        concept: "widget".into(),
        content: "builds".into(),
        confidence: 0.85,
    }];
    reflection_memory_operations("transcript", &facts, &sid, &b).unwrap();
    assert_eq!(b.search_facts("widget", 10, 0.0).unwrap().len(), 1);
    persistence_memory_operations(&sid, &b).unwrap();
    assert!(b.get_working(sid.as_str()).unwrap().is_empty());
}

// --- Statistics ---

#[test]
fn statistics_reflect_stored_items() {
    let b = stateful_bridge();
    b.store_fact("t", "c", 0.9, &[], "").unwrap();
    b.record_sensory("text", "d", 300).unwrap();
    b.push_working("g", "w", "t1", 1.0).unwrap();
    let s = b.get_statistics().unwrap();
    assert_eq!(
        (s.semantic_count, s.sensory_count, s.working_count),
        (1, 1, 1)
    );
    assert_eq!(s.total(), 3);
}

// --- Feral tests ---

#[test]
fn feral_empty_concept_still_stores() {
    assert!(
        stateful_bridge()
            .store_fact("", "c", 0.5, &[], "")
            .unwrap()
            .starts_with("sem_")
    );
}

#[test]
fn feral_confidence_boundary_values() {
    let b = stateful_bridge();
    b.store_fact("zero", "z conf", 0.0, &[], "").unwrap();
    b.store_fact("one", "f conf", 1.0, &[], "").unwrap();
    let f = b.search_facts("conf", 10, 1.0).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].concept, "one");
}

#[test]
fn feral_large_payload() {
    let b = stateful_bridge();
    let big = "x".repeat(100_000);
    b.store_fact("big", &big, 0.5, &[], "").unwrap();
    assert_eq!(
        b.search_facts("big", 10, 0.0).unwrap()[0].content.len(),
        100_000
    );
}

#[test]
fn feral_unknown_method_returns_error() {
    let t = InMemoryBridgeTransport::new("t", |m, _| {
        Err(BridgeErrorPayload {
            code: -32601,
            message: format!("unknown: {m}"),
        })
    });
    assert!(
        CognitiveMemoryBridge::new(Box::new(t))
            .get_statistics()
            .is_err()
    );
}

// --- Hive config ---

#[test]
fn hive_config_defaults_match_python() {
    assert!((DEFAULT_QUALITY_THRESHOLD - 0.3).abs() < f64::EPSILON);
    assert!((DEFAULT_CONFIDENCE_GATE - 0.3).abs() < f64::EPSILON);
}

#[test]
fn hive_config_validation_and_gates() {
    assert!(HiveConfig::new("a", 0.5, 0.5).validate().is_ok());
    assert!(HiveConfig::new("a", 1.5, 0.5).validate().is_err());
    assert!(HiveConfig::new("a", 0.5, -0.01).validate().is_err());
    let c = HiveConfig::new("a", 0.4, 0.6);
    assert!(c.should_import(0.5) && !c.should_import(0.3));
    assert!(c.should_promote(0.7) && !c.should_promote(0.5));
}
