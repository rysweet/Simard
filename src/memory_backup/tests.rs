#![allow(deprecated)]

use super::*;
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::memory::{FileBackedMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::memory_cognitive::{CognitiveFact, CognitiveProcedure};
use crate::session::{SessionId, SessionPhase};
use serde_json::json;
use std::sync::Mutex;
use uuid::Uuid;

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
        InMemoryBridgeTransport::new("test-backup", move |method, params| match method {
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

fn test_session_id() -> SessionId {
    SessionId::from_uuid(Uuid::nil())
}

fn make_record(key: &str) -> MemoryRecord {
    MemoryRecord {
        key: key.to_string(),
        scope: MemoryScope::Project,
        value: format!("val-{key}"),
        session_id: test_session_id(),
        recorded_in: SessionPhase::Execution,
        created_at: None,
    }
}

fn test_config(dir: &Path) -> BackupConfig {
    BackupConfig {
        backup_dir: dir.to_path_buf(),
        retention_days: 30,
        min_backups_to_keep: 2,
    }
}

#[test]
fn backup_and_verify_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let backup_root = tmp.path().join("backups");
    let store_path = tmp.path().join("memory.json");
    let config = test_config(&backup_root);

    let bridge = mock_bridge();
    bridge
        .store_fact("rust", "fast lang", 0.9, &[], "ep1")
        .unwrap();
    bridge
        .store_procedure("build", &["compile".into()], &[])
        .unwrap();

    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
    file_store.put(make_record("rec1")).unwrap();
    file_store.put(make_record("rec2")).unwrap();

    let manifest = backup_memory(&bridge, &file_store, "test-agent", &config).unwrap();
    assert_eq!(manifest.cognitive_facts_count, 1);
    assert_eq!(manifest.cognitive_procedures_count, 1);
    assert_eq!(manifest.memory_records_count, 2);
    assert!(!manifest.checksum.is_empty());

    let verification = verify_backup(&manifest.backup_dir).unwrap();
    assert!(matches!(verification.status, BackupStatus::Valid));
}

#[test]
fn verify_detects_missing_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("empty_backup");
    fs::create_dir_all(&dir).unwrap();

    let v = verify_backup(&dir).unwrap();
    assert!(matches!(v.status, BackupStatus::Incomplete { .. }));
}

#[test]
fn verify_detects_corrupted_checksum() {
    let tmp = tempfile::tempdir().unwrap();
    let backup_root = tmp.path().join("backups");
    let store_path = tmp.path().join("memory.json");
    let config = test_config(&backup_root);

    let bridge = mock_bridge();
    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();

    let manifest = backup_memory(&bridge, &file_store, "agent", &config).unwrap();

    // Tamper with the snapshot file.
    fs::write(&manifest.cognitive_snapshot_path, b"tampered").unwrap();

    let v = verify_backup(&manifest.backup_dir).unwrap();
    assert!(matches!(v.status, BackupStatus::Corrupted { .. }));
}

#[test]
fn verify_detects_missing_files() {
    let tmp = tempfile::tempdir().unwrap();
    let backup_root = tmp.path().join("backups");
    let store_path = tmp.path().join("memory.json");
    let config = test_config(&backup_root);

    let bridge = mock_bridge();
    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();

    let manifest = backup_memory(&bridge, &file_store, "agent", &config).unwrap();

    // Remove one backup file.
    fs::remove_file(&manifest.memory_records_path).unwrap();

    let v = verify_backup(&manifest.backup_dir).unwrap();
    assert!(matches!(v.status, BackupStatus::Incomplete { .. }));
}

#[test]
fn restore_from_valid_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let backup_root = tmp.path().join("backups");
    let store_path = tmp.path().join("memory.json");
    let config = test_config(&backup_root);

    let bridge = mock_bridge();
    bridge.store_fact("rust", "systems", 0.9, &[], "").unwrap();

    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
    file_store.put(make_record("r1")).unwrap();

    let manifest = backup_memory(&bridge, &file_store, "agent", &config).unwrap();

    // Restore into fresh targets.
    let target_bridge = mock_bridge();
    let target_store_path = tmp.path().join("restored.json");
    let target_store = FileBackedMemoryStore::try_new(&target_store_path).unwrap();

    let count = restore_from_backup(&target_bridge, &target_store, &manifest.backup_dir).unwrap();
    assert_eq!(count, 2); // 1 fact + 1 record
    assert_eq!(target_store.list_all().unwrap().len(), 1);
}

#[test]
fn restore_rejects_corrupted_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let backup_root = tmp.path().join("backups");
    let store_path = tmp.path().join("memory.json");
    let config = test_config(&backup_root);

    let bridge = mock_bridge();
    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();

    let manifest = backup_memory(&bridge, &file_store, "agent", &config).unwrap();
    fs::write(&manifest.cognitive_snapshot_path, b"bad").unwrap();

    let target_bridge = mock_bridge();
    let target_store = FileBackedMemoryStore::try_new(tmp.path().join("t.json")).unwrap();

    let err = restore_from_backup(&target_bridge, &target_store, &manifest.backup_dir);
    assert!(err.is_err());
}

