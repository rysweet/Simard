//! Issue #2550 (P0): WAL prefix-recovery must be **durable across a re-open** —
//! a store that just recovered records must NEVER be reset to empty on a later
//! open.
//!
//! Incident (verified 2026-07-04): the live cognitive store's WAL corrupted;
//! resilient recovery salvaged 40,488 records ("good prefix replayed +
//! checkpointed") but the checkpoint-after-recovery FAILED, and a LATER open
//! re-flagged the store corrupt and RESET it to empty — dropping ~20,480
//! memories to 128, with no restore path.
//!
//! These tests pin the durability contract at the Simard adapter boundary
//! (`LibraryCognitiveMemory`, which routes through
//! `CognitiveMemory::open_persistent` -> `LbugGraphStore::open_with_recovery`).
//! No sleeps, no network: everything runs against `TempDir`-rooted stores and
//! on-disk WAL surgery.

use std::path::Path;

use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};

/// The live store lives at `state_root/cognitive` (a file) with its write-ahead
/// log at the sibling `state_root/cognitive.wal` (LadybugDB appends `.wal` to
/// the full DB filename). Verified empirically; pinned here so the test breaks
/// loudly if the on-disk layout ever changes.
const DB_FILE: &str = "cognitive";
const WAL_FILE: &str = "cognitive.wal";

fn seed_facts(mem: &LibraryCognitiveMemory, start: usize, n: usize) {
    for i in start..start + n {
        mem.store_fact(
            &format!("concept-{i}"),
            &format!("durable content {i}"),
            0.9,
            &[],
            "issue-2550",
        )
        .expect("store_fact");
    }
}

fn total(mem: &LibraryCognitiveMemory) -> u64 {
    mem.get_statistics().expect("get_statistics").total()
}

/// Copy the on-disk store files into a fresh dir, mimicking the on-disk state of
/// a process killed before a clean checkpoint (the WAL still holds
/// committed-but-uncheckpointed records).
fn crash_snapshot(from: &Path, into: &Path) {
    std::fs::create_dir_all(into).unwrap();
    for name in [DB_FILE, WAL_FILE] {
        let src = from.join(name);
        if src.exists() {
            std::fs::copy(&src, into.join(name)).unwrap();
        }
    }
}

/// Truncate the WAL mid-record so a *strict* replay fails but a good prefix
/// remains — the exact shape of the incident.
fn corrupt_wal_tail(wal: &Path) {
    let len = std::fs::metadata(wal).unwrap().len();
    assert!(len > 64, "WAL must be non-trivial to corrupt: {len} bytes");
    let f = std::fs::OpenOptions::new().write(true).open(wal).unwrap();
    f.set_len(len - 41).unwrap();
    f.sync_all().unwrap();
}

/// The core incident invariant: after a corrupt-WAL recovery salvages a good
/// prefix, a SUBSEQUENT open must still see those records — never a reset to
/// empty.
#[test]
fn recovered_prefix_is_durable_across_a_reopen_not_reset() {
    // 1. Seed a store leaving records uncheckpointed in the WAL: fewer writes
    //    than the auto-checkpoint interval (128), and skip the closing
    //    checkpoint via mem::forget so the WAL still carries them.
    let live = tempfile::tempdir().unwrap();
    {
        let mem = LibraryCognitiveMemory::open(live.path()).expect("open live store");
        seed_facts(&mem, 0, 100);
        std::mem::forget(mem); // unclean: no close, no final checkpoint
    }
    let wal = live.path().join(WAL_FILE);
    assert!(
        wal.exists(),
        "seeded store must have a WAL at {}",
        wal.display()
    );

    // 2. Snapshot the on-disk files and corrupt the WAL tail of the copy.
    let crash = tempfile::tempdir().unwrap();
    crash_snapshot(live.path(), crash.path());
    corrupt_wal_tail(&crash.path().join(WAL_FILE));

    // 3. First recovery open: the resilient path replays the good prefix.
    let recovered = {
        let mem = LibraryCognitiveMemory::open(crash.path())
            .expect("recovery open must succeed, never crash on a corrupt WAL");
        let c = total(&mem);
        assert!(
            c > 0,
            "recovery must salvage the good prefix, not open empty (got {c})"
        );
        // Fold the survivors into the main DB so the next open needs no replay.
        mem.checkpoint().expect("checkpoint after recovery");
        c
    };

    // 4. SECOND open — the incident's fatal step. A store that just recovered
    //    records must NOT be re-quarantined and reset to empty.
    let mem = LibraryCognitiveMemory::open(crash.path())
        .expect("second open of a recovered store must succeed");
    let c = total(&mem);
    assert!(
        c > 0,
        "DATA LOSS REGRESSION: a recovered store was reset to empty on reopen \
         (had {recovered}, now {c})"
    );
    assert_eq!(
        c, recovered,
        "recovered record count must be stable across reopen (was {recovered}, now {c})"
    );
}

/// Plain durability sanity, independent of corruption: explicitly checkpointed
/// records must survive repeated clean reopens and never be silently reset.
#[test]
fn checkpointed_records_survive_repeated_reopens() {
    let root = tempfile::tempdir().unwrap();

    // Round 1: seed + checkpoint + close.
    {
        let mem = LibraryCognitiveMemory::open(root.path()).expect("open");
        seed_facts(&mem, 0, 50);
        mem.checkpoint().expect("checkpoint");
    }

    // Round 2: reopen, verify, add more, checkpoint, close.
    let after_second;
    {
        let mem = LibraryCognitiveMemory::open(root.path()).expect("reopen");
        assert_eq!(
            total(&mem),
            50,
            "all checkpointed facts must survive the first reopen"
        );
        seed_facts(&mem, 50, 25);
        mem.checkpoint().expect("checkpoint");
        after_second = total(&mem);
        assert_eq!(after_second, 75);
    }

    // Round 3: reopen again — nothing may be silently reset.
    {
        let mem = LibraryCognitiveMemory::open(root.path()).expect("reopen 2");
        assert_eq!(
            total(&mem),
            after_second,
            "records must be stable across a third reopen, never reset"
        );
    }
}
