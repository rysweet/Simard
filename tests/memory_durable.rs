use serde_json::json;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use simard::bridge::BridgeErrorPayload;
use simard::bridge_subprocess::InMemoryBridgeTransport;
use simard::memory_bridge::CognitiveMemoryBridge;
use simard::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveWorkingSlot,
};
use simard::memory_consolidation::{
    FactExtraction, execution_memory_operations, intake_memory_operations,
    persistence_memory_operations, preparation_memory_operations, reflection_memory_operations,
};
use simard::memory_hive::{DEFAULT_CONFIDENCE_GATE, DEFAULT_QUALITY_THRESHOLD, HiveConfig};
use simard::session::SessionId;

// ---------------------------------------------------------------------------
// Stateful in-memory mock that supports store-then-search-back patterns
// ---------------------------------------------------------------------------

fn stateful_bridge() -> CognitiveMemoryBridge {
    let facts: Arc<Mutex<Vec<CognitiveFact>>> = Arc::new(Mutex::new(Vec::new()));
    let slots: Arc<Mutex<Vec<CognitiveWorkingSlot>>> = Arc::new(Mutex::new(Vec::new()));
    let procs: Arc<Mutex<Vec<CognitiveProcedure>>> = Arc::new(Mutex::new(Vec::new()));
    let pros: Arc<Mutex<Vec<CognitiveProspective>>> = Arc::new(Mutex::new(Vec::new()));
    let epi_n: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let sen_n: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let (f, s, p, pr, ec, sc) = (
        facts.clone(),
        slots.clone(),
        procs.clone(),
        pros.clone(),
        epi_n.clone(),
        sen_n.clone(),
    );

    let transport =
        InMemoryBridgeTransport::new("stateful-memory", move |method, params| match method {
            "memory.store_fact" => {
                let mut g = f.lock().unwrap();
                let id = format!("sem_{:04}", g.len());
                let tags: Vec<String> = params["tags"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                g.push(CognitiveFact {
                    node_id: id.clone(),
                    concept: params["concept"].as_str().unwrap_or("").into(),
                    content: params["content"].as_str().unwrap_or("").into(),
                    confidence: params["confidence"].as_f64().unwrap_or(1.0),
                    source_id: params["source_id"].as_str().unwrap_or("").into(),
                    tags,
                });
                Ok(json!({"id": id}))
            }
            "memory.search_facts" => {
                let q = params["query"].as_str().unwrap_or("").to_lowercase();
                let lim = params["limit"].as_u64().unwrap_or(10) as usize;
                let mc = params["min_confidence"].as_f64().unwrap_or(0.0);
                let hits: Vec<_> = f
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|f| {
                        f.confidence >= mc
                            && (f.concept.to_lowercase().contains(&q)
                                || f.content.to_lowercase().contains(&q))
                    })
                    .take(lim)
                    .cloned()
                    .collect();
                Ok(json!({"facts": hits.iter().map(|f| json!({
                    "node_id": f.node_id, "concept": f.concept, "content": f.content,
                    "confidence": f.confidence, "source_id": f.source_id, "tags": f.tags,
                })).collect::<Vec<_>>()}))
            }
            "memory.push_working" => {
                let mut g = s.lock().unwrap();
                let id = format!("wrk_{:04}", g.len());
                g.push(CognitiveWorkingSlot {
                    node_id: id.clone(),
                    slot_type: params["slot_type"].as_str().unwrap_or("").into(),
                    content: params["content"].as_str().unwrap_or("").into(),
                    relevance: params["relevance"].as_f64().unwrap_or(1.0),
                    task_id: params["task_id"].as_str().unwrap_or("").into(),
                });
                Ok(json!({"id": id}))
            }
            "memory.get_working" => {
                let tid = params["task_id"].as_str().unwrap_or("");
                let r: Vec<_> = s
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|sl| sl.task_id == tid)
                    .cloned()
                    .collect();
                Ok(json!({"slots": r.iter().map(|sl| json!({
                    "node_id": sl.node_id, "slot_type": sl.slot_type,
                    "content": sl.content, "relevance": sl.relevance, "task_id": sl.task_id,
                })).collect::<Vec<_>>()}))
            }
            "memory.clear_working" => {
                let tid = params["task_id"].as_str().unwrap_or("");
                let mut g = s.lock().unwrap();
                let before = g.len();
                g.retain(|sl| sl.task_id != tid);
                Ok(json!({"count": before - g.len()}))
            }
            "memory.record_sensory" => {
                Ok(json!({"id": format!("sen_{:04}", sc.fetch_add(1, Ordering::SeqCst))}))
            }
            "memory.prune_expired_sensory" => Ok(json!({"count": 0})),
            "memory.store_episode" => {
                Ok(json!({"id": format!("epi_{:04}", ec.fetch_add(1, Ordering::SeqCst))}))
            }
            "memory.consolidate_episodes" => {
                let cnt = ec.load(Ordering::SeqCst);
                let bs = params["batch_size"].as_u64().unwrap_or(10) as u32;
                Ok(if cnt >= bs {
                    json!({"id": format!("con_{cnt:04}")})
                } else {
                    json!({"id": null})
                })
            }
            "memory.store_procedure" => {
                let mut g = p.lock().unwrap();
                let id = format!("proc_{:04}", g.len());
                let steps: Vec<String> = params["steps"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let prereqs: Vec<String> = params["prerequisites"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                g.push(CognitiveProcedure {
                    node_id: id.clone(),
                    name: params["name"].as_str().unwrap_or("").into(),
                    steps,
                    prerequisites: prereqs,
                    usage_count: 0,
                });
                Ok(json!({"id": id}))
            }
            "memory.recall_procedure" => {
                let q = params["query"].as_str().unwrap_or("").to_lowercase();
                let lim = params["limit"].as_u64().unwrap_or(5) as usize;
                let r: Vec<_> = p
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|pr| pr.name.to_lowercase().contains(&q))
                    .take(lim)
                    .cloned()
                    .collect();
                Ok(json!({"procedures": r.iter().map(|pr| json!({
                    "node_id": pr.node_id, "name": pr.name, "steps": pr.steps,
                    "prerequisites": pr.prerequisites, "usage_count": pr.usage_count,
                })).collect::<Vec<_>>()}))
            }
            "memory.store_prospective" => {
                let mut g = pr.lock().unwrap();
                let id = format!("pro_{:04}", g.len());
                g.push(CognitiveProspective {
                    node_id: id.clone(),
                    description: params["description"].as_str().unwrap_or("").into(),
                    trigger_condition: params["trigger_condition"].as_str().unwrap_or("").into(),
                    action_on_trigger: params["action_on_trigger"].as_str().unwrap_or("").into(),
                    status: "pending".into(),
                    priority: params["priority"].as_i64().unwrap_or(1),
                });
                Ok(json!({"id": id}))
            }
            "memory.check_triggers" => {
                let c = params["content"].as_str().unwrap_or("").to_lowercase();
                let t: Vec<_> = pr
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|pm| {
                        pm.status == "pending"
                            && pm
                                .trigger_condition
                                .split_whitespace()
                                .any(|w| c.contains(&w.to_lowercase()))
                    })
                    .cloned()
                    .map(|mut pm| {
                        pm.status = "triggered".into();
                        pm
                    })
                    .collect();
                Ok(json!({"prospectives": t.iter().map(|pm| json!({
                    "node_id": pm.node_id, "description": pm.description,
                    "trigger_condition": pm.trigger_condition,
                    "action_on_trigger": pm.action_on_trigger,
                    "status": pm.status, "priority": pm.priority,
                })).collect::<Vec<_>>()}))
            }
            "memory.get_statistics" => Ok(json!({
                "sensory_count": sc.load(Ordering::SeqCst) as u64,
                "working_count": s.lock().unwrap().len() as u64,
                "episodic_count": ec.load(Ordering::SeqCst) as u64,
                "semantic_count": f.lock().unwrap().len() as u64,
                "procedural_count": p.lock().unwrap().len() as u64,
                "prospective_count": pr.lock().unwrap().len() as u64,
            })),
            _ => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        });
    CognitiveMemoryBridge::new(Box::new(transport))
}

