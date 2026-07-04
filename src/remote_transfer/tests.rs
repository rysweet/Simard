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
                    usage_count: 0,
                    last_accessed_at: None,
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
            "memory.search_episodes_by_keywords" => Ok(json!({"episodes": []})),
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
            usage_count: 0,
            last_accessed_at: None,
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

/// Issue #2420: the verified-backup export must capture the **entire** store,
/// not the capped replication subset. With more facts than the legacy
/// `MAX_EXPORT_FACTS` cap, [`export_full_memory_snapshot`] returns all of them
/// while the capped [`export_memory_snapshot`] truncates — proving the backup
/// can no longer silently drop the tail as the live store grows past a fixed
/// cap (the failure that broke verified backups from Jun 20). The mock bridge
/// ignores `limit`, so this exercises the real library backend where the cap is
/// actually enforced.
#[test]
fn full_export_captures_more_than_capped_export() {
    use crate::cognitive_memory::LibraryCognitiveMemory;

    let mem = LibraryCognitiveMemory::in_memory().unwrap();
    let n = MAX_EXPORT_FACTS as usize + 25;
    for i in 0..n {
        mem.store_fact(
            &format!("concept-{i}"),
            &format!("content {i}"),
            0.9,
            &[],
            &format!("ep{i}"),
        )
        .unwrap();
    }

    let full = export_full_memory_snapshot(&mem, "agent").unwrap();
    let capped = export_memory_snapshot(&mem, "agent", None).unwrap();

    assert_eq!(full.facts.len(), n, "full export must return every fact");
    assert_eq!(
        capped.facts.len(),
        MAX_EXPORT_FACTS as usize,
        "capped export truncates at the legacy cap"
    );
    assert!(
        full.facts.len() > capped.facts.len(),
        "full export must exceed the legacy cap"
    );
}

/// Issue #2550: a full snapshot must round-trip **every** durable memory type
/// through export → import, and re-importing must be idempotent (dedup by
/// content). Complements the export-completeness test in
/// `tests/memory_snapshot_completeness_2550.rs` by pinning the RESTORE side.
#[test]
fn full_snapshot_round_trips_all_types_and_import_is_idempotent() {
    use crate::cognitive_memory::LibraryCognitiveMemory;

    let source = LibraryCognitiveMemory::in_memory().unwrap();
    source
        .store_fact("rust", "Rust is a systems language", 0.9, &[], "issue-2550")
        .unwrap();
    source
        .store_procedure("ooda:consolidate", &["distill episodes".to_string()], &[])
        .unwrap();
    source
        .store_episode(
            "episode: ran cargo test; 0 failures",
            "engineer-cycle",
            None,
        )
        .unwrap();
    source
        .store_prospective("ship the CLI", "trigger:ship-cli", "Pursue goal", 1)
        .unwrap();

    let snapshot = export_full_memory_snapshot(&source, "issue-2550").unwrap();
    assert_eq!(snapshot.facts.len(), 1);
    assert_eq!(snapshot.procedures.len(), 1);
    assert_eq!(snapshot.episodes.len(), 1);
    assert_eq!(snapshot.prospective.len(), 1);

    // Import into a fresh store: every type must land.
    let target = LibraryCognitiveMemory::in_memory().unwrap();
    let first = import_full_snapshot(&target, &snapshot).unwrap();
    assert_eq!(first, 4, "all four durable memories must be imported");

    let stats = target.get_statistics().unwrap();
    assert_eq!(stats.semantic_count, 1, "fact restored");
    assert_eq!(stats.procedural_count, 1, "procedure restored");
    assert_eq!(stats.episodic_count, 1, "episode restored");
    assert_eq!(stats.prospective_count, 1, "prospective restored");

    // Re-importing the same snapshot must dedup by content — nothing new.
    let second = import_full_snapshot(&target, &snapshot).unwrap();
    assert_eq!(
        second, 0,
        "re-import must deduplicate every item by content"
    );
    let after = target.get_statistics().unwrap();
    assert_eq!(after.total(), 4, "counts must be stable across re-import");
}

