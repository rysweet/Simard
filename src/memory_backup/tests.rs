#![allow(deprecated)]

use super::*;
use crate::memory::{FileBackedMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
use crate::memory_client::CognitiveMemoryClient;
use crate::memory_cognitive::{CognitiveFact, CognitiveProcedure};
use crate::rpc_transport::InMemoryRpcTransport;
use crate::session::{SessionId, SessionPhase};
use serde_json::json;
use std::sync::Mutex;
use uuid::Uuid;

struct MockStore {
    facts: Vec<CognitiveFact>,
    procedures: Vec<CognitiveProcedure>,
}

fn mock_memory() -> CognitiveMemoryClient {
    let store: &'static Mutex<MockStore> = Box::leak(Box::new(Mutex::new(MockStore {
        facts: vec![],
        procedures: vec![],
    })));

    let transport = InMemoryRpcTransport::new("test-backup", move |method, params| match method {
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

    let memory = mock_memory();
    memory
        .store_fact("rust", "fast lang", 0.9, &[], "ep1")
        .unwrap();
    memory
        .store_procedure("build", &["compile".into()], &[])
        .unwrap();

    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
    file_store.put(make_record("rec1")).unwrap();
    file_store.put(make_record("rec2")).unwrap();

    let manifest = backup_memory(&memory, &file_store, "test-agent", &config).unwrap();
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

    let memory = mock_memory();
    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();

    let manifest = backup_memory(&memory, &file_store, "agent", &config).unwrap();

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

    let memory = mock_memory();
    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();

    let manifest = backup_memory(&memory, &file_store, "agent", &config).unwrap();

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

    let memory = mock_memory();
    memory.store_fact("rust", "systems", 0.9, &[], "").unwrap();

    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
    file_store.put(make_record("r1")).unwrap();

    let manifest = backup_memory(&memory, &file_store, "agent", &config).unwrap();

    // Restore into fresh targets.
    let target_memory = mock_memory();
    let target_store_path = tmp.path().join("restored.json");
    let target_store = FileBackedMemoryStore::try_new(&target_store_path).unwrap();

    let count = restore_from_backup(&target_memory, &target_store, &manifest.backup_dir).unwrap();
    assert_eq!(count, 2); // 1 fact + 1 record
    assert_eq!(target_store.list_all().unwrap().len(), 1);
}

#[test]
fn restore_rejects_corrupted_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let backup_root = tmp.path().join("backups");
    let store_path = tmp.path().join("memory.json");
    let config = test_config(&backup_root);

    let memory = mock_memory();
    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();

    let manifest = backup_memory(&memory, &file_store, "agent", &config).unwrap();
    fs::write(&manifest.cognitive_snapshot_path, b"bad").unwrap();

    let target_memory = mock_memory();
    let target_store = FileBackedMemoryStore::try_new(tmp.path().join("t.json")).unwrap();

    let err = restore_from_backup(&target_memory, &target_store, &manifest.backup_dir);
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

    let memory = mock_memory();
    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();

    // Create two backups with distinct timestamp directories.
    backup_memory(&memory, &file_store, "a", &config).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    backup_memory(&memory, &file_store, "a", &config).unwrap();

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

    let memory = mock_memory();
    memory
        .store_fact("algorithms", "sorting and searching", 0.85, &[], "ep1")
        .unwrap();
    memory
        .store_fact("databases", "relational storage", 0.9, &[], "ep2")
        .unwrap();
    memory
        .store_procedure(
            "deploy",
            &["build".into(), "test".into(), "ship".into()],
            &[],
        )
        .unwrap();

    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
    let manifest = backup_memory(&memory, &file_store, "test-agent", &config).unwrap();
    assert_eq!(manifest.cognitive_facts_count, 2);
    assert_eq!(manifest.cognitive_procedures_count, 1);

    // Restore into a fresh memory and verify searchability.
    let target_memory = mock_memory();
    let target_store_path = tmp.path().join("restored.json");
    let target_store = FileBackedMemoryStore::try_new(&target_store_path).unwrap();

    let count = restore_from_backup(&target_memory, &target_store, &manifest.backup_dir).unwrap();
    assert!(count >= 3, "should restore at least 2 facts + 1 procedure");

    // Verify facts are searchable.
    let facts = target_memory.search_facts("algorithms", 10, 0.0).unwrap();
    assert!(!facts.is_empty(), "restored facts should be searchable");

    // Verify procedures are recallable.
    let procs = target_memory.recall_procedure("deploy", 5).unwrap();
    assert!(
        !procs.is_empty(),
        "restored procedures should be recallable"
    );
    assert_eq!(procs[0].steps, vec!["build", "test", "ship"]);
}

