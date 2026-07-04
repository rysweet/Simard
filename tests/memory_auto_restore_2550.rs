//! Issue #2550 (P0), Fix #3: on daemon startup, if the live store is
//! empty/near-empty AND a newer non-empty `cognitive_snapshot.json` backup
//! exists, auto-restore from the newest good snapshot so a corruption-reset
//! self-heals instead of losing everything.
//!
//! These tests pin the pure `auto_restore_if_empty` seam the daemon calls at
//! startup. No sleeps, no network: `TempDir`-rooted backups + store.

use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use simard::memory_backup::{BackupConfig, auto_restore_if_empty};
use simard::memory_cognitive::CognitiveFact;
use simard::remote_transfer::MemorySnapshot;

fn config_for(backup_dir: &std::path::Path) -> BackupConfig {
    BackupConfig {
        backup_dir: backup_dir.to_path_buf(),
        retention_days: 30,
        min_backups_to_keep: 3,
    }
}

/// Write `backup_dir/<ts>/cognitive_snapshot.json` with `facts` facts, matching
/// the periodic backup layout (`~/.simard/backups/<ts>/cognitive_snapshot.json`).
fn write_backup_snapshot(backup_dir: &std::path::Path, ts: &str, facts: usize) {
    let dir = backup_dir.join(ts);
    std::fs::create_dir_all(&dir).unwrap();
    let snapshot = MemorySnapshot {
        facts: (0..facts)
            .map(|i| CognitiveFact {
                node_id: format!("f{i}"),
                concept: format!("concept-{i}"),
                content: format!("restored fact {i}"),
                confidence: 0.9,
                source_id: "issue-2550".to_string(),
                tags: vec![],
                usage_count: 0,
                last_accessed_at: None,
            })
            .collect(),
        procedures: vec![],
        exported_at: 1_751_500_000,
        source_agent: "issue-2550".to_string(),
    };
    std::fs::write(
        dir.join("cognitive_snapshot.json"),
        serde_json::to_string_pretty(&snapshot).unwrap(),
    )
    .unwrap();
}

#[test]
fn auto_restore_fires_when_store_empty_and_newer_snapshot_exists() {
    let backups = tempfile::tempdir().unwrap();
    write_backup_snapshot(backups.path(), "20260703_120000", 2); // older
    write_backup_snapshot(backups.path(), "20260704_001600", 3); // newest, non-empty

    let store = tempfile::tempdir().unwrap();
    let mem = LibraryCognitiveMemory::open(store.path()).expect("open empty store");
    assert_eq!(
        mem.get_statistics().unwrap().total(),
        0,
        "precondition: the store starts empty"
    );

    let report = auto_restore_if_empty(&mem, &config_for(backups.path()))
        .expect("auto_restore_if_empty must not error")
        .expect("auto-restore MUST fire on an empty store with a newer non-empty snapshot");

    assert_eq!(
        report.restored, 3,
        "must restore from the NEWEST snapshot (3 facts), not the older one"
    );
    assert_eq!(
        mem.get_statistics().unwrap().semantic_count,
        3,
        "restored facts must actually be in the store"
    );
}

#[test]
fn auto_restore_is_a_noop_when_store_is_populated() {
    let backups = tempfile::tempdir().unwrap();
    write_backup_snapshot(backups.path(), "20260704_001600", 5);

    let store = tempfile::tempdir().unwrap();
    let mem = LibraryCognitiveMemory::open(store.path()).expect("open store");
    mem.store_fact("live", "already here", 0.9, &[], "live")
        .expect("store_fact");

    let outcome = auto_restore_if_empty(&mem, &config_for(backups.path())).expect("must not error");
    assert!(
        outcome.is_none(),
        "a populated store must not be auto-restored (would clobber/duplicate live data)"
    );
    assert_eq!(
        mem.get_statistics().unwrap().semantic_count,
        1,
        "the live store must be left untouched"
    );
}

#[test]
fn auto_restore_is_a_noop_when_no_snapshot_exists() {
    let backups = tempfile::tempdir().unwrap(); // empty: no backups at all
    let store = tempfile::tempdir().unwrap();
    let mem = LibraryCognitiveMemory::open(store.path()).expect("open empty store");

    let outcome = auto_restore_if_empty(&mem, &config_for(backups.path())).expect("must not error");
    assert!(
        outcome.is_none(),
        "no snapshot means there is nothing to restore"
    );
    assert_eq!(mem.get_statistics().unwrap().total(), 0);
}
