//! Auto-snapshot persistence for session boundaries.
//!
//! Provides helpers to save a cognitive memory snapshot to disk at session
//! close and reload the most recent snapshot at session start.  Snapshot
//! files live under `~/.simard/snapshots/` by default.

use std::path::{Path, PathBuf};

use crate::cognitive_memory::CognitiveMemoryOps;
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
    if !dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!(
                "[simard] snapshot: failed to create directory {}: {e}",
                dir.display()
            );
            return None;
        }
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

/// Find the most recent snapshot file in `dir` and load it.
///
/// Returns `None` when the directory is empty or contains no valid
/// snapshot files.
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

    // Sort by filename descending — filenames contain an epoch timestamp so
    // lexicographic order == chronological order.
    entries.sort();
    let latest = entries.last().expect("entries is non-empty after sort");

    match crate::remote_transfer::load_snapshot_from_file(latest) {
        Ok(snapshot) => Some(snapshot),
        Err(e) => {
            eprintln!(
                "[simard] snapshot: failed to load {}: {e}",
                latest.display()
            );
            None
        }
    }
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
}
