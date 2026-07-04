//! Auto-snapshot persistence for session boundaries.
//!
//! Provides helpers to save a cognitive memory snapshot to disk at session
//! close and reload the most recent snapshot at session start.  Snapshot
//! files live under `~/.simard/snapshots/` by default.

use std::path::{Path, PathBuf};

use crate::cognitive_memory::{CognitiveMemoryOps, StoreEmptiness};
use crate::error::SimardResult;
use crate::remote_transfer::MemorySnapshot;

/// Default directory for auto-snapshots.
const DEFAULT_SNAPSHOT_DIR: &str = ".simard/snapshots";

/// File extension for snapshot files.
const SNAPSHOT_EXT: &str = "json";

// ────────────────────────────────────────────────────────────────────────────
// Public API
// ────────────────────────────────────────────────────────────────────────────

/// Return the resolved snapshot directory, creating it if necessary.
///
/// Uses `override_dir` when `Some`, otherwise falls back to
/// `~/.simard/snapshots/`.  Returns `None` only when the home directory
/// cannot be determined *and* no override was given.
pub fn snapshot_dir(override_dir: Option<&Path>) -> Option<PathBuf> {
    let dir = match override_dir {
        Some(d) => d.to_path_buf(),
        None => {
            let home = dirs::home_dir()?;
            home.join(DEFAULT_SNAPSHOT_DIR)
        }
    };
    if !dir.exists()
        && let Err(e) = std::fs::create_dir_all(&dir)
    {
        eprintln!(
            "[simard] snapshot: failed to create directory {}: {e}",
            dir.display()
        );
        return None;
    }
    Some(dir)
}

/// Save a cognitive memory snapshot to disk.
///
/// The snapshot is written to `<dir>/<agent_name>-<epoch>.json`.
/// Errors are returned but callers should treat them as non-fatal.
#[allow(deprecated)] // we intentionally use the legacy snapshot API
pub fn save_session_snapshot(
    bridge: &dyn CognitiveMemoryOps,
    agent_name: &str,
    dir: &Path,
) -> SimardResult<PathBuf> {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| crate::error::SimardError::ClockBeforeUnixEpoch {
            reason: e.to_string(),
        })?
        .as_secs();

    let filename = format!("{agent_name}-{epoch}.{SNAPSHOT_EXT}");
    let path = dir.join(&filename);

    crate::remote_transfer::export_memory_snapshot(bridge, agent_name, Some(&path))?;

    Ok(path)
}

/// Find the most recent *loadable* snapshot in `dir` and return it.
///
/// Snapshots are tried newest → oldest. If the newest snapshot file is
/// corrupt or otherwise unreadable — e.g. a partial write from an older
/// binary that predates the crash-safe writer (issue #1918), on-disk
/// corruption, or a payload the current schema can no longer parse — the
/// loader transparently falls back to the most recent snapshot that *does*
/// load instead of discarding the entire retained snapshot history. A
/// session teardown persists up to `keep` snapshots (see
/// [`prune_snapshots`]), so a single bad snapshot at the tip must never
/// silently wipe memory across a restart; graceful degradation to the last
/// good snapshot preserves durable recall. This mirrors the library
/// backend's own "fall back to the most recent verified backup" startup
/// recovery.
///
/// Returns `None` only when the directory is empty, cannot be read, or
/// contains no loadable snapshot file.
#[allow(deprecated)] // we intentionally use the legacy snapshot API
pub fn load_latest_snapshot(dir: &Path) -> Option<MemorySnapshot> {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some(SNAPSHOT_EXT) {
                    Some(path)
                } else {
                    None
                }
            })
            .collect(),
        Err(e) => {
            eprintln!(
                "[simard] snapshot: failed to read directory {}: {e}",
                dir.display()
            );
            return None;
        }
    };

    if entries.is_empty() {
        return None;
    }

    // Sort by filename ascending — filenames embed an epoch timestamp so
    // lexicographic order == chronological order. Walk newest → oldest so a
    // corrupt or unreadable newest snapshot degrades to the most recent VALID
    // snapshot rather than returning `None` and discarding the entire retained
    // history (durable recall: one bad snapshot must not silently wipe memory
    // across a restart).
    entries.sort();
    let total = entries.len();
    for (skipped, path) in entries.iter().rev().enumerate() {
        match crate::remote_transfer::load_snapshot_from_file(path) {
            Ok(snapshot) => {
                if skipped > 0 {
                    eprintln!(
                        "[simard] snapshot: recovered older snapshot {} after skipping {} newer unreadable snapshot(s)",
                        path.display(),
                        skipped
                    );
                }
                return Some(snapshot);
            }
            Err(e) => {
                eprintln!(
                    "[simard] snapshot: failed to load {} ({} of {total}): {e}; trying an older snapshot",
                    path.display(),
                    skipped + 1
                );
            }
        }
    }

    eprintln!(
        "[simard] snapshot: all {total} snapshot(s) in {} were unreadable; no memory restored",
        dir.display()
    );
    None
}

