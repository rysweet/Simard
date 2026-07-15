//! Periodic stale-engineer-claim reaper (issue #4099).
//!
//! Closes the WITHIN-INCARNATION `engineer_claims` leak that PR #4095 does not
//! reach: claims whose engineer is provably dead (no worktree, or an idle
//! worktree whose newest-file mtime is stale beyond a generous threshold) but
//! whose goal is never polled again, so the per-goal reclaim paths never fire.
//! This is an ADDITIVE independent sweep run synchronously from the Overseer
//! tick (`run_cycle`), alongside `reconcile_inflight_investigations`. It adds no
//! new thread and reuses the shared ledger release chokepoint.
//!
//! BINDING policy (from `/tmp/claim-reaper-task.txt`, frozen in the session
//! requirements doc):
//!   * FAIL-CLOSED — only reclaim when CONFIDENT the engineer is dead. A fresh
//!     or quiet-but-alive worktree (idle age <= threshold) is NEVER reaped. Any
//!     unknown / IO-error liveness verdict is treated as [`ClaimLiveness::Live`]
//!     (skip), never reclaimed.
//!   * NO WALL-CLOCK KILL — staleness is newest-file-mtime idle detection, NOT a
//!     run-duration cap. A busy engineer that keeps writing is never reaped no
//!     matter how long it runs.
//!   * FAIL-VISIBLE — every reclaim emits exactly one `[simard]` tracing line
//!     naming the `claim_key`, the staleness age (or `n/a`), and the reason.
//!   * REUSE — reclaim flows through [`ClaimLedger::release_engineer_claim`]
//!     (the same idempotent, single-transaction DELETE the rest of the system
//!     uses) plus the worktree-cleanup primitive. NO hand-rolled SQL, no
//!     `--admin`.
//!
//! # Design
//! The type/trait/function SURFACE is the frozen contract the tests pin; the
//! behavioural bodies below implement it. All policy + per-entry containment
//! lives in the pure [`reap_stale_claims`] orchestrator over three injectable
//! seams ([`ClaimLedger`], [`ClaimLivenessProbe`], [`OrphanWorktreeCleanup`]) so
//! the whole sweep is exercised hermetically with fakes — no real filesystem,
//! process, or `gh`.

use std::path::PathBuf;

use crate::typed_ooda::CapabilityHandler;

/// Why the reaper's liveness probe judged a claim's engineer to be dead.
///
/// `NoWorktree` — no engineer-worktree directory backs the claim (nothing to
/// protect); reclaim immediately. `HeartbeatStale` — a worktree exists but its
/// newest-file mtime is idle; the reaper applies the staleness THRESHOLD to the
/// carried `age_secs` to decide whether it is stale enough to reclaim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadReason {
    /// No worktree directory maps to this claim's goal id.
    NoWorktree,
    /// A worktree exists; `age_secs` is its newest-file idle age.
    HeartbeatStale,
}

impl DeadReason {
    /// Stable, log-safe label used in the fail-visible `[simard]` reclaim line.
    pub fn label(self) -> &'static str {
        match self {
            DeadReason::NoWorktree => "no-worktree",
            DeadReason::HeartbeatStale => "heartbeat-stale",
        }
    }
}

/// Rich liveness verdict for a single `claim_key`.
///
/// Deliberately richer than the ledger's `bool`-valued `EngineerLiveness` so the
/// reaper can log the reason + age (fail-visible) and threshold the age. The
/// fail-closed contract is encoded in the variants: any uncertainty (IO error,
/// unreadable worktrees root) MUST map to [`ClaimLiveness::Live`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimLiveness {
    /// The engineer is (or is assumed, fail-closed, to be) alive. Never reaped.
    Live,
    /// The engineer is provably dead. `age_secs` is the worktree idle age for
    /// `HeartbeatStale` and `None` for `NoWorktree`.
    Dead {
        reason: DeadReason,
        age_secs: Option<u64>,
    },
}

/// Filesystem/heartbeat injection seam: assess one `claim_key`'s liveness.
///
/// Production impl scans `<state_root>/engineer-worktrees/` and derives the
/// verdict from worktree presence + newest-file mtime. Tests inject fakes so no
/// real filesystem, process, or `gh` is touched.
pub trait ClaimLivenessProbe: Send + Sync {
    fn assess(&self, claim_key: &str) -> ClaimLiveness;
}