// ---------------------------------------------------------------------------
// Issue #2420 — verified backup of the LIVE store
//
// A verified backup must be re-opened and its memory count confirmed BEFORE any
// prune; a mismatch must fail loudly rather than be silently trusted. (The
// "backup source == live store path the daemon opens" guarantee is pinned in
// `cognitive_memory::tests_live_store_path_2420`.)
// ---------------------------------------------------------------------------

/// Helper: build a backup with a known memory count (1 fact + 1 procedure + 2
/// file-backed records = 4 total) and return its directory.
fn backup_with_known_counts(tmp: &Path) -> (PathBuf, usize) {
    let backup_root = tmp.join("backups");
    let store_path = tmp.join("memory.json");
    let config = test_config(&backup_root);

    let memory = mock_memory();
    memory.store_fact("rust", "fast", 0.9, &[], "ep1").unwrap();
    memory
        .store_procedure("build", &["compile".into()], &[])
        .unwrap();

    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
    file_store.put(make_record("rec1")).unwrap();
    file_store.put(make_record("rec2")).unwrap();

    let manifest = backup_memory(&memory, &file_store, "agent", &config).unwrap();
    // 1 fact + 1 procedure + 2 records.
    (manifest.backup_dir, 4)
}

/// The count gate accepts a backup whose re-opened total matches the expected
/// live-store count.
#[test]
fn verify_backup_count_passes_on_exact_total() {
    let tmp = tempfile::tempdir().unwrap();
    let (backup_dir, total) = backup_with_known_counts(tmp.path());

    verify_backup_memory_count(&backup_dir, total).expect("matching memory count must verify Ok");
}

/// The count gate fails loudly when the backup holds a different number of
/// memories than expected (truncated / partial backup).
#[test]
fn verify_backup_count_fails_loudly_on_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let (backup_dir, total) = backup_with_known_counts(tmp.path());

    let err = verify_backup_memory_count(&backup_dir, total + 100);
    assert!(
        err.is_err(),
        "a count mismatch must be an Err, not silently trusted"
    );
}

/// `backup_memory_verified` returns a manifest only after the backup re-opens
/// cleanly with matching counts; `verify_backup` on the result is `Valid`.
#[test]
fn backup_memory_verified_returns_valid_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let backup_root = tmp.path().join("backups");
    let store_path = tmp.path().join("memory.json");
    let config = test_config(&backup_root);

    let memory = mock_memory();
    memory.store_fact("rust", "fast", 0.9, &[], "ep1").unwrap();
    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
    file_store.put(make_record("rec1")).unwrap();

    let manifest =
        backup_memory_verified(&memory, &file_store, "agent", &config).expect("verified backup");
    assert_eq!(manifest.cognitive_facts_count, 1);
    assert_eq!(manifest.memory_records_count, 1);

    let v = verify_backup(&manifest.backup_dir).unwrap();
    assert!(
        matches!(v.status, BackupStatus::Valid),
        "verified backup must be Valid on disk"
    );
}

