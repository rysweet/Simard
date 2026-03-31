//! Stateful in-memory mock for cognitive memory bridge tests.

use serde_json::json;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use simard::bridge::BridgeErrorPayload;
use simard::bridge_subprocess::InMemoryBridgeTransport;
use simard::memory_bridge::CognitiveMemoryBridge;
use simard::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveWorkingSlot,
};

type Facts = Arc<Mutex<Vec<CognitiveFact>>>;
type Slots = Arc<Mutex<Vec<CognitiveWorkingSlot>>>;
type Procs = Arc<Mutex<Vec<CognitiveProcedure>>>;
type Pros = Arc<Mutex<Vec<CognitiveProspective>>>;

fn parse_string_list(val: &serde_json::Value) -> Vec<String> {
    val.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

pub fn stateful_bridge() -> CognitiveMemoryBridge {
    let f: Facts = Arc::default();
    let s: Slots = Arc::default();
    let p: Procs = Arc::default();
    let pr: Pros = Arc::default();
    let ec = Arc::new(AtomicU32::new(0));
    let sc = Arc::new(AtomicU32::new(0));
    let (fc, sc2, pc, prc, ecc, scc) = (
        f.clone(),
        s.clone(),
        p.clone(),
        pr.clone(),
        ec.clone(),
        sc.clone(),
    );

    let transport = InMemoryBridgeTransport::new("mem", move |method, par| match method {
        "memory.store_fact" => {
            let mut g = fc.lock().unwrap();
            let id = format!("sem_{:04}", g.len());
            g.push(CognitiveFact {
                node_id: id.clone(),
                concept: par["concept"].as_str().unwrap_or("").into(),
                content: par["content"].as_str().unwrap_or("").into(),
                confidence: par["confidence"].as_f64().unwrap_or(1.0),
                source_id: par["source_id"].as_str().unwrap_or("").into(),
                tags: parse_string_list(&par["tags"]),
            });
            Ok(json!({"id": id}))
        }
        "memory.search_facts" => {
            let q = par["query"].as_str().unwrap_or("").to_lowercase();
            let lim = par["limit"].as_u64().unwrap_or(10) as usize;
            let mc = par["min_confidence"].as_f64().unwrap_or(0.0);
            let hits: Vec<_> = fc
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
            let mut g = sc2.lock().unwrap();
            let id = format!("wrk_{:04}", g.len());
            g.push(CognitiveWorkingSlot {
                node_id: id.clone(),
                slot_type: par["slot_type"].as_str().unwrap_or("").into(),
                content: par["content"].as_str().unwrap_or("").into(),
                relevance: par["relevance"].as_f64().unwrap_or(1.0),
                task_id: par["task_id"].as_str().unwrap_or("").into(),
            });
            Ok(json!({"id": id}))
        }
        "memory.get_working" => {
            let tid = par["task_id"].as_str().unwrap_or("");
            let r: Vec<_> = sc2
                .lock()
                .unwrap()
                .iter()
                .filter(|sl| sl.task_id == tid)
                .cloned()
                .collect();
            Ok(json!({"slots": r.iter().map(|sl| json!({
                "node_id": sl.node_id, "slot_type": sl.slot_type, "content": sl.content,
                "relevance": sl.relevance, "task_id": sl.task_id,
            })).collect::<Vec<_>>()}))
        }
        "memory.clear_working" => {
            let tid = par["task_id"].as_str().unwrap_or("");
            let mut g = sc2.lock().unwrap();
            let b = g.len();
            g.retain(|sl| sl.task_id != tid);
            Ok(json!({"count": b - g.len()}))
        }
        "memory.record_sensory" => {
            Ok(json!({"id": format!("sen_{:04}", scc.fetch_add(1, Ordering::SeqCst))}))
        }
        "memory.prune_expired_sensory" => Ok(json!({"count": 0})),
        "memory.store_episode" => {
            Ok(json!({"id": format!("epi_{:04}", ecc.fetch_add(1, Ordering::SeqCst))}))
        }
        "memory.consolidate_episodes" => {
            let c = ecc.load(Ordering::SeqCst);
            let bs = par["batch_size"].as_u64().unwrap_or(10) as u32;
            Ok(if c >= bs {
                json!({"id": format!("con_{c:04}")})
            } else {
                json!({"id": null})
            })
        }
        "memory.store_procedure" => {
            let mut g = pc.lock().unwrap();
            let id = format!("proc_{:04}", g.len());
            g.push(CognitiveProcedure {
                node_id: id.clone(),
                name: par["name"].as_str().unwrap_or("").into(),
                steps: parse_string_list(&par["steps"]),
                prerequisites: parse_string_list(&par["prerequisites"]),
                usage_count: 0,
            });
            Ok(json!({"id": id}))
        }
        "memory.recall_procedure" => {
            let q = par["query"].as_str().unwrap_or("").to_lowercase();
            let lim = par["limit"].as_u64().unwrap_or(5) as usize;
            let r: Vec<_> = pc
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
            let mut g = prc.lock().unwrap();
            let id = format!("pro_{:04}", g.len());
            g.push(CognitiveProspective {
                node_id: id.clone(),
                description: par["description"].as_str().unwrap_or("").into(),
                trigger_condition: par["trigger_condition"].as_str().unwrap_or("").into(),
                action_on_trigger: par["action_on_trigger"].as_str().unwrap_or("").into(),
                status: "pending".into(),
                priority: par["priority"].as_i64().unwrap_or(1),
            });
            Ok(json!({"id": id}))
        }
        "memory.check_triggers" => {
            let c = par["content"].as_str().unwrap_or("").to_lowercase();
            let t: Vec<_> = prc
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
            "sensory_count": scc.load(Ordering::SeqCst) as u64,
            "working_count": sc2.lock().unwrap().len() as u64,
            "episodic_count": ecc.load(Ordering::SeqCst) as u64,
            "semantic_count": fc.lock().unwrap().len() as u64,
            "procedural_count": pc.lock().unwrap().len() as u64,
            "prospective_count": prc.lock().unwrap().len() as u64,
        })),
        _ => Err(BridgeErrorPayload {
            code: -32601,
            message: format!("unknown: {method}"),
        }),
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}