/// Ledger seam the reaper sweeps and reclaims through. Backed in production by
/// [`CapabilityHandler`]; faked in tests. The reaper NEVER issues SQL directly —
/// it reclaims ONLY via [`ClaimLedger::release_engineer_claim`], the shared
/// idempotent chokepoint.
pub trait ClaimLedger {
    /// All `claim_key`s currently in `engineer_claims` (repo-agnostic).
    fn list_engineer_claims(&self) -> Vec<String>;
    /// Release (DELETE) one claim. Idempotent; `Err` is a real failure surfaced
    /// to the caller for per-entry containment, never swallowed silently.
    fn release_engineer_claim(&self, claim_key: &str) -> Result<(), String>;
}

/// Worktree-cleanup seam: remove the orphaned worktree directory backing a
/// reclaimed claim. Production impl routes through the guarded
/// `assert_under_root` + `remove_dir_all` primitive; tests inject a fake.
pub trait OrphanWorktreeCleanup: Send + Sync {
    fn cleanup(&self, claim_key: &str) -> Result<(), String>;
}

/// The reaper's three injected seams, boxed for wiring: the ledger sweep +
/// release chokepoint, the liveness probe, and the orphan-worktree cleanup.
pub type ClaimReaperSeamSet = (
    Box<dyn ClaimLedger>,
    Box<dyn ClaimLivenessProbe>,
    Box<dyn OrphanWorktreeCleanup>,
);

/// What one sweep did. Returned for assertions + tick telemetry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReapSummary {
    /// Claim keys reclaimed this sweep (release + worktree cleanup ran).
    pub reclaimed: Vec<String>,
    /// Claims left untouched (live/fresh/unknown), fail-closed.
    pub skipped: usize,
    /// Claims whose reclaim hit an error and were contained (not aborted).
    pub errors: usize,
}