/// A verified backup round-trips: restoring into fresh targets yields the same
/// total memory count the backup verified.
#[test]
fn backup_memory_verified_round_trips_count() {
    let tmp = tempfile::tempdir().unwrap();
    let backup_root = tmp.path().join("backups");
    let store_path = tmp.path().join("memory.json");
    let config = test_config(&backup_root);

    let memory = mock_memory();
    memory.store_fact("rust", "fast", 0.9, &[], "ep1").unwrap();
    memory
        .store_procedure("build", &["compile".into()], &[])
        .unwrap();
    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
    file_store.put(make_record("rec1")).unwrap();
    file_store.put(make_record("rec2")).unwrap();

    let manifest =
        backup_memory_verified(&memory, &file_store, "agent", &config).expect("verified backup");

    let target_memory = mock_memory();
    let target_store = FileBackedMemoryStore::try_new(tmp.path().join("restored.json")).unwrap();
    let restored =
        restore_from_backup(&target_memory, &target_store, &manifest.backup_dir).unwrap();

    // 1 fact + 1 procedure + 2 records.
    assert_eq!(
        restored, 4,
        "restore must round-trip the verified memory count"
    );
}

/// Issue #2420 acceptance: a verified backup of a store **larger than the legacy
/// export cap** (1000 facts) must round-trip the full memory count. This is the
/// exact regression behind the broken backups — the live store grew past the
/// fixed export cap, so a backup silently captured only the first 1000
/// memories. The mock memory ignores `limit`, so this must use the real library
/// backend (`in_memory` for speed; durability is irrelevant to the cap fix).
#[test]
fn verified_backup_round_trips_store_larger_than_export_cap() {
    use crate::cognitive_memory::LibraryCognitiveMemory;

    let tmp = tempfile::tempdir().unwrap();
    let src = LibraryCognitiveMemory::in_memory().unwrap();

    // > legacy MAX_EXPORT_FACTS (1000): a capped export would lose the tail.
    let n_facts = 1025usize;
    for i in 0..n_facts {
        src.store_fact(
            &format!("concept-{i}"),
            &format!("content {i}"),
            0.9,
            &[],
            &format!("ep{i}"),
        )
        .unwrap();
    }

    let file_store = FileBackedMemoryStore::try_new(tmp.path().join("memory.json")).unwrap();
    file_store.put(make_record("rec1")).unwrap();
    file_store.put(make_record("rec2")).unwrap();

    let config = test_config(&tmp.path().join("backups"));
    let manifest =
        backup_memory_verified(&src, &file_store, "agent", &config).expect("verified backup");

    assert_eq!(
        manifest.cognitive_facts_count, n_facts,
        "verified backup must capture every fact, not a capped subset"
    );
    assert_eq!(manifest.memory_records_count, 2);

    // Restore into FRESH targets; the total count must round-trip exactly.
    let dst = LibraryCognitiveMemory::in_memory().unwrap();
    let dst_store = FileBackedMemoryStore::try_new(tmp.path().join("restored.json")).unwrap();
    let restored = restore_from_backup(&dst, &dst_store, &manifest.backup_dir).unwrap();

    let expected = manifest.cognitive_facts_count
        + manifest.cognitive_procedures_count
        + manifest.memory_records_count;
    assert_eq!(
        restored, expected,
        "restore must round-trip the full >cap memory count"
    );
    assert!(
        restored > 1000,
        "round-trip count must exceed the legacy export cap"
    );
}

// ---------------------------------------------------------------------------
// Security: verify/restore must read data files from `backup_dir`, never from
// the absolute paths embedded in the (untrusted) manifest. A tampered manifest
// must not be able to redirect reads to files outside the backup directory, and
// a relocated backup must still verify/restore.
// ---------------------------------------------------------------------------