/// Prune old snapshot files, retaining only the `keep` most recent.
///
/// Files are sorted by name (which embeds an epoch timestamp) and the oldest
/// entries beyond the limit are deleted.  Deletion errors are logged but do
/// not abort the prune.
pub fn prune_snapshots(dir: &Path, keep: usize) {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some(SNAPSHOT_EXT) {
                    Some(path)
                } else {
                    None
                }
            })
            .collect(),
        Err(_) => return,
    };

    if entries.len() <= keep {
        return;
    }

    entries.sort();
    let to_remove = entries.len() - keep;
    for path in entries.iter().take(to_remove) {
        if let Err(e) = std::fs::remove_file(path) {
            eprintln!("[simard] snapshot: failed to prune {}: {e}", path.display());
        }
    }
}

/// Import a previously saved snapshot into the cognitive bridge.
///
/// Returns the number of items imported.
#[allow(deprecated)] // we intentionally use the legacy snapshot API
pub fn restore_snapshot(
    bridge: &dyn CognitiveMemoryOps,
    snapshot: &MemorySnapshot,
) -> SimardResult<usize> {
    crate::remote_transfer::import_memory_snapshot(bridge, snapshot)
}

/// Hydrate `bridge` from `snapshot` **only when the store is confirmed empty**,
/// failing closed on a read error (issue #2561).
///
/// This is the **intended** guarded entry point for a session/daemon startup to
/// self-heal a freshly-initialised — or corruption-reset (issue #2550) —
/// cognitive store from its most recent on-disk snapshot. It is **not yet wired
/// into any startup path**: this change ships the correctly-gated primitive, and
/// activating it at startup is deliberately deferred (the on-disk snapshot API is
/// `#[deprecated]` in favour of hive-mind replication). The guard is the crux of
/// #2561: the
/// decision to hydrate is taken from [`CognitiveMemoryOps::probe_emptiness`],
/// which distinguishes a *confirmed-empty* store from one whose reads are
/// *failing*, rather than from a bare count that a swallowed read error can
/// silently zero out.
///
/// Behaviour:
///
/// * [`StoreEmptiness::ConfirmedEmpty`] — the store was read successfully and is
///   genuinely empty → import the snapshot and return `Ok(Some(count))` where
///   `count` is the number of restored items.
/// * [`StoreEmptiness::NonEmpty`] — the store already holds memories → do
///   nothing and return `Ok(None)`. Re-importing would duplicate memories.
/// * `Err(..)` from the probe — a read *failed*. We **fail closed**: the error
///   is propagated and nothing is imported, so a transient read failure can
///   never be mistaken for an empty store and cause a snapshot to be layered on
///   top of still-present-but-unreadable durable memory.
///
/// The order matters: emptiness is confirmed *before* any write, so the import
/// side effect is unreachable on the error path.
#[allow(deprecated)] // restore_snapshot wraps the legacy snapshot API
pub fn auto_restore_if_empty(
    bridge: &dyn CognitiveMemoryOps,
    snapshot: &MemorySnapshot,
) -> SimardResult<Option<usize>> {
    // `?` propagates a read failure BEFORE `restore_snapshot` runs — the
    // fail-closed guarantee that protects durable memory (issue #2561).
    match bridge.probe_emptiness()? {
        StoreEmptiness::ConfirmedEmpty => Ok(Some(restore_snapshot(bridge, snapshot)?)),
        StoreEmptiness::NonEmpty => Ok(None),
    }
}