/// Sweep ALL `engineer_claims` and reclaim those whose engineer is provably
/// dead, independent of per-goal polling.
///
/// Policy applied per claim (see module docs):
///   * `enabled == false` ⇒ no-op (off switch; not even `NoWorktree` reclaimed).
///   * [`ClaimLiveness::Dead`] `{ NoWorktree, .. }` ⇒ reclaim.
///   * [`ClaimLiveness::Dead`] `{ HeartbeatStale, age }` where `age > stale_secs`
///     ⇒ reclaim.
///   * [`ClaimLiveness::Dead`] `{ HeartbeatStale, age }` where `age <= stale_secs`
///     ⇒ skip (fresh / quiet-but-alive; fail-closed).
///   * [`ClaimLiveness::Live`] ⇒ skip.
///
/// Reclaim = [`ClaimLedger::release_engineer_claim`] + [`OrphanWorktreeCleanup`],
/// emitting one fail-visible `[simard]` line. A per-claim error is CONTAINED (the
/// sweep continues) so one bad entry can never abort the tick.
pub fn reap_stale_claims(
    ledger: &dyn ClaimLedger,
    probe: &dyn ClaimLivenessProbe,
    cleanup: &dyn OrphanWorktreeCleanup,
    enabled: bool,
    stale_secs: u64,
) -> ReapSummary {
    let mut summary = ReapSummary::default();

    // Off switch: a disabled reaper is a total no-op — not even a `NoWorktree`
    // claim is reclaimed. Fail-safe against an operator turning the sweep off.
    if !enabled {
        return summary;
    }

    for claim_key in ledger.list_engineer_claims() {
        // Classify the claim's engineer. Fail-closed: anything short of a
        // CONFIDENT dead verdict (Live, or a `HeartbeatStale` age at/under the
        // threshold, or an unknown age) is skipped and never reclaimed.
        let (reason, age_secs) = match probe.assess(&claim_key) {
            ClaimLiveness::Live => {
                summary.skipped += 1;
                continue;
            }
            ClaimLiveness::Dead {
                reason: DeadReason::NoWorktree,
                ..
            } => (DeadReason::NoWorktree, None),
            ClaimLiveness::Dead {
                reason: DeadReason::HeartbeatStale,
                age_secs,
            } => match age_secs {
                // Strictly OLDER than the threshold ⇒ provably stale. Boundary
                // (age == threshold) is protected (no wall-clock kill).
                Some(age) if age > stale_secs => (DeadReason::HeartbeatStale, Some(age)),
                _ => {
                    summary.skipped += 1;
                    continue;
                }
            },
        };

        // FAIL-VISIBLE: one `[simard]` line per reclaim naming the claim_key, its
        // staleness age (or `n/a` for `NoWorktree`), and the reason. Never silent.
        let age_label = age_secs
            .map(|a| format!("{a}s"))
            .unwrap_or_else(|| "n/a".to_string());
        let reason_label = reason.label();
        tracing::warn!(
            target: "simard::claim_reaper",
            claim_key = %claim_key,
            age_secs = %age_label,
            reason = reason_label,
            "[simard] claim-reaper: reclaimed {claim_key} \
             (reason={reason_label}, age={age_label})",
        );

        // Reclaim through the SHARED release chokepoint (idempotent, single
        // transaction). No hand-rolled SQL. A release error is CONTAINED: count
        // it, log it, and move on so one bad entry never aborts the sweep.
        match ledger.release_engineer_claim(&claim_key) {
            Ok(()) => summary.reclaimed.push(claim_key.clone()),
            Err(error) => {
                summary.errors += 1;
                tracing::error!(
                    target: "simard::claim_reaper",
                    claim_key = %claim_key,
                    error = %error,
                    "[simard] claim-reaper release_engineer_claim failed \
                     (contained; row left in place, sweep continues)",
                );
                // Release failed ⇒ the cap slot is NOT freed; do not remove the
                // worktree (it may still back the un-released row).
                continue;
            }
        }

        // Clean the orphaned worktree directory. The cap slot is already
        // reclaimed (row deleted); dir removal is best-effort and its failure is
        // contained so the sweep continues.
        if let Err(error) = cleanup.cleanup(&claim_key) {
            summary.errors += 1;
            tracing::warn!(
                target: "simard::claim_reaper",
                claim_key = %claim_key,
                error = %error,
                "[simard] claim-reaper worktree cleanup failed \
                 (contained; claim row already released)",
            );
        }
    }

    summary
}

/// Extract the goal id from a `claim_key` of the shape `{owner}/{repo}:{goal_id}`.
///
/// Goal ids are validated identifiers (no `:`), so split at the LAST `:`. A
/// malformed key with no `:` maps to itself — it simply fails to match any
/// worktree dir (fail-closed), never a panic.
fn goal_id_from_claim_key(claim_key: &str) -> &str {
    claim_key
        .rsplit_once(':')
        .map(|(_, goal)| goal)
        .unwrap_or(claim_key)
}

/// Newest-file idle age (seconds) under `worktree`, or `None` if not even the
/// directory's own metadata can be read. Walks the whole subtree (including
/// `.git`) so a busy engineer touching ANY file — source or git internals —
/// keeps the worktree fresh and is never reaped. Symlinks are not followed.
fn newest_file_age_secs(worktree: &std::path::Path) -> Option<u64> {
    let newest = newest_mtime(worktree)?;
    let age = std::time::SystemTime::now()
        .duration_since(newest)
        .unwrap_or_default();
    Some(age.as_secs())
}

/// Maximum modification time of `root` and everything beneath it. Seeds with the
/// directory's own mtime so an empty-but-recently-touched worktree still yields
/// an age. IO errors on individual entries are skipped (never a panic).
fn newest_mtime(root: &std::path::Path) -> Option<std::time::SystemTime> {
    let mut newest: Option<std::time::SystemTime> = std::fs::symlink_metadata(root)
        .ok()
        .and_then(|m| m.modified().ok());
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if let Ok(mtime) = meta.modified() {
                newest = Some(match newest {
                    Some(current) if current >= mtime => current,
                    _ => mtime,
                });
            }
            if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    newest
}