fn test_session_id() -> SessionId {
    SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap()
}

// --- Store fact via bridge, search it back ---

#[test]
fn store_fact_and_search_back() {
    let bridge = stateful_bridge();
    let id = bridge
        .store_fact(
            "rust",
            "Rust is a systems language",
            0.95,
            &["lang".into()],
            "manual",
        )
        .unwrap();
    assert!(id.starts_with("sem_"));
    let facts = bridge.search_facts("rust", 10, 0.0).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "rust");
    assert!((facts[0].confidence - 0.95).abs() < f64::EPSILON);
}

#[test]
fn search_facts_respects_min_confidence() {
    let bridge = stateful_bridge();
    bridge.store_fact("low", "not sure", 0.2, &[], "").unwrap();
    bridge
        .store_fact("high", "very sure", 0.9, &[], "")
        .unwrap();
    let facts = bridge.search_facts("sure", 10, 0.5).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "high");
}

// --- Store episode, consolidate, verify ---

#[test]
fn store_episodes_and_consolidate() {
    let bridge = stateful_bridge();
    for i in 0..3 {
        bridge
            .store_episode(&format!("episode {i}"), "test", None)
            .unwrap();
    }
    let consolidated = bridge.consolidate_episodes(3).unwrap();
    assert!(consolidated.is_some());
    assert!(consolidated.unwrap().starts_with("con_"));
}

