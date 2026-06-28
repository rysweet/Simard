//! Issue #2420 — migration-aware live-store path resolution (TDD).
//!
//! Root cause being guarded: the verified backup copied the legacy single-file
//! store (`state_root/cognitive_memory.ladybug`), but after the lbug-0.17.x
//! de-fork migration (#2307) the **live** store the daemon opens is
//! `state_root/cognitive`. The periodic daemon backup therefore captured a
//! stale/missing path and produced no fresh verified backup from Jun 20 onward.
//!
//! [`live_store_path`] is the single source of truth both the daemon store-open
//! and the verified backup route through. These tests pin its contract so the
//! two can never silently drift to different paths again.

use std::path::PathBuf;

use super::{LEGACY_STORE_FILE, LIVE_STORE_SUBDIR, LibraryCognitiveMemory, live_store_path};

/// The live-store sub-path constant is exactly what the daemon's persistent
/// open uses (`state_root/cognitive`). If this constant changes without the
/// open path changing in lockstep, verified backups would rot again.
#[test]
fn live_store_subdir_constant_is_cognitive() {
    assert_eq!(LIVE_STORE_SUBDIR, "cognitive");
    assert_eq!(LEGACY_STORE_FILE, "cognitive_memory.ladybug");
}

/// Post-migration host: `state_root/cognitive` exists → the resolver MUST pick
/// it (never the legacy file).
#[test]
fn resolves_migrated_cognitive_store_when_present() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Simulate a migrated host: the live `cognitive` store directory exists,
    // and a stale legacy file is also lying around.
    std::fs::create_dir_all(root.join(LIVE_STORE_SUBDIR)).unwrap();
    std::fs::write(root.join(LEGACY_STORE_FILE), b"stale-legacy").unwrap();

    assert_eq!(
        live_store_path(root),
        root.join(LIVE_STORE_SUBDIR),
        "migrated host must resolve to the live `cognitive` store, not the stale legacy file"
    );
}

/// Un-migrated host: ONLY the legacy single-file store exists → fall back to it
/// so the backup still has a real source (and does not error) on such a host.
#[test]
fn falls_back_to_legacy_when_only_legacy_present() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join(LEGACY_STORE_FILE), b"legacy-store").unwrap();

    assert_eq!(
        live_store_path(root),
        root.join(LEGACY_STORE_FILE),
        "un-migrated host must resolve to the legacy single-file store"
    );
}

/// Fresh `state_root`: neither path exists yet → default to the live
/// `cognitive` path the daemon will create on first open. (Never default to the
/// legacy file — that is the bug we are fixing.)
#[test]
fn defaults_to_cognitive_when_neither_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    assert_eq!(
        live_store_path(root),
        root.join(LIVE_STORE_SUBDIR),
        "fresh state_root must default to the live `cognitive` path, not the legacy file"
    );
}

/// The single most important regression pin: the resolver MUST equal the path
/// the daemon's `LibraryCognitiveMemory::open` actually opens. Opening a store
/// creates `state_root/cognitive`; the resolver must point at exactly that.
#[test]
fn matches_daemon_open_path() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // This is precisely what the daemon does at boot.
    let _store = LibraryCognitiveMemory::open(root).expect("open live store");

    let opened: PathBuf = root.join(LIVE_STORE_SUBDIR);
    assert!(
        opened.exists(),
        "daemon open must create the live `cognitive` store path"
    );
    assert_eq!(
        live_store_path(root),
        opened,
        "verified-backup source must equal the live store the daemon opened"
    );
}
