//! End-to-end resilience test for the issue #2420 cognitive-memory backup path.
//!
//! Exercises the REAL library backend (lbug 0.17.1) through the full public
//! backup pipeline against a temporary, isolated state root:
//!
//! 1. A live store with **more than** the replication cap (`MAX_EXPORT_FACTS`)
//!    facts is captured in full (no silent truncation).
//! 2. The fresh backup verifies clean and restores with identical counts.
//! 3. Corrupt/shadow quarantine artifacts are bounded to the newest N while the
//!    live store file is never touched.
//!
//! This is the committed companion to the qa-team scenario
//! `tests/gadugi/memory-backup-resilience.yaml`.

use std::fs;

use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use simard::memory::{FileBackedMemoryStore, MemoryStore};
use simard::memory_backup::{
    BackupConfig, BackupStatus, CORRUPT_ARTIFACTS_KEEP, backup_memory, prune_corrupt_artifacts,
    restore_from_backup, verify_backup,
};

// Comfortably above the replication cap (MAX_EXPORT_FACTS = 1000) so the test
// proves the backup captures the full store rather than the truncated subset.
const FACTS: usize = 1050;
const PROCS: usize = 12;

#[test]
fn live_store_backup_round_trips_and_bounds_corrupt_artifacts() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path();

    // --- seed a real live store (state_root/cognitive) -------------------
    let live = LibraryCognitiveMemory::open(state_root).unwrap();
    for i in 0..FACTS {
        live.store_fact(
            &format!("concept-{i}"),
            "content",
            0.9,
            &[],
            &format!("src-{i}"),
        )
        .unwrap();
    }
    for i in 0..PROCS {
        live.store_procedure(&format!("proc-{i}"), &["step".into()], &[])
            .unwrap();
    }
    live.checkpoint().unwrap();
    assert!(
        state_root.join("cognitive").exists(),
        "live store file must exist after checkpoint"
    );

    // --- seed corrupt/shadow quarantine artifacts -----------------------
    let artifact_names = [
        "cognitive.corrupt-1000",
        "cognitive.corrupt-1001",
        "cognitive.corrupt-1002",
        "cognitive.shadow",
        "cognitive.wal.corrupt-1.cognitive.corrupt-2.cognitive.shadow",
        "cognitive_memory.corrupt-9",
        "cognitive.corrupt-1003",
        "cognitive.corrupt-1004",
    ];
    for name in artifact_names {
        fs::write(state_root.join(name), b"junk").unwrap();
    }

    // --- back up the LIVE store -----------------------------------------
    let config = BackupConfig {
        backup_dir: state_root.join("backups"),
        retention_days: 30,
        min_backups_to_keep: 3,
    };
    let store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json")).unwrap();
    let manifest = backup_memory(&live, &store, "qa-2420", &config).unwrap();

    // The FULL store is captured — not truncated at MAX_EXPORT_FACTS (=1000).
    assert_eq!(
        manifest.cognitive_facts_count, FACTS,
        "backup must capture every fact (no replication-cap truncation)"
    );
    assert_eq!(manifest.cognitive_procedures_count, PROCS);

    let verification = verify_backup(&manifest.backup_dir).unwrap();
    assert!(
        matches!(verification.status, BackupStatus::Valid),
        "fresh backup must verify clean, got {:?}",
        verification.status
    );

    // --- bound the corrupt/shadow artifacts -----------------------------
    let pruned = prune_corrupt_artifacts(state_root, CORRUPT_ARTIFACTS_KEEP).unwrap();
    assert_eq!(
        pruned,
        artifact_names.len() - CORRUPT_ARTIFACTS_KEEP,
        "must prune all but the newest CORRUPT_ARTIFACTS_KEEP"
    );
    let remaining = fs::read_dir(state_root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.contains(".corrupt-") || n.ends_with(".shadow")
        })
        .count();
    assert_eq!(remaining, CORRUPT_ARTIFACTS_KEEP);
    // The live store must survive the prune untouched.
    assert!(
        state_root.join("cognitive").exists(),
        "prune must never remove the live store"
    );

    // --- restore round-trips the live counts ----------------------------
    let target = LibraryCognitiveMemory::in_memory().unwrap();
    let target_store = FileBackedMemoryStore::try_new(state_root.join("restored.json")).unwrap();
    restore_from_backup(&target, &target_store, &manifest.backup_dir).unwrap();
    assert_eq!(
        target.search_facts("*", u32::MAX, 0.0).unwrap().len(),
        FACTS
    );
    assert_eq!(target.recall_procedure("*", u32::MAX).unwrap().len(), PROCS);

    // The fresh backup still opens cleanly after pruning.
    assert!(matches!(
        verify_backup(&manifest.backup_dir).unwrap().status,
        BackupStatus::Valid
    ));

    let _ = store.list_all();
}