#[test]
fn consolidate_returns_none_when_insufficient() {
    let bridge = stateful_bridge();
    bridge.store_episode("only one", "test", None).unwrap();
    assert!(bridge.consolidate_episodes(10).unwrap().is_none());
}

// --- Push/get/clear working memory slots ---

#[test]
fn push_get_clear_working_memory() {
    let bridge = stateful_bridge();
    let id1 = bridge
        .push_working("goal", "build feature", "t1", 1.0)
        .unwrap();
    let id2 = bridge
        .push_working("constraint", "must be fast", "t1", 0.8)
        .unwrap();
    assert_ne!(id1, id2);
    assert_eq!(bridge.get_working("t1").unwrap().len(), 2);
    assert_eq!(bridge.clear_working("t1").unwrap(), 2);
    assert!(bridge.get_working("t1").unwrap().is_empty());
}

#[test]
fn working_memory_isolates_by_task_id() {
    let bridge = stateful_bridge();
    bridge.push_working("goal", "task A", "a", 1.0).unwrap();
    bridge.push_working("goal", "task B", "b", 1.0).unwrap();
    assert_eq!(bridge.get_working("a").unwrap().len(), 1);
    assert_eq!(bridge.get_working("b").unwrap().len(), 1);
}

// --- Record and prune sensory items ---

#[test]
fn record_and_prune_sensory() {
    let bridge = stateful_bridge();
    let id = bridge.record_sensory("text", "hello", 300).unwrap();
    assert!(id.starts_with("sen_"));
    assert_eq!(bridge.prune_expired_sensory().unwrap(), 0);
}

// --- Store and recall procedures ---

#[test]
fn store_and_recall_procedure() {
    let bridge = stateful_bridge();
    let id = bridge
        .store_procedure(
            "deploy",
            &["build".into(), "test".into(), "push".into()],
            &["clean".into()],
        )
        .unwrap();
    assert!(id.starts_with("proc_"));
    let procs = bridge.recall_procedure("deploy", 5).unwrap();
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0].steps.len(), 3);
    assert_eq!(procs[0].prerequisites, vec!["clean"]);
}

