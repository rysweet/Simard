//! Durability tests for the cognitive memory backend.
//!
//! These tests pin two regressions that previously bit Simard:
//!
//! 1. `create_verified_backup` only copied `cognitive_memory.ladybug` and
//!    silently ignored the `*.wal` companion file, so backups missed
//!    every write that hadn't been checkpointed yet (which is most of
//!    them — lbug defers checkpointing until the WAL grows past a
//!    threshold).
//! 2. The OODA daemon's shutdown path didn't force a checkpoint before
//!    exit, so writes since the last threshold checkpoint were stuck in
//!    the WAL. Drop-based cleanup also didn't fire because the IPC
//!    server thread held an `Arc` clone forever.
//!
//! The fixes:
//! - `create_verified_backup` asks the live writer (if registered) to
//!   `CHECKPOINT;` first and copies the `*.wal` file alongside the main
//!   DB.
//! - The daemon explicitly calls `bridges.memory.checkpoint()` before
//!   exit and clears the in-process writer registration.
//!
//! The tests below exercise these contracts directly (no subprocess
//! needed): write facts → call `checkpoint()` / `create_verified_backup`
//! → verify the on-disk backup contains the writes when re-opened
//! standalone.

use std::path::PathBuf;
use std::sync::Arc;

use simard::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use simard::memory_ipc;

struct TempState {
    root: PathBuf,
}

impl TempState {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "simard-durability-{}-{}",
            label,
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&root).expect("mkdir state root");
        Self { root }
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        // Always clear the in-process writer registration so leaked
        // state from a panicking test doesn't poison the next one.
        memory_ipc::clear_in_process_writer();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn checkpoint_flushes_wal_so_a_separate_open_sees_writes() {
    let temp = TempState::new("checkpoint");

    // Phase 1: open writer, store facts, force checkpoint, drop writer.
    {
        let writer =
            NativeCognitiveMemory::open(&temp.root).expect("first open as writer succeeds");
        for i in 0..5 {
            writer
                .store_fact(
                    &format!("durability::concept::{i}"),
                    &format!("durability test fact #{i}"),
                    0.95,
                    &["durability".to_string()],
                    "checkpoint-test",
                )
                .expect("store_fact succeeds");
        }
        writer.checkpoint().expect("explicit checkpoint succeeds");
        // Drop the writer here so the file lock is released before the
        // re-open below.
    }

    // Phase 2: re-open and verify all 5 facts are visible.
    let reader = NativeCognitiveMemory::open(&temp.root).expect("reopen after checkpoint succeeds");
    let facts = reader
        .search_facts("durability", 64, 0.0)
        .expect("search_facts after reopen succeeds");
    assert_eq!(
        facts.len(),
        5,
        "all 5 facts must be visible on reopen after checkpoint+drop, got {}: {facts:?}",
        facts.len()
    );
}

#[test]
fn create_verified_backup_includes_wal_so_restored_db_has_uncommitted_writes() {
    let temp = TempState::new("backup");

    // Open writer, write 7 facts, register as in-process writer (so
    // create_verified_backup will checkpoint via this handle), then
    // call create_verified_backup. We do NOT explicitly checkpoint
    // before backup — this test specifically exercises the path where
    // writes are still in the WAL when backup runs.
    let writer = Arc::new(NativeCognitiveMemory::open(&temp.root).expect("open writer succeeds"))
        as Arc<dyn CognitiveMemoryOps>;
    memory_ipc::register_in_process_writer(temp.root.clone(), Arc::clone(&writer));

    for i in 0..7 {
        writer
            .store_fact(
                &format!("backup::concept::{i}"),
                &format!("backup test fact #{i}"),
                0.9,
                &["backup".to_string()],
                "backup-test",
            )
            .expect("store_fact succeeds");
    }

    let backup_path = NativeCognitiveMemory::create_verified_backup(&temp.root)
        .expect("create_verified_backup succeeds");

    assert!(
        backup_path.exists(),
        "backup file must exist at {}",
        backup_path.display()
    );
    let wal_companion = {
        let mut p = backup_path.clone();
        let mut name = p.file_name().unwrap().to_os_string();
        name.push(".wal");
        p.set_file_name(name);
        p
    };
    // Whether the WAL companion exists depends on whether checkpoint
    // emptied it; both are valid, but if it does exist it must be
    // copied alongside.
    if std::fs::metadata(temp.root.join("cognitive_memory.ladybug.wal"))
        .map(|m| m.len() > 0)
        .unwrap_or(false)
    {
        assert!(
            wal_companion.exists(),
            "non-empty source WAL must produce a .wal companion at {}",
            wal_companion.display()
        );
    }

    // Drop the live writer so we can re-open the backup standalone
    // without lock conflicts.
    memory_ipc::clear_in_process_writer();
    drop(writer);

    // Stage the backup as a fresh state-root layout so
    // `NativeCognitiveMemory::open` can pick it up.
    let restored_root = temp.root.join("restored");
    std::fs::create_dir_all(&restored_root).unwrap();
    std::fs::copy(&backup_path, restored_root.join("cognitive_memory.ladybug"))
        .expect("copy main DB into restored state root");
    if wal_companion.exists() {
        std::fs::copy(
            &wal_companion,
            restored_root.join("cognitive_memory.ladybug.wal"),
        )
        .expect("copy wal companion into restored state root");
    }

    let restored = NativeCognitiveMemory::open(&restored_root)
        .expect("backup must be openable as a standalone DB");
    let facts = restored
        .search_facts("backup", 64, 0.0)
        .expect("search_facts on restored backup succeeds");
    assert_eq!(
        facts.len(),
        7,
        "restored backup must contain all 7 facts (got {}): this is the silent-data-loss \
         regression — backup copied only the .ladybug file and missed the WAL. Facts: {facts:?}",
        facts.len()
    );
}

#[test]
fn prune_old_backups_removes_wal_companion() {
    let temp = TempState::new("prune");
    let backup_dir = temp.root.join("backups");
    std::fs::create_dir_all(&backup_dir).unwrap();

    // Synthesize 3 fake backups, two with .wal companions.
    for ts in &[100u64, 200, 300] {
        std::fs::write(
            backup_dir.join(format!("cognitive_memory.ladybug.{ts}")),
            b"db",
        )
        .unwrap();
        std::fs::write(
            backup_dir.join(format!("cognitive_memory.ladybug.{ts}.wal")),
            b"wal",
        )
        .unwrap();
    }

    NativeCognitiveMemory::prune_old_backups(&temp.root, 1);

    // Only ts=300 (newest) and its .wal must remain.
    assert!(backup_dir.join("cognitive_memory.ladybug.300").exists());
    assert!(backup_dir.join("cognitive_memory.ladybug.300.wal").exists());
    assert!(!backup_dir.join("cognitive_memory.ladybug.200").exists());
    assert!(
        !backup_dir.join("cognitive_memory.ladybug.200.wal").exists(),
        "prune must remove the .wal companion of the deleted backup"
    );
    assert!(!backup_dir.join("cognitive_memory.ladybug.100").exists());
    assert!(!backup_dir.join("cognitive_memory.ladybug.100.wal").exists());
}
