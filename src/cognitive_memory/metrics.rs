//! Cognitive-memory silent-drop counter (issue #1975).
//!
//! Mirrors the `meeting_silent_drop_total` pattern from #1956: an in-process
//! `OnceLock<HashMap<(kind, site), AtomicU64>>` counter that tests can snapshot
//! and reset without touching global state outside their scope.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Label pair for each counter bucket.
type Key = (String, String);

fn counters() -> &'static Mutex<HashMap<Key, AtomicU64>> {
    static COUNTERS: OnceLock<Mutex<HashMap<Key, AtomicU64>>> = OnceLock::new();
    COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Increment the silent-drop counter for `(kind, site)`.
pub fn increment(kind: &str, site: &str) {
    let mut map = counters().lock().expect("metrics lock poisoned");
    map.entry((kind.to_owned(), site.to_owned()))
        .or_insert_with(|| AtomicU64::new(0))
        .fetch_add(1, Ordering::Relaxed);
}

/// Snapshot the current counter value for `(kind, site)`.
pub fn cognitive_memory_silent_drop_count(kind: &str, site: &str) -> u64 {
    let map = counters().lock().expect("metrics lock poisoned");
    map.get(&(kind.to_owned(), site.to_owned()))
        .map(|v| v.load(Ordering::Relaxed))
        .unwrap_or(0)
}

/// Reset **all** counters to zero.  For serial-test isolation only.
pub fn scoped_reset() {
    let mut map = counters().lock().expect("metrics lock poisoned");
    map.clear();
}

// ============================================================================
// PruneOutcome — structured result from prune_old_backups
// ============================================================================

/// Outcome of [`NativeCognitiveMemory::prune_old_backups`]: the caller sees
/// both how many files were successfully removed and which (if any) failed.
#[derive(Debug)]
pub struct PruneOutcome {
    pub removed: usize,
    pub failed: Vec<(PathBuf, std::io::Error)>,
}