#[test]
fn recall_procedure_filters_by_query() {
    let bridge = stateful_bridge();
    bridge
        .store_procedure("deploy", &["push".into()], &[])
        .unwrap();
    bridge
        .store_procedure("build", &["compile".into()], &[])
        .unwrap();
    assert_eq!(bridge.recall_procedure("deploy", 5).unwrap().len(), 1);
}

// --- Store prospective, check triggers ---

#[test]
fn store_prospective_and_check_triggers() {
    let bridge = stateful_bridge();
    let id = bridge
        .store_prospective("watch errors", "error compile", "cargo fix", 5)
        .unwrap();
    assert!(id.starts_with("pro_"));
    assert!(
        bridge
            .check_triggers("all tests passed")
            .unwrap()
            .is_empty()
    );
    let triggered = bridge.check_triggers("found a compilation error").unwrap();
    assert_eq!(triggered.len(), 1);
    assert_eq!(triggered[0].status, "triggered");
    assert_eq!(triggered[0].action_on_trigger, "cargo fix");
}

// --- Session lifecycle operations (intake through persistence) ---

#[test]
fn full_session_lifecycle() {
    let bridge = stateful_bridge();
    let session = test_session_id();

    intake_memory_operations("build the widget", &session, &bridge).unwrap();
    assert!(!bridge.get_working(session.as_str()).unwrap().is_empty());

    let ctx = preparation_memory_operations("build the widget", &session, &bridge).unwrap();
    assert!(ctx.relevant_facts.is_empty());

    execution_memory_operations("$ cargo build\n   Compiling...", &session, &bridge).unwrap();

    let facts = vec![FactExtraction {
        concept: "widget".into(),
        content: "Widget builds with cargo".into(),
        confidence: 0.85,
    }];
    reflection_memory_operations("transcript...", &facts, &session, &bridge).unwrap();
    assert_eq!(bridge.search_facts("widget", 10, 0.0).unwrap().len(), 1);

    persistence_memory_operations(&session, &bridge).unwrap();
    assert!(bridge.get_working(session.as_str()).unwrap().is_empty());
}

// --- Statistics ---

#[test]
fn statistics_reflect_stored_items() {
    let bridge = stateful_bridge();
    bridge.store_fact("test", "content", 0.9, &[], "").unwrap();
    bridge.record_sensory("text", "data", 300).unwrap();
    bridge.push_working("goal", "work", "t1", 1.0).unwrap();
    let stats = bridge.get_statistics().unwrap();
    assert_eq!(stats.semantic_count, 1);
    assert_eq!(stats.sensory_count, 1);
    assert_eq!(stats.working_count, 1);
    assert_eq!(stats.total(), 3);
}

// --- Feral tests ---

#[test]
fn feral_empty_concept_still_stores() {
    let bridge = stateful_bridge();
    assert!(
        bridge
            .store_fact("", "content", 0.5, &[], "")
            .unwrap()
            .starts_with("sem_")
    );
}

#[test]
fn feral_confidence_boundary_values() {
    let bridge = stateful_bridge();
    bridge
        .store_fact("zero", "zero conf", 0.0, &[], "")
        .unwrap();
    bridge.store_fact("one", "full conf", 1.0, &[], "").unwrap();
    let facts = bridge.search_facts("conf", 10, 1.0).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "one");
}

#[test]
fn feral_large_payload() {
    let bridge = stateful_bridge();
    let big = "x".repeat(100_000);
    bridge.store_fact("big", &big, 0.5, &[], "").unwrap();
    assert_eq!(
        bridge.search_facts("big", 10, 0.0).unwrap()[0]
            .content
            .len(),
        100_000
    );
}

#[test]
fn feral_unknown_method_returns_error() {
    let transport = InMemoryBridgeTransport::new("test", |method, _| {
        Err(BridgeErrorPayload {
            code: -32601,
            message: format!("unknown: {method}"),
        })
    });
    assert!(
        CognitiveMemoryBridge::new(Box::new(transport))
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
    assert!(c.should_import(0.5));
    assert!(!c.should_import(0.3));
    assert!(c.should_promote(0.7));
    assert!(!c.should_promote(0.5));
}
