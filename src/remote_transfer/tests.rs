use super::*;
use crate::memory_client::CognitiveMemoryClient;
use crate::rpc_transport::InMemoryRpcTransport;
use serde_json::json;
use std::sync::Mutex;

struct MockStore {
    facts: Vec<CognitiveFact>,
    procedures: Vec<CognitiveProcedure>,
}

fn mock_memory() -> CognitiveMemoryClient {
    let store: &'static Mutex<MockStore> = Box::leak(Box::new(Mutex::new(MockStore {
        facts: vec![],
        procedures: vec![],
    })));

    let transport = InMemoryRpcTransport::new("test-memory", move |method, params| match method {
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
        _ => Err(crate::rpc::RpcErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    CognitiveMemoryClient::new(Box::new(transport))
}

#[test]
fn export_empty_memory_returns_empty_snapshot() {
    let memory = mock_memory();
    let snapshot = export_memory_snapshot(&memory, "test-agent", None).unwrap();
    assert!(snapshot.is_empty());
    assert_eq!(snapshot.total_items(), 0);
    assert_eq!(snapshot.source_agent, "test-agent");
    assert!(snapshot.exported_at > 0);
}

#[test]
fn export_rejects_empty_agent_name() {
    let memory = mock_memory();
    let err = export_memory_snapshot(&memory, "", None).unwrap_err();
    assert!(matches!(err, SimardError::InvalidConfigValue { .. }));
}

#[test]
fn round_trip_export_import() {
    let source = mock_memory();
    // Store some data in the source memory.
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

    // Import into a fresh target memory.
    let target = mock_memory();
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
    let memory = mock_memory();
    memory
        .store_fact("rust", "fast language", 0.95, &[], "")
        .unwrap();

    let dir = std::env::temp_dir().join("simard-test-snapshot");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("snapshot.json");

    let snapshot = export_memory_snapshot(&memory, "file-agent", Some(&path)).unwrap();
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
/// cap (the failure that broke verified backups from Jun 20). The mock memory
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

/// Issue #2562: a prospective trigger that was already `resolved` when the
/// snapshot was taken must restore as `resolved` — NOT `pending` — so an
/// auto-restore / `simard memory import` can never re-fire a goal the daemon
/// already completed via `check_triggers`. Pins the RESTORE side of the
/// status-preservation contract documented on `import_full_snapshot`.
#[test]
fn restored_resolved_prospective_stays_resolved_so_completed_goal_does_not_refire() {
    use crate::cognitive_memory::LibraryCognitiveMemory;

    // Source store: create a prospective, then resolve it (a completed goal).
    let source = LibraryCognitiveMemory::in_memory().unwrap();
    let node_id = source
        .store_prospective("ship the CLI", "deploy", "Pursue goal", 1)
        .unwrap();
    source.resolve_prospective(&node_id).unwrap();

    // The snapshot must capture the terminal status verbatim (export side).
    let snapshot = export_full_memory_snapshot(&source, "issue-2562").unwrap();
    assert_eq!(snapshot.prospective.len(), 1);
    assert_eq!(
        snapshot.prospective[0].status, "resolved",
        "export must capture the resolved status the source store holds"
    );

    // Restore into a fresh store.
    let target = LibraryCognitiveMemory::in_memory().unwrap();
    import_full_snapshot(&target, &snapshot).unwrap();

    // The restored record must keep its resolved status (equals the snapshot),
    // never regressing to pending.
    let restored = target.list_all_prospective(u32::MAX).unwrap();
    assert_eq!(restored.len(), 1, "the prospective must be restored");
    assert_eq!(
        restored[0].status, "resolved",
        "restored status must equal the snapshotted status, not reset to pending"
    );

    // The behavioural guarantee: a completed trigger must not re-fire.
    let fired = target
        .check_triggers("please deploy the release now")
        .unwrap();
    assert!(
        fired.is_empty(),
        "a restored resolved trigger must not re-fire after restore (issue #2562)"
    );
}

/// Issue #2562: a prospective captured as `triggered` (it fired but was not yet
/// resolved) must not come back `pending` and re-fire on restore. It restores to
/// a terminal, non-firing status so an auto-restore cannot resurrect an
/// already-fired trigger.
#[test]
fn restored_triggered_prospective_does_not_come_back_pending_and_refire() {
    use crate::cognitive_memory::LibraryCognitiveMemory;

    let source = LibraryCognitiveMemory::in_memory().unwrap();
    source
        .store_prospective("ship the CLI", "deploy", "Pursue goal", 1)
        .unwrap();
    // Fire it: check_triggers moves the matched record to "triggered".
    let fired = source.check_triggers("time to deploy").unwrap();
    assert_eq!(fired.len(), 1, "the trigger must fire on the source store");

    let snapshot = export_full_memory_snapshot(&source, "issue-2562").unwrap();
    assert_eq!(snapshot.prospective.len(), 1);
    assert_eq!(
        snapshot.prospective[0].status, "triggered",
        "export must capture the triggered status"
    );

    let target = LibraryCognitiveMemory::in_memory().unwrap();
    import_full_snapshot(&target, &snapshot).unwrap();

    let restored = target.list_all_prospective(u32::MAX).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(
        restored[0].status, "resolved",
        "a triggered trigger restores to the terminal, non-firing resolved \
         status (the library has no arbitrary status setter) — never pending \
         (issue #2562)"
    );
    let refired = target.check_triggers("time to deploy again").unwrap();
    assert!(
        refired.is_empty(),
        "a restored already-fired trigger must not re-fire (issue #2562)"
    );
}

/// Issue #2562 (guard): a genuinely `pending` prospective must still restore as
/// `pending` and stay eligible to fire — the status-preservation fix must not
/// blanket-resolve live triggers.
#[test]
fn restored_pending_prospective_stays_pending_and_can_still_fire() {
    use crate::cognitive_memory::LibraryCognitiveMemory;

    let source = LibraryCognitiveMemory::in_memory().unwrap();
    source
        .store_prospective("ship the CLI", "deploy", "Pursue goal", 1)
        .unwrap();

    let snapshot = export_full_memory_snapshot(&source, "issue-2562").unwrap();
    assert_eq!(snapshot.prospective.len(), 1);
    assert_eq!(
        snapshot.prospective[0].status, "pending",
        "a freshly stored trigger is pending"
    );

    let target = LibraryCognitiveMemory::in_memory().unwrap();
    import_full_snapshot(&target, &snapshot).unwrap();

    let restored = target.list_all_prospective(u32::MAX).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(
        restored[0].status, "pending",
        "a pending trigger must restore as pending"
    );

    let fired = target.check_triggers("please deploy now").unwrap();
    assert_eq!(
        fired.len(),
        1,
        "a restored pending trigger must still be able to fire"
    );
}

/// Issue #2562 (idempotent self-heal): re-importing a snapshot must correct a
/// store that a pre-fix restore left with a stale `pending` copy of an
/// already-handled trigger. When the target already holds the same
/// `(trigger_condition, description)` as `pending` but the snapshot marks it
/// terminal, the import resolves the existing record so it can no longer fire.
#[test]
fn reimport_resolves_a_preexisting_stale_pending_duplicate() {
    use crate::cognitive_memory::LibraryCognitiveMemory;

    // Snapshot captured a resolved (already-handled) trigger.
    let source = LibraryCognitiveMemory::in_memory().unwrap();
    let src_node = source
        .store_prospective("ship the CLI", "deploy", "Pursue goal", 1)
        .unwrap();
    source.resolve_prospective(&src_node).unwrap();
    let snapshot = export_full_memory_snapshot(&source, "issue-2562").unwrap();
    assert_eq!(snapshot.prospective[0].status, "resolved");

    // Target already holds the SAME trigger as a stale `pending` record — the
    // exact residue a pre-#2562 restore would have left behind.
    let target = LibraryCognitiveMemory::in_memory().unwrap();
    target
        .store_prospective("ship the CLI", "deploy", "Pursue goal", 1)
        .unwrap();
    let before = target.list_all_prospective(u32::MAX).unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(
        before[0].status, "pending",
        "precondition: stale pending record"
    );

    // Re-import: dedup skips a fresh insert (0 new items) but must still resolve
    // the pre-existing stale record.
    let imported = import_full_snapshot(&target, &snapshot).unwrap();
    assert_eq!(imported, 0, "the duplicate must not be inserted again");

    let after = target.list_all_prospective(u32::MAX).unwrap();
    assert_eq!(after.len(), 1, "no duplicate row is created");
    assert_eq!(
        after[0].status, "resolved",
        "the stale pending duplicate must be resolved so it can no longer fire"
    );
    let fired = target.check_triggers("please deploy now").unwrap();
    assert!(
        fired.is_empty(),
        "the self-healed trigger must not fire after re-import (issue #2562)"
    );
}

/// Issue #2562 (guard against over-correction): a live `pending` trigger whose
/// `(trigger_condition, description)` also appears as `pending` in the snapshot
/// must stay `pending` and remain able to fire — the self-heal only resolves
/// records the snapshot marks terminal.
#[test]
fn reimport_leaves_a_matching_pending_trigger_firable() {
    use crate::cognitive_memory::LibraryCognitiveMemory;

    let source = LibraryCognitiveMemory::in_memory().unwrap();
    source
        .store_prospective("ship the CLI", "deploy", "Pursue goal", 1)
        .unwrap();
    let snapshot = export_full_memory_snapshot(&source, "issue-2562").unwrap();
    assert_eq!(snapshot.prospective[0].status, "pending");

    let target = LibraryCognitiveMemory::in_memory().unwrap();
    target
        .store_prospective("ship the CLI", "deploy", "Pursue goal", 1)
        .unwrap();

    let imported = import_full_snapshot(&target, &snapshot).unwrap();
    assert_eq!(imported, 0, "the duplicate must not be inserted again");

    let after = target.list_all_prospective(u32::MAX).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].status, "pending",
        "a pending trigger matching a pending snapshot record stays pending"
    );
    let fired = target.check_triggers("please deploy now").unwrap();
    assert_eq!(
        fired.len(),
        1,
        "the still-pending trigger must remain firable"
    );
}