/// A backup directory that has been relocated (its `manifest.json` still stores
/// the original, now-stale absolute paths) must still verify `Valid` and
/// restore, because verify/restore read the files from `backup_dir` — not from
/// the manifest's stored paths. Code that trusted the manifest paths would
/// report the snapshot/records as missing here.
#[test]
fn verify_and_restore_use_backup_dir_not_manifest_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let backup_root = tmp.path().join("backups");
    let store_path = tmp.path().join("memory.json");
    let config = test_config(&backup_root);

    let memory = mock_memory();
    memory.store_fact("rust", "fast", 0.9, &[], "ep1").unwrap();
    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
    file_store.put(make_record("r1")).unwrap();
    let manifest = backup_memory(&memory, &file_store, "agent", &config).unwrap();

    // Relocate the backup to a new directory, then destroy the original so the
    // manifest's stored absolute paths dangle.
    let moved = tmp.path().join("relocated_backup");
    fs::create_dir_all(&moved).unwrap();
    for f in [MANIFEST_FILE, SNAPSHOT_FILE, RECORDS_FILE] {
        fs::copy(manifest.backup_dir.join(f), moved.join(f)).unwrap();
    }
    fs::remove_dir_all(&manifest.backup_dir).unwrap();

    let v = verify_backup(&moved).unwrap();
    assert!(
        matches!(v.status, BackupStatus::Valid),
        "relocated backup must verify Valid by reading files from backup_dir, got {:?}",
        v.status
    );

    let target_memory = mock_memory();
    let target_store = FileBackedMemoryStore::try_new(tmp.path().join("restored.json")).unwrap();
    let count = restore_from_backup(&target_memory, &target_store, &moved).unwrap();
    assert_eq!(
        count, 2,
        "restore must read the relocated files (1 fact + 1 record)"
    );
}

/// A manifest crafted to point at data files OUTSIDE the backup directory (with
/// a self-consistent checksum/counts over those external files) must not cause
/// restore to ingest the external data. This is the path-traversal / arbitrary
/// file-read hardening: the attacker's injected fact must never reach the store.
#[test]
fn restore_does_not_read_manifest_paths_outside_backup_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let backup_root = tmp.path().join("backups");
    let store_path = tmp.path().join("memory.json");
    let config = test_config(&backup_root);

    // Legitimate backup with a single benign fact.
    let memory = mock_memory();
    memory.store_fact("benign", "ok", 0.9, &[], "ep1").unwrap();
    let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
    let manifest = backup_memory(&memory, &file_store, "agent", &config).unwrap();

    // Attacker writes data files OUTSIDE the backup dir and crafts a manifest
    // pointing at them, with a self-consistent checksum and counts.
    let evil_dir = tmp.path().join("evil");
    fs::create_dir_all(&evil_dir).unwrap();
    let evil_snapshot_path = evil_dir.join("snapshot.json");
    let evil_records_path = evil_dir.join("records.json");

    let evil_memory = mock_memory();
    evil_memory
        .store_fact("attacker-injected", "pwn", 0.9, &[], "x")
        .unwrap();
    let evil_snapshot =
        crate::remote_transfer::export_full_memory_snapshot(&evil_memory, "agent").unwrap();
    let evil_snapshot_bytes = serde_json::to_vec_pretty(&evil_snapshot).unwrap();
    fs::write(&evil_snapshot_path, &evil_snapshot_bytes).unwrap();
    let evil_records: Vec<MemoryRecord> = Vec::new();
    let evil_records_bytes = serde_json::to_vec_pretty(&evil_records).unwrap();
    fs::write(&evil_records_path, &evil_records_bytes).unwrap();

    let mut tampered = manifest.clone();
    tampered.cognitive_snapshot_path = evil_snapshot_path;
    tampered.memory_records_path = evil_records_path;
    tampered.checksum = sha256_hex(&[&evil_snapshot_bytes, &evil_records_bytes]);
    tampered.cognitive_facts_count = evil_snapshot.facts.len();
    tampered.cognitive_procedures_count = evil_snapshot.procedures.len();
    tampered.memory_records_count = evil_records.len();
    fs::write(
        manifest.backup_dir.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&tampered).unwrap(),
    )
    .unwrap();

    // Restore from the (legit) backup dir; the crafted external paths must be
    // ignored. Whether the result is Ok or a verification Err, the attacker's
    // fact must never be ingested.
    let target_memory = mock_memory();
    let target_store = FileBackedMemoryStore::try_new(tmp.path().join("restored.json")).unwrap();
    let _ = restore_from_backup(&target_memory, &target_store, &manifest.backup_dir);

    let facts = target_memory
        .search_facts("attacker-injected", 10, 0.0)
        .unwrap();
    assert!(
        facts.iter().all(|f| f.concept != "attacker-injected"),
        "restore must not ingest data from manifest paths outside the backup directory"
    );
}
