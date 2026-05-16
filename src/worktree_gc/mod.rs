//! Worktree garbage collector (issue #1697 follow-up).
//!
//! Enumerates engineer worktrees under a configurable list of roots and
//! prunes those whose:
//!   - branch was merged into main upstream (gh PR list returns nonempty), OR
//!   - have been idle > 7 days (no `.simard-engineer-claim` heartbeat
//!     update, or worktree mtime > 7 days), OR
//!   - branch was deleted from origin.
//!
//! The first-cause precedence is `BranchMerged > BranchDeletedFromOrigin >
//! IdleTooLong` so the operator-facing log line names the most semantic
//! reason first.
//!
//! # Why this exists
//!
//! Engineer worktrees are allocated under
//! `<state_root>/engineer-worktrees/` per OODA cycle. Without GC, merged
//! engineer branches accumulate as ghost worktrees on disk — combined
//! with the per-worktree cargo target dir (issue #1697), each leaked
//! worktree costs 7-12 GB. The disk-fill incident was triggered by 81 GB
//! of orphaned worktrees that no path was ever responsible for cleaning.
//!
//! `cleanup` (in `engineer_worktree::cleanup`) handles the per-allocation
//! tear-down for live OODA cycles. This module handles the cross-cycle
//! and cross-process leakage that `cleanup` cannot see (because the OODA
//! daemon that allocated those worktrees has long since exited).
//!
//! # Safety
//!
//! - `--dry-run` is the default. The CLI demands an explicit `--apply`
//!   to perform any filesystem mutation.
//! - Pruning gates on `git worktree remove --force` first; the `rm -rf`
//!   fallback only triggers if git's removal failed and the dir is
//!   still present (e.g. on filesystems where git left a stub).
//! - All filesystem deletions are guarded by a canonical-prefix check
//!   against the configured roots: a candidate dir whose canonical path
//!   does not live under one of the roots is refused, never deleted.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub mod liveness;
pub mod parse;
pub mod policy;
pub mod runner;

#[cfg(test)]
mod tests;

pub use liveness::{LiveProcessProbe, ProcfsLiveProcessProbe};
pub use parse::{WorktreeEntry, parse_worktree_list};
pub use policy::{GcCandidate, PruneReason, evaluate_candidate};
pub use runner::{GcReport, GhClient, GhClientShell, run_gc};

/// Default idle threshold for the IdleTooLong rule.
pub const DEFAULT_IDLE_DAYS: u64 = 7;

/// Default upstream remote name used when checking
/// `branch deleted from origin`.
pub const DEFAULT_REMOTE: &str = "origin";

/// GC configuration assembled from CLI flags + env defaults.
#[derive(Debug, Clone)]
pub struct GcConfig {
    /// Filesystem roots to scan for engineer worktrees. Worktrees whose
    /// canonical path does not start with any of these roots are skipped.
    pub roots: Vec<PathBuf>,
    /// Parent repository to drive `git worktree list` and
    /// `git worktree remove`. Must already be a valid git working tree
    /// or bare repo.
    pub parent_repo: PathBuf,
    /// `false` → only print what would be pruned. `true` → actually prune.
    pub apply: bool,
    /// Idle threshold (days). Worktrees whose claim/sentinel mtime
    /// exceeds this many days qualify as IdleTooLong.
    pub idle_days: u64,
    /// "Now" reference used by the policy. Tests inject a fixed value;
    /// production passes `SystemTime::now()`.
    pub now: SystemTime,
}

impl GcConfig {
    /// Build a default config: dry-run, 7-day idle threshold, two roots
    /// (the production engineer-worktrees root and the rebase-worktrees
    /// root inside the home checkout). Both can be overridden by the
    /// caller before passing into [`run_gc`].
    pub fn defaults(parent_repo: PathBuf) -> Self {
        Self {
            roots: default_roots(),
            parent_repo,
            apply: false,
            idle_days: DEFAULT_IDLE_DAYS,
            now: SystemTime::now(),
        }
    }
}

/// Resolve the default scan roots. Reads `SIMARD_WORKTREE_GC_ROOTS`
/// (colon-separated) when set; otherwise returns a conservative pair:
/// `<HOME>/.simard/engineer-worktrees` and `<HOME>/src/Simard/worktrees`.
pub fn default_roots() -> Vec<PathBuf> {
    if let Ok(raw) = std::env::var("SIMARD_WORKTREE_GC_ROOTS") {
        let parsed: Vec<PathBuf> = raw
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
        if !parsed.is_empty() {
            return parsed;
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    vec![
        PathBuf::from(format!("{home}/.simard/engineer-worktrees")),
        PathBuf::from(format!("{home}/src/Simard/worktrees")),
    ]
}

/// Returns true iff `dir` lives, after canonicalization, inside any of
/// the configured `roots`. Roots that do not exist are skipped; this
/// allows a host to be missing one of the default roots without failing
/// the safety check on the other.
pub fn under_any_root(dir: &Path, roots: &[PathBuf]) -> bool {
    let Ok(canon_dir) = dir.canonicalize() else {
        return false;
    };
    for root in roots {
        if let Ok(canon_root) = root.canonicalize()
            && canon_dir.starts_with(&canon_root)
        {
            return true;
        }
    }
    false
}
