//! Issue #4687 (P0): a cognitive-memory store closed **cleanly** MUST replay
//! its WAL with **zero silent loss** on the next open — no checksum failure, no
//! tail truncation, no "recovered from corrupt WAL (good prefix)" dropping the
//! most recent writes.
//!
//! # The incident this pins the fix for
//!
//! `journalctl --user -u simard-ooda` showed, on EVERY daemon start:
//!
//!   * `lbug_store: WAL replay failed on open; attempting recovery … error=
//!     Storage exception: Checksum verification failed, the WAL file is corrupted`
//!   * immediately followed by `recovered from corrupt WAL (good prefix)` —
//!     i.e. the WAL tail is silently truncated, dropping the most recent
//!     cognitive-memory writes;
//!   * and `auto-checkpoint failed … Error renaming file …/cognitive.wal to
//!     …/cognitive.wal.checkpoint. No such file or directory`, so the
//!     checkpoint never advanced and the WAL kept re-corrupting.
//!
//! The upstream fix (single-owner checkpointing, fsync-before-advance
//! ordering, existence-guarded rename, clean-provenance replay-failure → hard
//! error instead of silent tail-drop) restores the contract these tests pin.
//!
//! # Relationship to #2550
//!
//! `wal_recovery_durability_2550.rs` pins the **crash-provenance** path: a
//! salvaged good-prefix must survive a later reopen (never reset to empty).
//! This file pins the complementary **clean-provenance** path: after a clean
//! close there is NOTHING to salvage — replay must simply succeed in full. The
//! two contracts are kept distinct on purpose (ambiguity A3): #2550's
//! salvage-into-fresh-DB path stays intact; #4687 forbids the silent tail-drop
//! on the clean path.
//!
//! These are contract/regression guards at the Simard adapter boundary
//! (`LibraryCognitiveMemory` → `CognitiveMemory::open_persistent`). They run
//! against `TempDir`-rooted stores — no sleeps, no network. They are **RED**
//! against the buggy pinned rev `c266e15d…` (which drops the most recent writes
//! on a clean reopen) and turn **GREEN** once the #4687 fix is adopted via
//! `tests/issue_4687_amplihack_pin_bump.rs`.

use serial_test::serial;
use simard::cognitive_memory::metrics;
use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};

/// The live store lives at `state_root/cognitive` (a file) with its write-ahead
/// log at the sibling `state_root/cognitive.wal`. Pinned here so the test breaks
/// loudly if the on-disk layout ever changes.
const DB_FILE: &str = "cognitive";
const WAL_FILE: &str = "cognitive.wal";

/// LadybugDB's default auto-checkpoint interval. Seeding MORE than this many
/// writes guarantees the WAL churns through at least one checkpoint boundary,
/// exercising the rename/fsync ordering the incident corrupted.
const CHECKPOINT_INTERVAL: usize = 128;

fn seed_facts(mem: &LibraryCognitiveMemory, start: usize, n: usize) {
    for i in start..start + n {
        mem.store_fact(
            &format!("concept-{i}"),
            &format!("durable content {i}"),
            0.9,
            &[],
            "issue-4687",
        )
        .expect("store_fact");
    }
}

fn total(mem: &LibraryCognitiveMemory) -> u64 {
    mem.get_statistics().expect("get_statistics").total()
}

/// The core #4687 invariant: a store seeded past the checkpoint boundary,
/// checkpointed, and closed **cleanly** (via `Drop`, no `mem::forget`) must
/// reopen with EVERY record intact — no checksum failure, no silent tail-drop.
#[test]
fn clean_shutdown_reopen_preserves_all_records() {
    let root = tempfile::tempdir().unwrap();
    let seeded = CHECKPOINT_INTERVAL + 64; // cross ≥1 auto-checkpoint boundary

    // Round 1: seed past the checkpoint interval, checkpoint, clean close.
    {
        let mem = LibraryCognitiveMemory::open(root.path()).expect("open live store");
        seed_facts(&mem, 0, seeded);
        mem.checkpoint().expect("checkpoint");
        // No mem::forget — `mem` drops here, running the clean-shutdown path.
    }

    // A clean close must leave a fully replayable WAL/DB. If the on-disk WAL
    // still exists it must NOT be corrupt; the reopen below must not emit a
    // checksum failure nor silently truncate the tail.
    let mem = LibraryCognitiveMemory::open(root.path())
        .expect("clean reopen must succeed without WAL corruption");
    let c = total(&mem);
    assert_eq!(
        c, seeded as u64,
        "SILENT WAL LOSS REGRESSION (#4687): a cleanly-closed store dropped \
         records on reopen (seeded {seeded}, reopened with {c}). The clean-\
         shutdown WAL must replay with zero loss — no 'good prefix' truncation."
    );
}