/// Load the most recent snapshot from `dir` and hydrate `bridge` from it **only
/// when the store is confirmed empty**, failing closed on a read error (issue
/// #2561).
///
/// A convenience wrapper over [`auto_restore_if_empty`] that sources the
/// snapshot with [`load_latest_snapshot`]. Emptiness is confirmed *first*, so a
/// non-empty (or unreadable) store never even reads a snapshot off disk:
///
/// * store confirmed empty and a snapshot exists → `Ok(Some(count))`.
/// * store confirmed empty but no loadable snapshot in `dir` → `Ok(None)`
///   (nothing to restore; the fresh store is left as-is). Note: a *corrupt or
///   unreadable* newest snapshot is currently treated the same as *absent* here
///   — [`load_latest_snapshot`] logs and returns `None`, so this wrapper returns
///   `Ok(None)` rather than surfacing the load error. Surfacing that as `Err`
///   (fail-closed on the snapshot *source*) is deferred to the activation work.
/// * store non-empty → `Ok(None)` (skip; re-importing would duplicate).
/// * probe read failure → `Err(..)`, nothing imported (fail closed).
pub fn auto_restore_latest_if_empty(
    bridge: &dyn CognitiveMemoryOps,
    dir: &Path,
) -> SimardResult<Option<usize>> {
    // Fail closed BEFORE touching a snapshot: only a confirmed-empty store is
    // eligible for hydration.
    match bridge.probe_emptiness()? {
        StoreEmptiness::NonEmpty => return Ok(None),
        StoreEmptiness::ConfirmedEmpty => {}
    }
    match load_latest_snapshot(dir) {
        // Emptiness is already confirmed above, so restore directly.
        Some(snapshot) => Ok(Some(restore_snapshot(bridge, &snapshot)?)),
        None => Ok(None),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_dir_uses_override() {
        let dir = snapshot_dir(Some(Path::new("/custom/path")));
        // We cannot guarantee the dir is created (may not have perms) but
        // the path should be returned when the override is given and the
        // parent exists or creation succeeds.  On CI the parent may not
        // exist, so just verify the function does not panic.
        let _ = dir;
    }

    #[test]
    fn load_latest_snapshot_returns_none_for_empty_dir() {
        let dir = std::env::temp_dir().join("simard-test-empty-snapshots");
        let _ = std::fs::create_dir_all(&dir);
        assert!(load_latest_snapshot(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_latest_snapshot_returns_none_for_missing_dir() {
        let dir = Path::new("/nonexistent/simard/snapshots");
        assert!(load_latest_snapshot(dir).is_none());
    }

    #[test]
    fn round_trip_save_and_load() {
        use crate::bridge_subprocess::InMemoryBridgeTransport;
        use crate::memory_bridge::CognitiveMemoryBridge;
        use serde_json::json;

        let transport =
            InMemoryBridgeTransport::new("test-snapshot", move |method, _params| match method {
                "memory.search_facts" => Ok(json!({
                    "facts": [{
                        "node_id": "f1",
                        "concept": "snapshot-test",
                        "content": "round-trip works",
                        "confidence": 0.95,
                        "source_id": "test",
                        "tags": []
                    }]
                })),
                "memory.recall_procedure" => Ok(json!({"procedures": []})),
                "memory.store_fact" => Ok(json!({"id": "imported-1"})),
                "memory.store_procedure" => Ok(json!({"id": "imported-p1"})),
                "memory.search_episodes_by_keywords" => Ok(json!({"episodes": []})),
                _ => Err(crate::bridge::BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            });
        let bridge = CognitiveMemoryBridge::new(Box::new(transport));

        let dir = std::env::temp_dir().join("simard-test-roundtrip-snapshots");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");

        // Save
        let path = save_session_snapshot(&bridge, "test-agent", &dir).expect("save snapshot");
        assert!(path.exists());

        // Load
        let loaded = load_latest_snapshot(&dir).expect("load snapshot");
        assert_eq!(loaded.facts.len(), 1);
        assert_eq!(loaded.facts[0].concept, "snapshot-test");
        assert_eq!(loaded.source_agent, "test-agent");

        // Restore
        let count = restore_snapshot(&bridge, &loaded).expect("restore snapshot");
        assert_eq!(count, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Issue: durable recall must survive a corrupt/partial "tip" snapshot.
    // A session teardown persists up to `keep` snapshots; if the newest one is
    // unreadable (e.g. a partial write from a pre-#1918 binary, on-disk
    // corruption, or a payload the current schema can no longer parse), the
    // loader must fall back to the most recent snapshot that *does* load rather
    // than returning `None` and silently discarding the entire history.
    #[test]
    fn load_latest_snapshot_falls_back_to_older_valid_when_newest_corrupt() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        // Older, VALID snapshot (bare legacy format is accepted by the loader).
        std::fs::write(
            dir.join("agent-0000000100.json"),
            r#"{"facts":[],"procedures":[],"exported_at":100,"source_agent":"old-valid"}"#,
        )
        .expect("write valid snapshot");
        // Newer, CORRUPT snapshot (simulates an interrupted/partial write).
        std::fs::write(
            dir.join("agent-0000000200.json"),
            b"{ this is not valid json",
        )
        .expect("write corrupt snapshot");

        let loaded = load_latest_snapshot(dir)
            .expect("must fall back to the older valid snapshot instead of returning None");
        assert_eq!(
            loaded.source_agent, "old-valid",
            "loader must recover the most recent LOADABLE snapshot, not surface None for a corrupt tip"
        );
    }

    #[test]
    fn load_latest_snapshot_returns_none_when_all_snapshots_corrupt() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        std::fs::write(dir.join("agent-0000000100.json"), b"not json either")
            .expect("write corrupt snapshot");
        std::fs::write(dir.join("agent-0000000200.json"), b"{ broken")
            .expect("write corrupt snapshot");

        assert!(
            load_latest_snapshot(dir).is_none(),
            "when no snapshot in the directory is loadable the loader must report None"
        );
    }

    #[test]
    fn load_latest_snapshot_prefers_newest_when_valid() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();

        std::fs::write(
            dir.join("agent-0000000100.json"),
            r#"{"facts":[],"procedures":[],"exported_at":100,"source_agent":"older"}"#,
        )
        .expect("write older snapshot");
        std::fs::write(
            dir.join("agent-0000000200.json"),
            r#"{"facts":[],"procedures":[],"exported_at":200,"source_agent":"newer"}"#,
        )
        .expect("write newer snapshot");

        let loaded = load_latest_snapshot(dir).expect("load newest valid snapshot");
        assert_eq!(
            loaded.source_agent, "newer",
            "when the newest snapshot is valid it must be returned unchanged (no needless fallback)"
        );
    }

    #[test]
    fn prune_snapshots_keeps_most_recent() {
        let dir = std::env::temp_dir().join("simard-test-prune-snapshots");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");

        // Create 15 fake snapshot files with ascending epoch names.
        for i in 1u32..=15 {
            let path = dir.join(format!("agent-{i:010}.json"));
            std::fs::write(&path, "{}").expect("write fake snapshot");
        }

        prune_snapshots(&dir, 10);

        let remaining: Vec<_> = std::fs::read_dir(&dir)
            .expect("read dir")
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(remaining.len(), 10, "should retain exactly 10 snapshots");

        // The oldest 5 (epochs 1-5) should be gone; the newest 10 (6-15) remain.
        for i in 1u32..=5 {
            let path = dir.join(format!("agent-{i:010}.json"));
            assert!(!path.exists(), "old snapshot {i} should have been pruned");
        }
        for i in 6u32..=15 {
            let path = dir.join(format!("agent-{i:010}.json"));
            assert!(path.exists(), "recent snapshot {i} should still exist");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ────────────────────────────────────────────────────────────────────
    // Auto-restore fail-closed regression tests (issue #2561)
    //
    // These are fully hermetic: they drive an in-memory bridge transport and
    // (for the disk-sourced path) a per-test tempdir. Nothing touches
    // `$HOME/.simard` or any real cognitive store.
    // ────────────────────────────────────────────────────────────────────

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::bridge::BridgeErrorPayload;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;
    use serde_json::json;

    /// A one-fact, one-procedure snapshot. Restoring it issues exactly two
    /// writes (`store_fact` + `store_procedure`), so an import-side-effect
    /// counter of 2 means the whole snapshot was applied and 0 means nothing
    /// was written.
    fn two_item_snapshot() -> MemorySnapshot {
        serde_json::from_value(json!({
            "facts": [{
                "node_id": "f1",
                "concept": "restore-guard",
                "content": "durable knowledge",
                "confidence": 0.9,
                "source_id": "test",
                "tags": []
            }],
            "procedures": [{
                "node_id": "p1",
                "name": "rebuild",
                "steps": ["compile", "test"],
                "prerequisites": [],
                "usage_count": 0
            }],
            "exported_at": 0,
            "source_agent": "test-agent"
        }))
        .expect("construct snapshot")
    }

    /// Build a destination bridge whose `memory.get_statistics` response is
    /// produced by `stats`, and whose write ops (`store_fact` /
    /// `store_procedure`) bump the returned counter. The counter is the
    /// "durable memory touched" probe: a fail-closed restore must leave it at 0.
    fn dest_bridge(
        stats: impl Fn() -> Result<serde_json::Value, BridgeErrorPayload> + Send + Sync + 'static,
    ) -> (CognitiveMemoryBridge, Arc<AtomicUsize>) {
        let writes = Arc::new(AtomicUsize::new(0));
        let writes_h = Arc::clone(&writes);
        let transport =
            InMemoryBridgeTransport::new("test-dest", move |method, _params| match method {
                "memory.get_statistics" => stats(),
                "memory.store_fact" => {
                    writes_h.fetch_add(1, Ordering::SeqCst);
                    Ok(json!({"id": "imported-fact"}))
                }
                "memory.store_procedure" => {
                    writes_h.fetch_add(1, Ordering::SeqCst);
                    Ok(json!({"id": "imported-proc"}))
                }
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unexpected method: {method}"),
                }),
            });
        (CognitiveMemoryBridge::new(Box::new(transport)), writes)
    }

    fn zero_stats() -> serde_json::Value {
        json!({
            "sensory_count": 0,
            "working_count": 0,
            "episodic_count": 0,
            "semantic_count": 0,
            "procedural_count": 0,
            "prospective_count": 0
        })
    }

    #[test]
    fn auto_restore_hydrates_a_confirmed_empty_store() {
        // Path (a): a store that reads cleanly as all-zeros is genuinely empty,
        // so the snapshot is applied in full.
        let (bridge, writes) = dest_bridge(|| Ok(zero_stats()));
        let snapshot = two_item_snapshot();

        let restored = auto_restore_if_empty(&bridge, &snapshot).expect("confirmed-empty restore");

        assert_eq!(restored, Some(2), "both snapshot items should be restored");
        assert_eq!(
            writes.load(Ordering::SeqCst),
            2,
            "restore should have written the fact and the procedure",
        );
    }

    #[test]
    fn auto_restore_fails_closed_on_read_error_without_wiping_memory() {
        // Path (b): the emptiness probe FAILS (a swallowed read-failure surfaced
        // as an error). The gate must propagate the error and perform NO writes,
        // so still-present-but-unreadable durable memory is never overwritten or
        // duplicated. This is the core #2561 regression guard.
        let (bridge, writes) = dest_bridge(|| {
            Err(BridgeErrorPayload {
                code: -32000,
                message: "transient read failure at startup".to_string(),
            })
        });
        let snapshot = two_item_snapshot();

        let result = auto_restore_if_empty(&bridge, &snapshot);

        assert!(
            result.is_err(),
            "a read failure must be surfaced as Err, never coerced to empty",
        );
        assert_eq!(
            writes.load(Ordering::SeqCst),
            0,
            "fail-closed restore must not write anything (durable memory intact)",
        );
    }

    #[test]
    fn auto_restore_skips_a_non_empty_store() {
        // Path (c): a store that already holds memories must not be re-imported —
        // doing so would duplicate every memory.
        let (bridge, writes) = dest_bridge(|| {
            Ok(json!({
                "sensory_count": 0,
                "working_count": 0,
                "episodic_count": 0,
                "semantic_count": 5,
                "procedural_count": 0,
                "prospective_count": 0
            }))
        });
        let snapshot = two_item_snapshot();

        let restored = auto_restore_if_empty(&bridge, &snapshot).expect("non-empty skip is Ok");

        assert_eq!(restored, None, "a non-empty store must be left untouched");
        assert_eq!(
            writes.load(Ordering::SeqCst),
            0,
            "no snapshot items should be written into a non-empty store",
        );
    }

    #[test]
    fn auto_restore_latest_loads_and_hydrates_when_confirmed_empty() {
        // End-to-end over the disk-sourced convenience wrapper, hermetic via a
        // per-test tempdir: a confirmed-empty store hydrates from the newest
        // on-disk snapshot.
        let dir = std::env::temp_dir().join(format!(
            "simard-test-autorestore-empty-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");

        let snapshot = two_item_snapshot();
        let envelope = crate::remote_transfer::PersistedEnvelope {
            schema_version: crate::remote_transfer::ENVELOPE_SCHEMA_VERSION,
            payload: snapshot,
        };
        std::fs::write(
            dir.join("test-agent-0000000001.json"),
            serde_json::to_vec_pretty(&envelope).expect("serialize snapshot"),
        )
        .expect("write snapshot file");

        let (bridge, writes) = dest_bridge(|| Ok(zero_stats()));
        let restored =
            auto_restore_latest_if_empty(&bridge, &dir).expect("confirmed-empty latest restore");

        assert_eq!(restored, Some(2), "the latest snapshot should be restored");
        assert_eq!(writes.load(Ordering::SeqCst), 2, "both items written");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_restore_latest_fails_closed_on_read_error() {
        // The disk-sourced wrapper must also fail closed: a probe error aborts
        // BEFORE any snapshot is read or applied.
        let dir = std::env::temp_dir().join(format!(
            "simard-test-autorestore-failclosed-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");

        let envelope = crate::remote_transfer::PersistedEnvelope {
            schema_version: crate::remote_transfer::ENVELOPE_SCHEMA_VERSION,
            payload: two_item_snapshot(),
        };
        std::fs::write(
            dir.join("test-agent-0000000001.json"),
            serde_json::to_vec_pretty(&envelope).expect("serialize snapshot"),
        )
        .expect("write snapshot file");

        let (bridge, writes) = dest_bridge(|| {
            Err(BridgeErrorPayload {
                code: -32000,
                message: "transient read failure at startup".to_string(),
            })
        });
        let result = auto_restore_latest_if_empty(&bridge, &dir);

        assert!(result.is_err(), "probe error must abort the restore");
        assert_eq!(
            writes.load(Ordering::SeqCst),
            0,
            "no writes on the fail-closed path (durable memory intact)",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_restore_latest_skips_a_non_empty_store() {
        // Wrapper ordering guard: a non-empty store returns `Ok(None)` from the
        // emptiness check BEFORE any snapshot is read off disk. The tempdir holds
        // a valid snapshot precisely to prove it is never consulted.
        let dir = std::env::temp_dir().join(format!(
            "simard-test-autorestore-latest-nonempty-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");

        let envelope = crate::remote_transfer::PersistedEnvelope {
            schema_version: crate::remote_transfer::ENVELOPE_SCHEMA_VERSION,
            payload: two_item_snapshot(),
        };
        std::fs::write(
            dir.join("test-agent-0000000001.json"),
            serde_json::to_vec_pretty(&envelope).expect("serialize snapshot"),
        )
        .expect("write snapshot file");

        let (bridge, writes) = dest_bridge(|| {
            Ok(json!({
                "sensory_count": 0,
                "working_count": 0,
                "episodic_count": 0,
                "semantic_count": 5,
                "procedural_count": 0,
                "prospective_count": 0
            }))
        });
        let restored =
            auto_restore_latest_if_empty(&bridge, &dir).expect("non-empty latest skip is Ok");

        assert_eq!(
            restored, None,
            "a non-empty store must be left untouched even when a snapshot exists",
        );
        assert_eq!(
            writes.load(Ordering::SeqCst),
            0,
            "no snapshot items should be written into a non-empty store",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_restore_latest_returns_none_when_confirmed_empty_but_no_snapshot() {
        // Confirmed-empty store with an empty snapshot dir: nothing to restore, so
        // the fresh store is left as-is (`Ok(None)`, zero writes). This exercises
        // the "no loadable snapshot" branch — an absent (or corrupt/unreadable)
        // newest snapshot both surface here as `None`.
        let dir = std::env::temp_dir().join(format!(
            "simard-test-autorestore-latest-nosnapshot-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");

        let (bridge, writes) = dest_bridge(|| Ok(zero_stats()));
        let restored = auto_restore_latest_if_empty(&bridge, &dir)
            .expect("confirmed-empty with no snapshot is Ok");

        assert_eq!(
            restored, None,
            "a confirmed-empty store with no loadable snapshot restores nothing",
        );
        assert_eq!(
            writes.load(Ordering::SeqCst),
            0,
            "no writes when there is no snapshot to restore",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
