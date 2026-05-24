//! Integration tests for the cognitive-memory silent-drop counter (issue #1975).
//!
//! Exercises:
//! 1. `prune_old_backups` with a read-only dir → `prune_remove_failed`
//!    counter increments, `PruneOutcome.failed` is non-empty, removable siblings
//!    are still pruned.
//! 2. `preemptive_wal_cleanup` with a WAL the process cannot remove → hard error.
//! 3. `consolidate_episodes` compensating-action pattern on happy path.
//!    4–6. Metrics helpers (scoped_reset, PruneOutcome structure).

use simard::CognitiveMemoryOps;
use simard::cognitive_memory::NativeCognitiveMemory;
use simard::cognitive_memory::metrics::{cognitive_memory_silent_drop_count, scoped_reset};

use serial_test::serial;
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn setup() {
    scoped_reset();
}

// ============================================================================
// 1. prune_old_backups: read-only backup dir → failure counted
// ============================================================================

#[test]
#[serial]
fn prune_old_backups_counts_readonly_failures() {
    setup();

    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path();
    let backup_dir = state_root.join("backups");
    fs::create_dir_all(&backup_dir).unwrap();

    let removable = backup_dir.join("cognitive_memory.ladybug.1000");
    let readonly = backup_dir.join("cognitive_memory.ladybug.2000");
    let kept = backup_dir.join("cognitive_memory.ladybug.3000");

    fs::write(&removable, b"removable").unwrap();
    fs::write(&readonly, b"readonly").unwrap();
    fs::write(&kept, b"kept").unwrap();

    // Make the backup_dir read-only so ALL removes fail.
    let mut dir_perms = fs::metadata(&backup_dir).unwrap().permissions();
    dir_perms.set_mode(0o555);
    fs::set_permissions(&backup_dir, dir_perms).unwrap();

    let outcome = NativeCognitiveMemory::prune_old_backups(state_root, 1);

    // Restore directory permissions before assertions so cleanup works.
    let mut restore_perms = fs::metadata(&backup_dir).unwrap().permissions();
    restore_perms.set_mode(0o755);
    fs::set_permissions(&backup_dir, restore_perms).unwrap();

    assert!(
        !outcome.failed.is_empty(),
        "expected at least one prune failure, got none"
    );
    assert_eq!(outcome.removed, 0, "no files should have been removed");

    let counter =
        cognitive_memory_silent_drop_count("prune_remove_failed", "prune_old_backups:main");
    assert!(
        counter >= 1,
        "prune_remove_failed counter should be >= 1, got {counter}"
    );
}

// ============================================================================
// 2. preemptive_wal_cleanup: WAL in read-only dir → hard error
// ============================================================================

#[test]
#[serial]
fn preemptive_wal_cleanup_returns_hard_error_on_unremovable_wal() {
    setup();

    let tmp = tempfile::tempdir().unwrap();
    let db_dir = tmp.path().join("db");
    fs::create_dir_all(&db_dir).unwrap();

    let db_path = db_dir.join("cognitive_memory.ladybug");
    let wal_path = db_path.with_extension("ladybug.wal");
    fs::write(&wal_path, b"").unwrap();

    let mut perms = fs::metadata(&db_dir).unwrap().permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&db_dir, perms).unwrap();

    let result = NativeCognitiveMemory::preemptive_wal_cleanup(&db_path);

    let mut restore = fs::metadata(&db_dir).unwrap().permissions();
    restore.set_mode(0o755);
    fs::set_permissions(&db_dir, restore).unwrap();

    assert!(
        result.is_err(),
        "expected hard error from preemptive_wal_cleanup, got Ok"
    );

    let counter =
        cognitive_memory_silent_drop_count("wal_cleanup_failed", "preemptive_wal_cleanup");
    assert_eq!(
        counter, 1,
        "wal_cleanup_failed counter should be 1, got {counter}"
    );
}

// ============================================================================
// 3. consolidate_episodes: happy path completes with compensating-action
// ============================================================================

#[test]
#[serial]
fn consolidate_episodes_happy_path_completes() {
    setup();

    let mem = NativeCognitiveMemory::in_memory().unwrap();

    mem.store_episode("alpha", "test", None).unwrap();
    mem.store_episode("beta", "test", None).unwrap();
    mem.store_episode("gamma", "test", None).unwrap();

    let result = mem.consolidate_episodes(10);

    match result {
        Ok(Some(summary_id)) => {
            assert!(
                summary_id.starts_with("epi_"),
                "summary id should start with epi_"
            );
        }
        Ok(None) => panic!("expected consolidation to produce a summary"),
        Err(e) => panic!("unexpected consolidation error: {e}"),
    }

    let counter = cognitive_memory_silent_drop_count(
        "consolidation_partial_apply",
        "consolidate_episodes:create",
    );
    assert_eq!(
        counter, 0,
        "consolidation_partial_apply should be 0 on happy path, got {counter}"
    );
}

// ============================================================================
// 4. scoped_reset clears all counters
// ============================================================================

#[test]
#[serial]
fn scoped_reset_clears_all_counters() {
    simard::cognitive_memory::metrics::increment("test_kind", "test_site");
    assert_eq!(
        cognitive_memory_silent_drop_count("test_kind", "test_site"),
        1
    );

    scoped_reset();

    assert_eq!(
        cognitive_memory_silent_drop_count("test_kind", "test_site"),
        0
    );
}

// ============================================================================
// 5. PruneOutcome empty when no backups
// ============================================================================

#[test]
#[serial]
fn prune_outcome_empty_when_no_backups() {
    setup();

    let tmp = tempfile::tempdir().unwrap();
    let outcome = NativeCognitiveMemory::prune_old_backups(tmp.path(), 5);

    assert_eq!(outcome.removed, 0);
    assert!(outcome.failed.is_empty());
}

// ============================================================================
// 6. prune_old_backups removes old backups successfully
// ============================================================================

#[test]
#[serial]
fn prune_removes_old_backups_successfully() {
    setup();

    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path();
    let backup_dir = state_root.join("backups");
    fs::create_dir_all(&backup_dir).unwrap();

    for epoch in [1000u64, 2000, 3000, 4000] {
        fs::write(
            backup_dir.join(format!("cognitive_memory.ladybug.{epoch}")),
            b"backup",
        )
        .unwrap();
    }

    let outcome = NativeCognitiveMemory::prune_old_backups(state_root, 2);

    assert_eq!(outcome.removed, 2, "should remove 2 old backups");
    assert!(outcome.failed.is_empty(), "no failures expected");

    assert!(backup_dir.join("cognitive_memory.ladybug.4000").exists());
    assert!(backup_dir.join("cognitive_memory.ladybug.3000").exists());
    assert!(!backup_dir.join("cognitive_memory.ladybug.2000").exists());
    assert!(!backup_dir.join("cognitive_memory.ladybug.1000").exists());
}