/// Production liveness probe: maps a `claim_key` (`{owner}/{repo}:{goal_id}`) to
/// its worktree under `<state_root>/engineer-worktrees/` and derives the verdict
/// from directory presence + newest-file mtime.
///
/// Fail-closed: if the worktrees root cannot be enumerated (IO error) the verdict
/// is [`ClaimLiveness::Live`] — an unreadable root must NEVER be mistaken for
/// `NoWorktree`.
pub struct WorktreeClaimLivenessProbe {
    state_root: PathBuf,
}

impl WorktreeClaimLivenessProbe {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        Self {
            state_root: state_root.into(),
        }
    }
}

impl ClaimLivenessProbe for WorktreeClaimLivenessProbe {
    fn assess(&self, claim_key: &str) -> ClaimLiveness {
        let goal_id = goal_id_from_claim_key(claim_key);
        let worktrees_root = self
            .state_root
            .join(crate::engineer_worktree::WORKTREES_SUBDIR);

        // FAIL-CLOSED: an unreadable / absent worktrees ROOT must NEVER be
        // mistaken for `NoWorktree` — a transient IO error would otherwise
        // mass-reap every live claim. Treat an un-enumerable root as `Live`.
        let entries = match std::fs::read_dir(&worktrees_root) {
            Ok(entries) => entries,
            Err(_) => return ClaimLiveness::Live,
        };

        // Correlate the claim to its worktree the SAME way the spawn/discovery
        // path does: by goal id recovered from the dir name (repo-agnostic).
        let mut matched: Option<PathBuf> = None;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if crate::engineer_worktree::goal_id_from_worktree_dir(name) == goal_id {
                matched = Some(path);
                break;
            }
        }

        let Some(worktree) = matched else {
            // Root readable, no dir maps to this goal ⇒ provably no worktree.
            return ClaimLiveness::Dead {
                reason: DeadReason::NoWorktree,
                age_secs: None,
            };
        };

        // A worktree exists: its idle age is now − newest-file mtime. If not even
        // the dir metadata can be read, fail-closed to `Live` (never reap on an
        // unknown age).
        match newest_file_age_secs(&worktree) {
            Some(age_secs) => ClaimLiveness::Dead {
                reason: DeadReason::HeartbeatStale,
                age_secs: Some(age_secs),
            },
            None => ClaimLiveness::Live,
        }
    }
}

/// Production [`OrphanWorktreeCleanup`]: removes the worktree directory backing a
/// reclaimed claim's goal, under the SAME `assert_under_root` containment guard
/// (canonicalize + prefix-check, closing TOCTOU + path traversal) the OODA
/// cleanup path uses. `OodaState`-free so it can run from the Overseer tick.
///
/// Idempotent: a missing root or missing dir is success (nothing to clean). A
/// corrupt `claim_key` can only fail to match an on-disk dir — the delete target
/// is ALWAYS a discovered directory, never a path constructed from the key.
pub struct WorktreeDirCleanup {
    state_root: PathBuf,
}

impl WorktreeDirCleanup {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        Self {
            state_root: state_root.into(),
        }
    }
}

impl OrphanWorktreeCleanup for WorktreeDirCleanup {
    fn cleanup(&self, claim_key: &str) -> Result<(), String> {
        let goal_id = goal_id_from_claim_key(claim_key);
        let worktrees_root = self
            .state_root
            .join(crate::engineer_worktree::WORKTREES_SUBDIR);

        // Canonical root for the containment guard. A non-existent root means
        // there is nothing to clean — idempotent success.
        let Ok(root_canonical) = worktrees_root.canonicalize() else {
            return Ok(());
        };
        let entries = match std::fs::read_dir(&worktrees_root) {
            Ok(entries) => entries,
            Err(_) => return Ok(()),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if crate::engineer_worktree::goal_id_from_worktree_dir(name) != goal_id {
                continue;
            }
            // MANDATORY guard: refuse to remove anything whose canonical path is
            // not contained in the canonical worktrees root.
            let safe = crate::engineer_worktree::assert_under_root(&path, &root_canonical)?;
            std::fs::remove_dir_all(&safe)
                .map_err(|e| format!("failed to remove worktree dir {}: {e}", safe.display()))?;
        }
        Ok(())
    }
}

/// [`ClaimLedger`] backed by the real capability ledger. Reuses
/// [`CapabilityHandler::release_engineer_claim`] — the shared chokepoint — so the
/// reaper never hand-rolls a DELETE.
impl ClaimLedger for CapabilityHandler {
    fn list_engineer_claims(&self) -> Vec<String> {
        CapabilityHandler::list_engineer_claims(self).unwrap_or_default()
    }