#[test]
fn list_backups_empty_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(&tmp.path().join("no-such-dir"));
    let backups = list_backups(&config).unwrap();
    assert!(backups.is_empty());
}

#[test]
fn prune_old_backups_respects_min_keep() {
    let tmp = tempfile::tempdir().unwrap();
    let backup_root = tmp.path().join("backups");
    let store_path = tmp.path().join("memory.json");
    let mut config = test_config(&backup_root);
    config.retention_days = 0;
    config.min_backups_to_keep = 1;

    let bridge = mock_bridge();
    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();

    // Create two backups with distinct timestamp directories.
    backup_memory(&bridge, &file_store, "a", &config).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    backup_memory(&bridge, &file_store, "a", &config).unwrap();

    let before = list_backups(&config).unwrap().len();
    assert_eq!(before, 2);

    let pruned = prune_old_backups(&config).unwrap();
    assert_eq!(pruned, 1);

    let after = list_backups(&config).unwrap().len();
    assert_eq!(after, 1);
}

#[test]
fn prune_nonexistent_dir_returns_zero() {
    let config = test_config(Path::new("/nonexistent/path"));
    assert_eq!(prune_old_backups(&config).unwrap(), 0);
}

#[test]
fn backup_config_default_points_to_home() {
    let config = BackupConfig::default();
    assert!(config.backup_dir.to_string_lossy().contains(".simard"));
    assert!(config.backup_dir.to_string_lossy().contains("backups"));
    assert_eq!(config.retention_days, 30);
    assert_eq!(config.min_backups_to_keep, 3);
}

#[test]
fn backup_restore_round_trip_searchable() {
    let tmp = tempfile::tempdir().unwrap();
    let backup_root = tmp.path().join("backups");
    let store_path = tmp.path().join("memory.json");
    let config = test_config(&backup_root);

    let bridge = mock_bridge();
    bridge
        .store_fact("algorithms", "sorting and searching", 0.85, &[], "ep1")
        .unwrap();
    bridge
        .store_fact("databases", "relational storage", 0.9, &[], "ep2")
        .unwrap();
    bridge
        .store_procedure(
            "deploy",
            &["build".into(), "test".into(), "ship".into()],
            &[],
        )
        .unwrap();

    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
    let manifest = backup_memory(&bridge, &file_store, "test-agent", &config).unwrap();
    assert_eq!(manifest.cognitive_facts_count, 2);
    assert_eq!(manifest.cognitive_procedures_count, 1);

    // Restore into a fresh bridge and verify searchability.
    let target_bridge = mock_bridge();
    let target_store_path = tmp.path().join("restored.json");
    let target_store = FileBackedMemoryStore::try_new(&target_store_path).unwrap();

    let count = restore_from_backup(&target_bridge, &target_store, &manifest.backup_dir).unwrap();
    assert!(count >= 3, "should restore at least 2 facts + 1 procedure");

    // Verify facts are searchable.
    let facts = target_bridge.search_facts("algorithms", 10, 0.0).unwrap();
    assert!(!facts.is_empty(), "restored facts should be searchable");

    // Verify procedures are recallable.
    let procs = target_bridge.recall_procedure("deploy", 5).unwrap();
    assert!(
        !procs.is_empty(),
        "restored procedures should be recallable"
    );
    assert_eq!(procs[0].steps, vec!["build", "test", "ship"]);
}

// ---------------------------------------------------------------------------
// issue #2420: corrupt-artifact bounding + scheduled live-store backup
// ---------------------------------------------------------------------------

fn touch_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
}

#[test]
fn is_corrupt_artifact_matches_quarantine_names() {
    // Migrated lbug store quarantine names (carry the `.corrupt-` marker).
    assert!(super::is_corrupt_artifact(
        "cognitive.corrupt-1782585794318985440"
    ));
    assert!(super::is_corrupt_artifact(
        "cognitive_memory.corrupt-1781915091"
    ));
    // Concatenated rename chain (issue #2420 gap #2) still carries the marker.
    assert!(super::is_corrupt_artifact(
        "cognitive.wal.corrupt-1782585794318721438.cognitive.wal.corrupt-1782444638849049543.cognitive.shadow"
    ));
    // A *bare* shadow file (no `.corrupt-` marker) is NOT a prune target: it
    // could be an active shadow-paging file of the live store.
    assert!(!super::is_corrupt_artifact("cognitive.shadow"));
    assert!(!super::is_corrupt_artifact("snapshot.shadow"));
}

#[test]
fn is_corrupt_artifact_never_matches_live_store() {
    // The live store file and its active sidecars must always be protected.
    assert!(!super::is_corrupt_artifact("cognitive"));
    assert!(!super::is_corrupt_artifact("cognitive.wal"));
    assert!(!super::is_corrupt_artifact("cognitive.shm"));
    assert!(!super::is_corrupt_artifact("cognitive.shadow"));
    // Unrelated files are left alone.
    assert!(!super::is_corrupt_artifact("memory_records.json"));
    assert!(!super::is_corrupt_artifact("config.toml"));
    assert!(!super::is_corrupt_artifact("cognitive_memory.ladybug"));
}