/// The most-recent writes — the ones the incident silently dropped — must
/// survive a clean reopen. Seeds a second batch AFTER a checkpoint so those
/// writes live in the WAL tail, then relies on the clean `Drop` to persist
/// them durably. Also asserts the adapter records ZERO silent-drop events for
/// the clean path (no silent discard of committed writes, ambiguity A3/A4).
#[test]
#[serial]
fn recent_writes_survive_clean_reopen_with_no_silent_drop() {
    metrics::scoped_reset();

    let root = tempfile::tempdir().unwrap();
    let first = 100usize; // checkpointed batch
    let recent = 40usize; // most-recent batch, lives in the WAL tail

    {
        let mem = LibraryCognitiveMemory::open(root.path()).expect("open live store");
        seed_facts(&mem, 0, first);
        mem.checkpoint().expect("checkpoint first batch");
        // The MOST RECENT writes — exactly what the incident dropped.
        seed_facts(&mem, first, recent);
        // Clean close: `Drop` must durably persist the WAL tail (fsync-before-
        // advance) so the recent batch is replayable, never silently truncated.
    }

    let mem = LibraryCognitiveMemory::open(root.path())
        .expect("clean reopen must succeed without WAL corruption");
    let c = total(&mem);
    assert_eq!(
        c,
        (first + recent) as u64,
        "MOST-RECENT WRITES LOST (#4687): clean reopen dropped the WAL-tail \
         batch (expected {} = {first}+{recent}, got {c}). Recovery must never \
         silently discard committed writes.",
        first + recent
    );

    // The clean path must not have quietly discarded anything: no silent-drop
    // counter may have fired for a WAL/cognitive-memory site during this open.
    let dropped = metrics::cognitive_memory_silent_drop_count("wal", "clean_reopen")
        + metrics::cognitive_memory_silent_drop_count("wal_replay", "open");
    assert_eq!(
        dropped, 0,
        "clean-shutdown reopen recorded {dropped} silent-drop event(s); a clean \
         WAL replay must lose nothing and surface any real loss as an EXPLICIT \
         error, never a silent counter-only drop."
    );

    metrics::scoped_reset();
}

/// Plain durability across repeated CLEAN reopens: explicitly checkpointed
/// records survive every clean close/open with no drift and no silent reset.
#[test]
fn checkpointed_records_survive_repeated_clean_reopens() {
    let root = tempfile::tempdir().unwrap();

    {
        let mem = LibraryCognitiveMemory::open(root.path()).expect("open");
        seed_facts(&mem, 0, 50);
        mem.checkpoint().expect("checkpoint");
    }

    let after_second;
    {
        let mem = LibraryCognitiveMemory::open(root.path()).expect("reopen");
        assert_eq!(
            total(&mem),
            50,
            "all checkpointed facts must survive the first clean reopen"
        );
        seed_facts(&mem, 50, 25);
        mem.checkpoint().expect("checkpoint");
        after_second = total(&mem);
        assert_eq!(after_second, 75);
    }

    {
        let mem = LibraryCognitiveMemory::open(root.path()).expect("reopen 2");
        assert_eq!(
            total(&mem),
            after_second,
            "records must be stable across a third clean reopen, never reset"
        );
    }
}

/// Belt-and-suspenders: after a clean close the on-disk store must be present
/// and, when a WAL sidecar remains, it must be non-empty/parseable rather than
/// a zero-length or dangling file that the next open would treat as corrupt.
/// This pins the "clean close leaves a valid, fully-replayable WAL" half of the
/// fix (design C3) at the file layer, independent of record counts.
#[test]
fn clean_close_leaves_a_valid_store_on_disk() {
    let root = tempfile::tempdir().unwrap();
    {
        let mem = LibraryCognitiveMemory::open(root.path()).expect("open");
        seed_facts(&mem, 0, 200); // > checkpoint interval
        mem.checkpoint().expect("checkpoint");
    }

    let db = root.path().join(DB_FILE);
    assert!(
        db.exists(),
        "clean close must leave the store DB file at {}",
        db.display()
    );

    // If a WAL sidecar survives a clean close, it must not be a zero-length
    // stub — a truncated/empty WAL is exactly what triggers the incident's
    // "Checksum verification failed" on the next open.
    let wal = root.path().join(WAL_FILE);
    if wal.exists() {
        let len = std::fs::metadata(&wal).unwrap().len();
        assert!(
            len > 0,
            "a surviving WAL after clean close must not be zero-length \
             (dangling/truncated WAL corrupts the next replay): {}",
            wal.display()
        );
    }

    // And it must reopen cleanly with the full record set — the ultimate proof
    // the on-disk state is self-consistent after a clean shutdown.
    let mem = LibraryCognitiveMemory::open(root.path()).expect("clean reopen");
    assert_eq!(
        total(&mem),
        200,
        "clean-closed store must reopen with all 200 records intact"
    );
}
