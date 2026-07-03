//! Issue #2420 end-to-end (outside-in) acceptance check.
//!
//! Drives the SAME flow the OODA daemon runs in production — open the live
//! cognitive store at `state_root/cognitive`, take a *verified* backup, verify
//! its memory count, then restore into a fresh store — but against a REAL
//! persistent lbug store (not an in-memory adapter). Proves the issue's
//! acceptance: "a fresh verified backup of the LIVE store can be produced and a
//! restore round-trips the current memory count."

use simard::cognitive_memory::{
    CognitiveMemoryOps, LIVE_STORE_SUBDIR, LibraryCognitiveMemory, live_store_path,
};
use simard::memory::FileBackedMemoryStore;
use simard::memory_backup::{
    BackupConfig, backup_memory_verified, restore_from_backup, verify_backup_memory_count,
};

#[test]
fn fresh_verified_backup_of_live_store_round_trips_count() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path();

    // 1. Open the LIVE store exactly as the daemon does at boot. This creates
    //    `state_root/cognitive` — the post-migration live store path.
    let live = LibraryCognitiveMemory::open(state_root).expect("open live store");

    // ROOT-CAUSE ASSERTION: the verified-backup source path == the live store
    // path the daemon actually opened (the bug was these two diverging, so
    // backups silently copied the stale legacy single-file path).
    let expected_live = state_root.join(LIVE_STORE_SUBDIR);
    assert!(
        expected_live.exists(),
        "daemon open must create the live `cognitive` store directory"
    );
    assert_eq!(
        live_store_path(state_root),
        expected_live,
        "verified-backup source must equal the live store the daemon opens"
    );

    // 2. Populate the live store with a realistic batch (> the legacy 1000-fact
    //    export cap, the exact size class that broke the backups).
    let n_facts = 1200usize;
    for i in 0..n_facts {
        live.store_fact(
            &format!("concept-{i}"),
            &format!("content {i}"),
            0.9,
            &[],
            &format!("ep{i}"),
        )
        .unwrap();
    }
    live.store_procedure("build", &["compile".into(), "link".into()], &[])
        .unwrap();
    let n_procs = 1usize;

    // File-backed records live next to the store (the daemon backs these up too).
    let records_path = state_root.join("memory_records.json");
    let file_store = FileBackedMemoryStore::try_new(&records_path).unwrap();
    let n_records = 0usize;

    let expected_total = n_facts + n_procs + n_records;

    // 3. Produce a VERIFIED backup under `state_root/backups` (daemon behavior).
    let config = BackupConfig {
        backup_dir: state_root.join("backups"),
        ..BackupConfig::default()
    };
    let manifest = backup_memory_verified(&live, &file_store, "simard", &config)
        .expect("verified backup of the live store must succeed");

    assert!(
        manifest.backup_dir.starts_with(state_root.join("backups")),
        "backup must land under state_root/backups"
    );
    assert_eq!(
        manifest.cognitive_facts_count, n_facts,
        "verified backup must capture every live fact (uncapped), not a 1000 subset"
    );
    assert_eq!(manifest.cognitive_procedures_count, n_procs);

    // 4. Fail-loud count verification BEFORE any prune (issue #2420 gate).
    verify_backup_memory_count(&manifest.backup_dir, expected_total)
        .expect("verified backup must hold the live store's exact memory count");

    // 5. Restore into a FRESH live store and assert the count round-trips.
    let other_root = state_root.join("restore-root");
    std::fs::create_dir_all(&other_root).unwrap();
    let restored_live = LibraryCognitiveMemory::open(&other_root).expect("open restore store");
    let restored_store =
        FileBackedMemoryStore::try_new(other_root.join("memory_records.json")).unwrap();
    let restored = restore_from_backup(&restored_live, &restored_store, &manifest.backup_dir)
        .expect("restore from the verified backup must succeed");

    assert_eq!(
        restored, expected_total,
        "restore must round-trip the live store's current memory count"
    );
    assert!(
        restored > 1000,
        "round-trip count must exceed the legacy export cap (got {restored})"
    );
}