#[test]
fn prune_corrupt_artifacts_keeps_newest_n_and_protects_live_store() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // The live store, an active sidecar, and a bare shadow file — all survive.
    touch_file(&root.join("cognitive"), b"LIVE-STORE");
    touch_file(&root.join("cognitive.wal"), b"LIVE-WAL");
    touch_file(&root.join("cognitive.shadow"), b"ACTIVE-SHADOW");
    // An unrelated file — must survive.
    touch_file(&root.join("memory_records.json"), b"[]");

    // Eight corrupt artifacts created oldest-first; small sleeps give strictly
    // increasing mtimes (same approach as `prune_old_backups_respects_min_keep`).
    for i in 0..8 {
        touch_file(&root.join(format!("cognitive.corrupt-{i:020}")), b"junk");
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    // One quarantined directory (historic native-store shape), newest of all.
    let dir = root.join("cognitive.corrupt-99999999999999999999");
    fs::create_dir_all(&dir).unwrap();
    touch_file(&dir.join("inner"), b"x");

    // 9 corrupt artifacts total, keep 5 -> prune 4.
    let pruned = prune_corrupt_artifacts(root, 5).unwrap();
    assert_eq!(pruned, 4);

    // Live store + sidecar + bare shadow + unrelated file untouched.
    assert!(root.join("cognitive").exists());
    assert!(root.join("cognitive.wal").exists());
    assert!(
        root.join("cognitive.shadow").exists(),
        "bare shadow must survive"
    );
    assert!(root.join("memory_records.json").exists());

    // Exactly 5 corrupt artifacts remain, and they are the newest ones.
    let remaining: Vec<String> = fs::read_dir(root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| super::is_corrupt_artifact(n))
        .collect();
    assert_eq!(remaining.len(), 5);
    // The newest (the directory) must be retained.
    assert!(
        remaining
            .iter()
            .any(|n| n == "cognitive.corrupt-99999999999999999999")
    );
    // The oldest (corrupt-0) must be gone.
    assert!(
        !remaining
            .iter()
            .any(|n| n.ends_with("00000000000000000000"))
    );
}

#[test]
fn prune_corrupt_artifacts_noop_when_under_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    touch_file(&root.join("cognitive.corrupt-1"), b"a");
    touch_file(&root.join("cognitive.corrupt-2"), b"b");
    // 2 corrupt artifacts, keep 5 -> prune nothing.
    assert_eq!(prune_corrupt_artifacts(root, 5).unwrap(), 0);
    assert!(root.join("cognitive.corrupt-1").exists());
    assert!(root.join("cognitive.corrupt-2").exists());
}

#[test]
fn prune_corrupt_artifacts_missing_dir_returns_zero() {
    assert_eq!(
        prune_corrupt_artifacts(Path::new("/no/such/state/root"), 5).unwrap(),
        0
    );
}

#[test]
fn run_scheduled_backup_round_trips_live_store_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path();
    let backup_root = state_root.join("backups");
    let config = test_config(&backup_root);

    // A "live" store fronted by the bridge — 2 facts + 1 procedure.
    let bridge = mock_bridge();
    bridge
        .store_fact("rust", "systems lang", 0.9, &[], "ep1")
        .unwrap();
    bridge
        .store_fact("ladybug", "graph store", 0.8, &[], "ep2")
        .unwrap();
    bridge
        .store_procedure("backup", &["snapshot".into(), "verify".into()], &[])
        .unwrap();

    // File-backed records co-located under state_root, plus a stale corrupt
    // artifact that the pass should bound away once 5 are exceeded.
    let store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json")).unwrap();
    store.put(make_record("rec1")).unwrap();
    for i in 0..7 {
        touch_file(
            &state_root.join(format!("cognitive.corrupt-{i:020}")),
            b"junk",
        );
    }

    let outcome =
        run_scheduled_backup_with(&bridge, &store, state_root, "test-agent", &config).unwrap();

    assert!(outcome.is_valid(), "fresh backup must verify clean");
    assert_eq!(outcome.manifest.cognitive_facts_count, 2);
    assert_eq!(outcome.manifest.cognitive_procedures_count, 1);
    assert_eq!(outcome.manifest.memory_records_count, 1);
    // 7 corrupt artifacts, keep 5 -> 2 pruned.
    assert_eq!(outcome.corrupt_artifacts_pruned, 2);

    // The verified backup round-trips into a fresh target with identical counts.
    let target_bridge = mock_bridge();
    let target_store = FileBackedMemoryStore::try_new(state_root.join("restored.json")).unwrap();
    let restored =
        restore_from_backup(&target_bridge, &target_store, &outcome.manifest.backup_dir).unwrap();
    assert_eq!(restored, 4); // 2 facts + 1 procedure + 1 record
    assert_eq!(
        target_bridge.recall_procedure("backup", 5).unwrap().len(),
        1
    );
}
