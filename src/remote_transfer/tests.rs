use super::*;
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::memory_bridge::CognitiveMemoryBridge;
use serde_json::json;
use std::sync::Mutex;

struct MockStore {
    facts: Vec<CognitiveFact>,
    procedures: Vec<CognitiveProcedure>,
}

fn mock_bridge() -> CognitiveMemoryBridge {
    let store: &'static Mutex<MockStore> = Box::leak(Box::new(Mutex::new(MockStore {
        facts: vec![],
        procedures: vec![],
    })));

    let transport =
        InMemoryBridgeTransport::new("test-memory", move |method, params| match method {
            "memory.search_facts" => {
                let s = store.lock().unwrap();
                let facts: Vec<serde_json::Value> = s
                    .facts
                    .iter()
                    .map(|f| {
                        json!({
                            "node_id": f.node_id, "concept": f.concept,
                            "content": f.content, "confidence": f.confidence,
                            "source_id": f.source_id, "tags": f.tags,
                        })
                    })
                    .collect();
                Ok(json!({"facts": facts}))
            }
            "memory.recall_procedure" => {
                let s = store.lock().unwrap();
                let procs: Vec<serde_json::Value> = s
                    .procedures
                    .iter()
                    .map(|p| {
                        json!({
                            "node_id": p.node_id, "name": p.name,
                            "steps": p.steps, "prerequisites": p.prerequisites,
                            "usage_count": p.usage_count,
                        })
                    })
                    .collect();
                Ok(json!({"procedures": procs}))
            }
            "memory.store_fact" => {
                let mut s = store.lock().unwrap();
                let id = format!("fact-{}", s.facts.len() + 1);
                s.facts.push(CognitiveFact {
                    node_id: id.clone(),
                    concept: params["concept"].as_str().unwrap_or("").to_string(),
                    content: params["content"].as_str().unwrap_or("").to_string(),
                    confidence: params["confidence"].as_f64().unwrap_or(0.0),
                    source_id: params["source_id"].as_str().unwrap_or("").to_string(),
                    tags: params["tags"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                });
                Ok(json!({"id": id}))
            }
            "memory.store_procedure" => {
                let mut s = store.lock().unwrap();
                let id = format!("proc-{}", s.procedures.len() + 1);
                s.procedures.push(CognitiveProcedure {
                    node_id: id.clone(),
                    name: params["name"].as_str().unwrap_or("").to_string(),
                    steps: params["steps"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                    prerequisites: params["prerequisites"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                    usage_count: 0,
                });
                Ok(json!({"id": id}))
            }
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    CognitiveMemoryBridge::new(Box::new(transport))
}

#[test]
fn export_empty_bridge_returns_empty_snapshot() {
    let bridge = mock_bridge();
    let snapshot = export_memory_snapshot(&bridge, "test-agent", None).unwrap();
    assert!(snapshot.is_empty());
    assert_eq!(snapshot.total_items(), 0);
    assert_eq!(snapshot.source_agent, "test-agent");
    assert!(snapshot.exported_at > 0);
}

#[test]
fn export_rejects_empty_agent_name() {
    let bridge = mock_bridge();
    let err = export_memory_snapshot(&bridge, "", None).unwrap_err();
    assert!(matches!(err, SimardError::InvalidConfigValue { .. }));
}

#[test]
fn round_trip_export_import() {
    let source = mock_bridge();
    // Store some data in the source bridge.
    source
        .store_fact("rust", "systems language", 0.9, &[], "ep-1")
        .unwrap();
    source
        .store_procedure("build", &["compile".to_string(), "test".to_string()], &[])
        .unwrap();

    let snapshot = export_memory_snapshot(&source, "agent-1", None).unwrap();
    assert_eq!(snapshot.facts.len(), 1);
    assert_eq!(snapshot.procedures.len(), 1);
    assert_eq!(snapshot.total_items(), 2);

    // Import into a fresh target bridge.
    let target = mock_bridge();
    let count = import_memory_snapshot(&target, &snapshot).unwrap();
    assert_eq!(count, 2);

    // Verify the target has the data.
    let target_snapshot = export_memory_snapshot(&target, "agent-2", None).unwrap();
    assert_eq!(target_snapshot.facts.len(), 1);
    assert_eq!(target_snapshot.procedures.len(), 1);
}

#[test]
fn snapshot_serializes_to_json() {
    let snapshot = MemorySnapshot {
        facts: vec![CognitiveFact {
            node_id: "f1".to_string(),
            concept: "test".to_string(),
            content: "test content".to_string(),
            confidence: 0.8,
            source_id: "".to_string(),
            tags: vec![],
        }],
        procedures: vec![],
        exported_at: 1000,
        source_agent: "agent-x".to_string(),
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    let parsed: MemorySnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.facts.len(), 1);
    assert_eq!(parsed.source_agent, "agent-x");
}

#[test]
fn snapshot_display_is_readable() {
    let snapshot = MemorySnapshot {
        facts: vec![],
        procedures: vec![],
        exported_at: 1000,
        source_agent: "agent-x".to_string(),
    };
    let s = snapshot.to_string();
    assert!(s.contains("facts=0"));
    assert!(s.contains("agent-x"));
}

#[test]
fn export_to_file_and_load() {
    let bridge = mock_bridge();
    bridge
        .store_fact("rust", "fast language", 0.95, &[], "")
        .unwrap();

    let dir = std::env::temp_dir().join("simard-test-snapshot");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("snapshot.json");

    let snapshot = export_memory_snapshot(&bridge, "file-agent", Some(&path)).unwrap();
    assert_eq!(snapshot.facts.len(), 1);

    let loaded = load_snapshot_from_file(&path).unwrap();
    assert_eq!(loaded.facts.len(), 1);
    assert_eq!(loaded.source_agent, "file-agent");

    let _ = std::fs::remove_dir_all(&dir);
}