/// Issue #2550: `load_full_snapshot_from_file` must transparently accept every
/// on-disk shape Simard has written — a full snapshot, a legacy bare
/// `MemorySnapshot`, and a `PersistedEnvelope`-wrapped snapshot — and surface a
/// clear error on a corrupt file. This pins the loader's behavior across the
/// fast (direct-deserialize) path and the envelope-unwrap fallback so the
/// resource-usage optimization on the P0 recovery path cannot regress any shape.
#[test]
fn load_full_snapshot_from_file_reads_full_bare_and_enveloped_shapes() {
    use crate::cognitive_memory::LibraryCognitiveMemory;

    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();

    // Build a full snapshot (all four durable types) from a live store.
    let source = LibraryCognitiveMemory::in_memory().unwrap();
    source
        .store_fact("rust", "Rust is a systems language", 0.9, &[], "issue-2550")
        .unwrap();
    source
        .store_procedure("ooda:consolidate", &["distill episodes".to_string()], &[])
        .unwrap();
    source
        .store_episode(
            "episode: ran cargo test; 0 failures",
            "engineer-cycle",
            None,
        )
        .unwrap();
    source
        .store_prospective("ship the CLI", "trigger:ship-cli", "Pursue goal", 1)
        .unwrap();
    let full = export_full_memory_snapshot(&source, "issue-2550").unwrap();

    // (1) Full snapshot: the fast path must round-trip every durable type.
    let full_path = dir.join("full.json");
    std::fs::write(&full_path, serde_json::to_vec(&full).unwrap()).unwrap();
    let loaded_full = load_full_snapshot_from_file(&full_path).unwrap();
    assert_eq!(
        loaded_full.total_items(),
        4,
        "full snapshot must load all four durable types"
    );
    assert_eq!(
        loaded_full.episodes.len(),
        1,
        "episodes must survive the load"
    );
    assert_eq!(
        loaded_full.prospective.len(),
        1,
        "prospective triggers must survive the load"
    );

    // (2) Legacy bare snapshot (facts + procedures only, no episodes/prospective
    // keys): the fast path must still load it, defaulting the missing types.
    let bare = MemorySnapshot {
        facts: full.facts.clone(),
        procedures: full.procedures.clone(),
        exported_at: full.exported_at,
        source_agent: full.source_agent.clone(),
    };
    let bare_path = dir.join("bare.json");
    std::fs::write(&bare_path, serde_json::to_vec(&bare).unwrap()).unwrap();
    let loaded_bare = load_full_snapshot_from_file(&bare_path).unwrap();
    assert_eq!(loaded_bare.facts.len(), 1);
    assert_eq!(loaded_bare.procedures.len(), 1);
    assert!(
        loaded_bare.episodes.is_empty(),
        "a legacy bare snapshot carries no episodes"
    );
    assert!(
        loaded_bare.prospective.is_empty(),
        "a legacy bare snapshot carries no prospective triggers"
    );

    // (3) PersistedEnvelope-wrapped snapshot (session-boundary file): the
    // envelope-unwrap fallback must recover the payload.
    let envelope = PersistedEnvelope {
        schema_version: ENVELOPE_SCHEMA_VERSION,
        payload: bare.clone(),
    };
    let env_path = dir.join("enveloped.json");
    std::fs::write(&env_path, serde_json::to_vec(&envelope).unwrap()).unwrap();
    let loaded_env = load_full_snapshot_from_file(&env_path).unwrap();
    assert_eq!(
        loaded_env.facts.len(),
        1,
        "the enveloped payload's facts must load via the unwrap fallback"
    );
    assert_eq!(
        loaded_env.procedures.len(),
        1,
        "the enveloped payload's procedures must load via the unwrap fallback"
    );

    // (4) A corrupt file must surface an error, not a silent empty snapshot.
    let bad_path = dir.join("corrupt.json");
    std::fs::write(&bad_path, b"{ this is not valid json").unwrap();
    assert!(
        load_full_snapshot_from_file(&bad_path).is_err(),
        "a corrupt snapshot file must error rather than yield an empty snapshot"
    );
}
