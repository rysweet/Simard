//! The salience → OODA-Decide handoff (issue #5, thread 7).
//!
//! The salience thread writes **two projections** of one appraisal: the
//! free-text `reason` goes to durable `salience:<goal_id>` facts, and a
//! **numeric-only, validated** ranking goes to `state/salience_signal.json` for
//! the OODA Decide-context builder. This module owns that Decide-facing file —
//! the writer (numeric + validated ids + atomic temp/rename) and the
//! **fail-closed** consumer (staleness + schema + size guards). See
//! `docs/concepts/salience-and-decide.md`.
//!
//! **Status (issue #5):** implemented; [`write_signal`] and
//! [`read_valid_signal`] are covered by the hermetic unit tests in
//! `tests_catalog`.
#![allow(dead_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};

/// Relative path (under the state root) of the Decide-facing signal file.
pub const SIGNAL_REL_PATH: &str = "state/salience_signal.json";

/// The salience thread's default cadence (seconds), mirrored here so the
/// Decide-side consumer can apply the same `2 × interval` staleness window (I7)
/// without importing the thread's config.
pub const DEFAULT_INTERVAL_SECS: u64 = 1800;

/// Hard cap on the on-disk signal file the consumer will read (S8): a larger
/// file is treated as absent (fail-closed), never parsed.
pub const MAX_SIGNAL_BYTES: u64 = 64 * 1024;

/// One Decide-facing ranking entry — **numbers and a validated id only**. There
/// is deliberately no `reason`/string field here (S1); the free-text rationale
/// lives only in `salience:<goal_id>` facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SalienceEntry {
    /// A goal id validated against the live board before being written.
    pub goal_id: String,
    /// Appraised valence, clamped to `[-1.0, 1.0]`.
    pub valence: f64,
    /// Appraised urgency, clamped to `[0.0, 1.0]`.
    pub urgency: f64,
}

/// The Decide-facing salience signal: a generation epoch plus a numeric ranking.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SalienceSignal {
    /// Unix epoch (seconds) the appraisal was generated — drives the staleness
    /// guard (I7).
    pub generated_epoch: u64,
    /// The numeric-only ranking (validated ids, clamped scores).
    pub ranking: Vec<SalienceEntry>,
}

impl SalienceEntry {
    /// Clamp `valence` into `[-1, 1]` and `urgency` into `[0, 1]` (defense in
    /// depth — applied on both write and read).
    pub fn clamped(mut self) -> Self {
        self.valence = self.valence.clamp(-1.0, 1.0);
        self.urgency = self.urgency.clamp(0.0, 1.0);
        self
    }
}

/// Absolute path of the signal file under `state_root`.
pub fn signal_path(state_root: &Path) -> PathBuf {
    state_root.join(SIGNAL_REL_PATH)
}

/// Write the Decide-facing signal atomically (temp file + rename).
///
/// Contract (pinned by tests):
/// - only `valid_goal_ids` are written; entries with an unknown `goal_id` are
///   dropped (S1 — no unvalidated ids reach Decide);
/// - every `valence`/`urgency` is re-clamped before write (defense in depth);
/// - **no string field other than a validated `goal_id`** is ever serialized;
/// - the write is atomic (temp + rename) so a reader never sees a torn file.
pub fn write_signal(
    _state_root: &Path,
    _signal: &SalienceSignal,
    _valid_goal_ids: &[String],
) -> SimardResult<()> {
    let valid: HashSet<&str> = _valid_goal_ids.iter().map(String::as_str).collect();
    // Drop any entry whose id is not on the live board (S1) and re-clamp every
    // score (defense in depth). `SalienceEntry` has no string field beyond the
    // validated `goal_id`, so the serialized file is numeric-only by construction.
    let ranking: Vec<SalienceEntry> = _signal
        .ranking
        .iter()
        .filter(|e| valid.contains(e.goal_id.as_str()))
        .cloned()
        .map(SalienceEntry::clamped)
        .collect();
    let out = SalienceSignal {
        generated_epoch: _signal.generated_epoch,
        ranking,
    };

    let path = signal_path(_state_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SimardError::ArtifactIo {
            path: parent.to_path_buf(),
            reason: format!("creating salience signal dir: {e}"),
        })?;
    }
    let json = serde_json::to_string(&out).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("serialising salience signal: {e}"),
    })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes()).map_err(|e| SimardError::ArtifactIo {
        path: tmp.clone(),
        reason: format!("writing salience signal temp file: {e}"),
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("renaming salience signal temp file into place: {e}"),
    })?;
    Ok(())
}

/// Fail-closed consumer for the OODA Decide-context builder.
///
/// Returns `Some(signal)` **only** when the file is present, well-formed, within
/// [`MAX_SIGNAL_BYTES`], and fresh (`now_epoch - generated_epoch <= 2 *
/// interval_secs`, I7). An absent, truncated, oversized, or schema-mismatched
/// file yields `None` — treated exactly like "no salience input" (S8). On a
/// successful read every field is re-validated and re-clamped. There is **no
/// fallback to a guessed ranking**.
pub fn read_valid_signal(
    _state_root: &Path,
    _now_epoch: u64,
    _interval_secs: u64,
) -> Option<SalienceSignal> {
    let path = signal_path(_state_root);
    // Presence + size guard (S8): an absent or oversized file is treated exactly
    // like "no salience input" — never parsed.
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_SIGNAL_BYTES {
        return None;
    }
    let raw = std::fs::read_to_string(&path).ok()?;
    // Schema guard (S8): a torn/mismatched file yields None, not a guess.
    let signal: SalienceSignal = serde_json::from_str(&raw).ok()?;

    // Staleness guard (I7): a stalled writer cannot pin Decide to an old ranking.
    // `saturating_mul`/`checked_sub` keep the arithmetic total and overflow-safe.
    let max_age = _interval_secs.saturating_mul(2);
    let age = _now_epoch.checked_sub(signal.generated_epoch)?;
    if age > max_age {
        return None;
    }

    // Re-validate + re-clamp every field on the way out (defense in depth).
    let ranking = signal
        .ranking
        .into_iter()
        .map(SalienceEntry::clamped)
        .collect();
    Some(SalienceSignal {
        generated_epoch: signal.generated_epoch,
        ranking,
    })
}

/// Advisory Decide-side ordering (issue #5, thread 7): the validated goal ids
/// from a **fresh** salience signal, in descending appraised urgency. Returns an
/// empty vector when no fresh, well-formed signal exists (fail-closed) — the
/// caller then leaves its priorities exactly as Orient produced them.
///
/// This is *advice only*: it can reorder which goals Decide considers first
/// under the concurrency cap; it never changes an action's kind and never
/// dispatches anything. See `docs/concepts/salience-and-decide.md`.
pub fn advisory_priority_order(
    state_root: &Path,
    now_epoch: u64,
    interval_secs: u64,
) -> Vec<String> {
    match read_valid_signal(state_root, now_epoch, interval_secs) {
        Some(signal) => {
            let mut ranking = signal.ranking;
            ranking.sort_by(|a, b| {
                b.urgency
                    .partial_cmp(&a.urgency)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            ranking.into_iter().map(|e| e.goal_id).collect()
        }
        None => Vec::new(),
    }
}