    fn release_engineer_claim(&self, claim_key: &str) -> Result<(), String> {
        CapabilityHandler::release_engineer_claim(self, claim_key).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    // ----- Injected fakes (T6): no real fs, no processes, no gh. -------------

    /// In-memory ledger double. Holds the claim set and records every release so
    /// tests can assert the reaper reclaims ONLY through this shared chokepoint
    /// (never hand-rolled SQL). `release_fails_for` forces a contained error.
    struct FakeLedger {
        claims: RefCell<Vec<String>>,
        released: RefCell<Vec<String>>,
        release_fails_for: Option<String>,
    }

    impl FakeLedger {
        fn new(claims: &[&str]) -> Self {
            Self {
                claims: RefCell::new(claims.iter().map(|s| s.to_string()).collect()),
                released: RefCell::new(Vec::new()),
                release_fails_for: None,
            }
        }

        fn failing_release_for(mut self, key: &str) -> Self {
            self.release_fails_for = Some(key.to_string());
            self
        }
    }

    impl ClaimLedger for FakeLedger {
        fn list_engineer_claims(&self) -> Vec<String> {
            self.claims.borrow().clone()
        }

        fn release_engineer_claim(&self, claim_key: &str) -> Result<(), String> {
            self.released.borrow_mut().push(claim_key.to_string());
            if self.release_fails_for.as_deref() == Some(claim_key) {
                return Err(format!("injected release failure for {claim_key}"));
            }
            self.claims.borrow_mut().retain(|k| k != claim_key);
            Ok(())
        }
    }

    /// Deterministic probe: fixed verdict per claim key.
    struct MapProbe {
        verdicts: BTreeMap<String, ClaimLiveness>,
    }

    impl MapProbe {
        fn new(pairs: &[(&str, ClaimLiveness)]) -> Self {
            Self {
                verdicts: pairs
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), v.clone()))
                    .collect(),
            }
        }
    }

    impl ClaimLivenessProbe for MapProbe {
        fn assess(&self, claim_key: &str) -> ClaimLiveness {
            // Unknown key ⇒ fail-closed Live (never reaped).
            self.verdicts
                .get(claim_key)
                .cloned()
                .unwrap_or(ClaimLiveness::Live)
        }
    }

    /// Records every worktree cleanup so tests can assert the orphaned dir was
    /// cleaned exactly for reclaimed claims. `cleanup_fails_for` forces an error.
    /// Uses a `Mutex` (not `RefCell`) because [`OrphanWorktreeCleanup`] is
    /// `Send + Sync` (the production impl lives in the `Overseer` across ticks).
    struct FakeCleanup {
        cleaned: std::sync::Mutex<Vec<String>>,
        cleanup_fails_for: Option<String>,
    }

    impl FakeCleanup {
        fn new() -> Self {
            Self {
                cleaned: std::sync::Mutex::new(Vec::new()),
                cleanup_fails_for: None,
            }
        }

        fn failing_for(mut self, key: &str) -> Self {
            self.cleanup_fails_for = Some(key.to_string());
            self
        }

        fn cleaned(&self) -> Vec<String> {
            self.cleaned.lock().expect("cleanup mutex").clone()
        }
    }

    impl OrphanWorktreeCleanup for FakeCleanup {
        fn cleanup(&self, claim_key: &str) -> Result<(), String> {
            self.cleaned
                .lock()
                .expect("cleanup mutex")
                .push(claim_key.to_string());
            if self.cleanup_fails_for.as_deref() == Some(claim_key) {
                return Err(format!("injected cleanup failure for {claim_key}"));
            }
            Ok(())
        }
    }

    const STALE_SECS: u64 = 1800;

    fn dead(reason: DeadReason, age: Option<u64>) -> ClaimLiveness {
        ClaimLiveness::Dead {
            reason,
            age_secs: age,
        }
    }

    // ----- T1: no-worktree ⇒ reaped immediately ------------------------------

    #[test]
    fn t1_claim_with_no_worktree_is_reaped_immediately() {
        let key = "rysweet/Simard:g1";
        let ledger = FakeLedger::new(&[key]);
        let probe = MapProbe::new(&[(key, dead(DeadReason::NoWorktree, None))]);
        let cleanup = FakeCleanup::new();

        let summary = reap_stale_claims(&ledger, &probe, &cleanup, true, STALE_SECS);

        assert_eq!(summary.reclaimed, vec![key.to_string()]);
        // Reclaimed ONLY through the shared release chokepoint.
        assert_eq!(*ledger.released.borrow(), vec![key.to_string()]);
        // Ledger row is gone.
        assert!(ledger.list_engineer_claims().is_empty());
        // Orphaned worktree dir cleaned.
        assert_eq!(cleanup.cleaned(), vec![key.to_string()]);
    }

    // ----- T2: fresh worktree (idle age <= threshold) ⇒ NOT reaped -----------

    #[test]
    fn t2_fresh_worktree_is_not_reaped() {
        let key = "rysweet/Simard:busy-goal";
        let ledger = FakeLedger::new(&[key]);
        // Idle only 1s — far under the 1800s threshold ⇒ quiet-but-alive.
        let probe = MapProbe::new(&[(key, dead(DeadReason::HeartbeatStale, Some(1)))]);
        let cleanup = FakeCleanup::new();

        let summary = reap_stale_claims(&ledger, &probe, &cleanup, true, STALE_SECS);

        assert!(
            summary.reclaimed.is_empty(),
            "a fresh worktree must never be reaped (no wall-clock kill)"
        );
        assert!(ledger.released.borrow().is_empty());
        assert_eq!(ledger.list_engineer_claims(), vec![key.to_string()]);
        assert!(cleanup.cleaned().is_empty());
    }

    // ----- T3: stale worktree (idle age > threshold) ⇒ reaped ----------------

    #[test]
    fn t3_stale_worktree_is_reaped() {
        let key = "rysweet/Simard:abandoned";
        let ledger = FakeLedger::new(&[key]);
        // Idle 3600s (> 1800 threshold) ⇒ provably stale.
        let probe = MapProbe::new(&[(key, dead(DeadReason::HeartbeatStale, Some(3600)))]);
        let cleanup = FakeCleanup::new();

        let summary = reap_stale_claims(&ledger, &probe, &cleanup, true, STALE_SECS);

        assert_eq!(summary.reclaimed, vec![key.to_string()]);
        assert_eq!(*ledger.released.borrow(), vec![key.to_string()]);
        assert!(ledger.list_engineer_claims().is_empty());
        assert_eq!(cleanup.cleaned(), vec![key.to_string()]);
    }

    /// Boundary: idle age EXACTLY equal to the threshold is NOT stale (strict
    /// `>`), so a claim sitting on the boundary is protected (fail-closed).
    #[test]
    fn t3b_age_equal_to_threshold_is_not_reaped() {
        let key = "rysweet/Simard:on-the-line";
        let ledger = FakeLedger::new(&[key]);
        let probe = MapProbe::new(&[(key, dead(DeadReason::HeartbeatStale, Some(STALE_SECS)))]);
        let cleanup = FakeCleanup::new();

        let summary = reap_stale_claims(&ledger, &probe, &cleanup, true, STALE_SECS);

        assert!(summary.reclaimed.is_empty());
        assert_eq!(ledger.list_engineer_claims(), vec![key.to_string()]);
    }

    // ----- T4: reclaim goes through the shared release path -------------------

    #[test]
    fn t4_reclaim_uses_release_chokepoint_and_cleans_worktree() {
        let dead_key = "rysweet/Simard:g1";
        let live_key = "rysweet/Simard:running";
        let ledger = FakeLedger::new(&[dead_key, live_key]);
        let probe = MapProbe::new(&[
            (dead_key, dead(DeadReason::NoWorktree, None)),
            (live_key, ClaimLiveness::Live),
        ]);
        let cleanup = FakeCleanup::new();

        let summary = reap_stale_claims(&ledger, &probe, &cleanup, true, STALE_SECS);

        // Exactly the dead claim reclaimed — via release_engineer_claim ONLY.
        assert_eq!(summary.reclaimed, vec![dead_key.to_string()]);
        assert_eq!(*ledger.released.borrow(), vec![dead_key.to_string()]);
        assert_eq!(cleanup.cleaned(), vec![dead_key.to_string()]);
        // The live claim is untouched: row remains, never released or cleaned.
        assert_eq!(ledger.list_engineer_claims(), vec![live_key.to_string()]);
    }

    // ----- T5: config off switch ⇒ no reclaims (see also config.rs tests) ----

    #[test]
    fn t5_disabled_reaper_reclaims_nothing_even_with_dead_claim() {
        let key = "rysweet/Simard:g2";
        let ledger = FakeLedger::new(&[key]);
        // Dead with no worktree — would be reaped if enabled.
        let probe = MapProbe::new(&[(key, dead(DeadReason::NoWorktree, None))]);
        let cleanup = FakeCleanup::new();

        let summary = reap_stale_claims(&ledger, &probe, &cleanup, false, STALE_SECS);

        assert!(summary.reclaimed.is_empty(), "disabled reaper must be a no-op");
        assert!(ledger.released.borrow().is_empty());
        assert_eq!(ledger.list_engineer_claims(), vec![key.to_string()]);
        assert!(cleanup.cleaned().is_empty());
    }

    // ----- Fail-closed: Live and unknown verdicts are never reaped -----------

    #[test]
    fn live_verdict_is_never_reaped() {
        let key = "rysweet/Simard:healthy";
        let ledger = FakeLedger::new(&[key]);
        let probe = MapProbe::new(&[(key, ClaimLiveness::Live)]);
        let cleanup = FakeCleanup::new();

        let summary = reap_stale_claims(&ledger, &probe, &cleanup, true, STALE_SECS);

        assert!(summary.reclaimed.is_empty());
        assert_eq!(ledger.list_engineer_claims(), vec![key.to_string()]);
    }

    #[test]
    fn unknown_liveness_is_treated_as_live_fail_closed() {
        // The probe returns Live for any key it does not know about; a claim with
        // no verdict entry models an IO-unknown assessment and must be skipped.
        let key = "rysweet/Simard:mystery";
        let ledger = FakeLedger::new(&[key]);
        let probe = MapProbe::new(&[]); // no verdict ⇒ Live (fail-closed)
        let cleanup = FakeCleanup::new();

        let summary = reap_stale_claims(&ledger, &probe, &cleanup, true, STALE_SECS);

        assert!(summary.reclaimed.is_empty());
        assert_eq!(ledger.list_engineer_claims(), vec![key.to_string()]);
    }

    // ----- Cross-repo: one sweep handles every repo's claim keys -------------

    #[test]
    fn sweep_handles_claims_across_multiple_repos() {
        let dead_a = "rysweet/Simard:g1";
        let dead_b = "rysweet/amplihack-rs:g2";
        let live_c = "rysweet/agent-kgpacks-rs-audit:g3";
        let ledger = FakeLedger::new(&[dead_a, dead_b, live_c]);
        let probe = MapProbe::new(&[
            (dead_a, dead(DeadReason::NoWorktree, None)),
            (dead_b, dead(DeadReason::HeartbeatStale, Some(9_000))),
            (live_c, ClaimLiveness::Live),
        ]);
        let cleanup = FakeCleanup::new();

        let summary = reap_stale_claims(&ledger, &probe, &cleanup, true, STALE_SECS);

        let mut reclaimed = summary.reclaimed.clone();
        reclaimed.sort();
        let mut expected = vec![dead_a.to_string(), dead_b.to_string()];
        expected.sort();
        assert_eq!(reclaimed, expected);
        assert_eq!(ledger.list_engineer_claims(), vec![live_c.to_string()]);
    }

    // ----- Per-entry containment (R3): one failure never aborts the sweep ----

    #[test]
    fn release_error_is_contained_and_sweep_continues() {
        let bad = "rysweet/Simard:explodes";
        let good = "rysweet/Simard:ok";
        let ledger = FakeLedger::new(&[bad, good]).failing_release_for(bad);
        let probe = MapProbe::new(&[
            (bad, dead(DeadReason::NoWorktree, None)),
            (good, dead(DeadReason::NoWorktree, None)),
        ]);
        let cleanup = FakeCleanup::new();

        let summary = reap_stale_claims(&ledger, &probe, &cleanup, true, STALE_SECS);

        // The good claim is still reclaimed despite the bad claim erroring.
        assert!(summary.reclaimed.contains(&good.to_string()));
        assert!(summary.errors >= 1, "the failing release must be counted, not swallowed");
        assert_eq!(
            ledger.list_engineer_claims(),
            vec![bad.to_string()],
            "the errored claim's row remains (release failed); the good one is gone"
        );
    }

    #[test]
    fn cleanup_error_is_contained_and_sweep_continues() {
        let bad = "rysweet/Simard:dirty";
        let good = "rysweet/Simard:clean";
        let ledger = FakeLedger::new(&[bad, good]);
        let probe = MapProbe::new(&[
            (bad, dead(DeadReason::NoWorktree, None)),
            (good, dead(DeadReason::NoWorktree, None)),
        ]);
        let cleanup = FakeCleanup::new().failing_for(bad);

        let summary = reap_stale_claims(&ledger, &probe, &cleanup, true, STALE_SECS);

        assert!(summary.reclaimed.contains(&good.to_string()));
        assert!(summary.errors >= 1);
    }

    // ----- DeadReason labels feed the fail-visible [simard] line -------------

    #[test]
    fn dead_reason_labels_are_stable() {
        assert_eq!(DeadReason::NoWorktree.label(), "no-worktree");
        assert_eq!(DeadReason::HeartbeatStale.label(), "heartbeat-stale");
    }

    // ----- Production probe: filesystem seam (NoWorktree / fresh / failclosed)-

    /// A claim whose goal has NO worktree directory under an existing (readable)
    /// worktrees root ⇒ `Dead { NoWorktree }`.
    #[test]
    fn probe_reports_no_worktree_when_dir_absent() {
        let state_root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(state_root.path().join("engineer-worktrees"))
            .expect("create empty worktrees root");
        let probe = WorktreeClaimLivenessProbe::new(state_root.path());

        let verdict = probe.assess("rysweet/Simard:ghost-goal");

        assert_eq!(verdict, dead(DeadReason::NoWorktree, None));
    }

    /// A worktree with a freshly-written file ⇒ `Dead { HeartbeatStale }` with a
    /// SMALL idle age (well under any sane threshold) — the reaper then skips it.
    #[test]
    fn probe_reports_small_age_for_fresh_worktree() {
        let state_root = tempfile::tempdir().expect("tempdir");
        let goal = "advance-thing";
        let wt = state_root
            .path()
            .join("engineer-worktrees")
            .join(format!("{goal}-1783168109-b000d0"));
        std::fs::create_dir_all(&wt).expect("create worktree dir");
        std::fs::write(wt.join("progress.txt"), b"working").expect("write fresh file");

        let probe = WorktreeClaimLivenessProbe::new(state_root.path());
        let verdict = probe.assess(&format!("rysweet/Simard:{goal}"));

        match verdict {
            ClaimLiveness::Dead {
                reason: DeadReason::HeartbeatStale,
                age_secs: Some(age),
            } => assert!(age < 300, "fresh worktree idle age should be tiny, got {age}s"),
            other => panic!("expected HeartbeatStale with a small age, got {other:?}"),
        }
    }

    /// FAIL-CLOSED: an unreadable / absent worktrees ROOT must NOT be mistaken
    /// for `NoWorktree`. When the root cannot be enumerated the verdict is
    /// [`ClaimLiveness::Live`] so a transient IO error never mass-reaps live
    /// claims.
    #[test]
    fn probe_is_fail_closed_when_worktrees_root_unreadable() {
        // state_root points at a path with NO engineer-worktrees dir at all;
        // enumeration fails ⇒ fail-closed Live (not NoWorktree).
        let state_root = tempfile::tempdir().expect("tempdir");
        let probe = WorktreeClaimLivenessProbe::new(
            state_root.path().join("does-not-exist-so-root-is-unreadable"),
        );

        let verdict = probe.assess("rysweet/Simard:any-goal");

        assert_eq!(
            verdict,
            ClaimLiveness::Live,
            "unreadable worktrees root must fail-closed to Live, never NoWorktree"
        );
    }
}
