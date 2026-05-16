//! Pure decision logic for the worktree GC.
//!
//! Tests inject the upstream-status answers (`branch_merged`,
//! `branch_exists_on_origin`) and the on-disk mtime so the policy can be
//! driven without spawning gh / git.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::parse::WorktreeEntry;

/// Why a worktree qualifies for pruning. Multiple reasons may be true at
/// once; the runner records all of them and reports the most-semantic
/// reason first (see `mod.rs` precedence rules).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PruneReason {
    /// The branch has at least one merged PR upstream.
    BranchMerged { pr_numbers: Vec<u32> },
    /// `git ls-remote --heads <remote> <branch>` returned nothing.
    BranchDeletedFromOrigin,
    /// Sentinel/worktree mtime exceeded the configured idle threshold.
    IdleTooLong { age_days: u64 },
}

/// A worktree that the policy says we should prune. Carries enough
/// context for the runner's log line and for the `--dry-run` report.
#[derive(Debug, Clone)]
pub struct GcCandidate {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub reasons: Vec<PruneReason>,
}

impl GcCandidate {
    /// Headline reason for log/report output.
    pub fn primary_reason(&self) -> Option<&PruneReason> {
        self.reasons
            .iter()
            .find(|r| matches!(r, PruneReason::BranchMerged { .. }))
            .or_else(|| {
                self.reasons
                    .iter()
                    .find(|r| matches!(r, PruneReason::BranchDeletedFromOrigin))
            })
            .or_else(|| {
                self.reasons
                    .iter()
                    .find(|r| matches!(r, PruneReason::IdleTooLong { .. }))
            })
    }
}

/// Inputs the policy needs about each worktree, gathered by the runner
/// (filesystem) and the GhClient (gh CLI). Tests construct these
/// directly so the policy can be exercised hermetically.
#[derive(Debug, Clone)]
pub struct CandidateInputs {
    /// Merged PRs upstream targeting `entry.branch`. Empty vec means
    /// "checked, none merged"; see runner for `gh` shellout.
    pub merged_prs: Vec<u32>,
    /// `Some(true)` → branch present on origin; `Some(false)` → branch
    /// absent from origin (deleted); `None` → check skipped (no
    /// `branch` to check, or remote unreachable).
    pub branch_on_origin: Option<bool>,
    /// Most recent mtime of the worktree's claim sentinel, falling back
    /// to the worktree directory mtime when the sentinel is absent.
    pub last_activity: Option<SystemTime>,
    /// `true` if a live process has this worktree as its CWD (issue
    /// #1886). When set, the policy refuses to mark the worktree as a
    /// GC candidate regardless of merged/deleted/idle signals — pruning
    /// a worktree under a running engineer destroys its CWD and forces
    /// the agent to spin until its 1-hour timeout fires.
    pub has_live_process: bool,
}

/// Apply the policy to one worktree entry. Returns `Some(GcCandidate)`
/// if at least one prune reason is true, `None` otherwise.
///
/// Pure — no IO. The runner gathers `inputs` first and then calls this.
pub fn evaluate_candidate(
    entry: &WorktreeEntry,
    inputs: &CandidateInputs,
    now: SystemTime,
    idle_days: u64,
) -> Option<GcCandidate> {
    // Never propose pruning the bare parent.
    if entry.is_bare {
        return None;
    }

    // #1886: live process beats every other signal. Even if the branch
    // is merged or the worktree is "idle" by mtime, an active engineer
    // subprocess inside it means pruning would destroy its CWD.
    if inputs.has_live_process {
        tracing::info!(
            target: "simard::worktree_gc",
            worktree = %entry.path.display(),
            branch = entry.branch.as_deref().unwrap_or("<detached>"),
            "skipping prune: live process detected in worktree (#1886)",
        );
        return None;
    }

    let mut reasons = Vec::new();

    if !inputs.merged_prs.is_empty() {
        reasons.push(PruneReason::BranchMerged {
            pr_numbers: inputs.merged_prs.clone(),
        });
    }

    // Branch-deleted-from-origin only applies when we have a branch to
    // check AND the remote check actually answered. A `None` answer is
    // treated as inconclusive — never prune on a network error.
    if entry.branch.is_some() && inputs.branch_on_origin == Some(false) {
        reasons.push(PruneReason::BranchDeletedFromOrigin);
    }

    if let Some(activity) = inputs.last_activity
        && let Ok(age) = now.duration_since(activity)
    {
        let threshold = Duration::from_secs(idle_days * 24 * 60 * 60);
        if age >= threshold {
            // Saturating arithmetic keeps the result well-defined even
            // for absurd durations from a clock-skewed sentinel.
            let age_days = age.as_secs() / (24 * 60 * 60);
            reasons.push(PruneReason::IdleTooLong { age_days });
        }
    }

    if reasons.is_empty() {
        None
    } else {
        Some(GcCandidate {
            path: entry.path.clone(),
            branch: entry.branch.clone(),
            reasons,
        })
    }
}

/// Helper: read the most recent mtime among `<dir>/.simard-engineer-claim`
/// and `<dir>` itself, returning the newer of the two. Returns `None` if
/// neither path exists or both stat() calls fail.
///
/// Uses an explicit fallback chain so a worktree whose sentinel was
/// removed by hand still gets the dir's own mtime as the activity proxy,
/// and a worktree whose dir never had a sentinel still works.
pub fn worktree_last_activity(dir: &Path) -> Option<SystemTime> {
    let claim = dir.join(crate::engineer_worktree::ENGINEER_CLAIM_FILE);
    let mut best: Option<SystemTime> = None;

    for path in [claim.as_path(), dir] {
        if let Ok(meta) = std::fs::metadata(path)
            && let Ok(mtime) = meta.modified()
        {
            best = Some(match best {
                Some(prev) if prev > mtime => prev,
                _ => mtime,
            });
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(path: &str, branch: Option<&str>) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(path),
            head: Some("abc".to_string()),
            branch: branch.map(String::from),
            is_bare: false,
            is_detached: branch.is_none(),
        }
    }

    #[test]
    fn fresh_worktree_with_open_branch_is_not_a_candidate() {
        let e = entry("/tmp/wt", Some("feat/x"));
        let inputs = CandidateInputs {
            merged_prs: vec![],
            branch_on_origin: Some(true),
            last_activity: Some(SystemTime::now()),
            has_live_process: false,
        };
        let now = SystemTime::now();
        assert!(evaluate_candidate(&e, &inputs, now, 7).is_none());
    }

    #[test]
    fn merged_branch_is_candidate_with_branchmerged_reason() {
        let e = entry("/tmp/wt", Some("feat/x"));
        let inputs = CandidateInputs {
            merged_prs: vec![42],
            branch_on_origin: Some(true),
            last_activity: Some(SystemTime::now()),
            has_live_process: false,
        };
        let cand = evaluate_candidate(&e, &inputs, SystemTime::now(), 7).expect("merged → cand");
        assert_eq!(cand.reasons.len(), 1);
        match &cand.reasons[0] {
            PruneReason::BranchMerged { pr_numbers } => assert_eq!(pr_numbers, &vec![42]),
            other => panic!("wrong reason: {other:?}"),
        }
    }

    #[test]
    fn branch_deleted_from_origin_is_candidate() {
        let e = entry("/tmp/wt", Some("feat/gone"));
        let inputs = CandidateInputs {
            merged_prs: vec![],
            branch_on_origin: Some(false),
            last_activity: Some(SystemTime::now()),
            has_live_process: false,
        };
        let cand =
            evaluate_candidate(&e, &inputs, SystemTime::now(), 7).expect("deleted → candidate");
        assert!(matches!(
            cand.reasons[0],
            PruneReason::BranchDeletedFromOrigin
        ));
    }

    #[test]
    fn idle_above_threshold_is_candidate() {
        let e = entry("/tmp/wt", Some("feat/old"));
        let now = SystemTime::now();
        let activity = now - Duration::from_secs(8 * 24 * 3600);
        let inputs = CandidateInputs {
            merged_prs: vec![],
            branch_on_origin: Some(true),
            last_activity: Some(activity),
            has_live_process: false,
        };
        let cand = evaluate_candidate(&e, &inputs, now, 7).expect("idle → candidate");
        match &cand.reasons[0] {
            PruneReason::IdleTooLong { age_days } => assert!(*age_days >= 8, "got {age_days}"),
            other => panic!("wrong reason: {other:?}"),
        }
    }

    #[test]
    fn idle_below_threshold_is_not_a_candidate() {
        let e = entry("/tmp/wt", Some("feat/x"));
        let now = SystemTime::now();
        let activity = now - Duration::from_secs(2 * 24 * 3600);
        let inputs = CandidateInputs {
            merged_prs: vec![],
            branch_on_origin: Some(true),
            last_activity: Some(activity),
            has_live_process: false,
        };
        assert!(evaluate_candidate(&e, &inputs, now, 7).is_none());
    }

    #[test]
    fn bare_parent_is_never_a_candidate() {
        let mut e = entry("/tmp/repo", None);
        e.is_bare = true;
        let inputs = CandidateInputs {
            merged_prs: vec![1, 2, 3],
            branch_on_origin: Some(false),
            last_activity: Some(SystemTime::UNIX_EPOCH),
            has_live_process: false,
        };
        assert!(
            evaluate_candidate(&e, &inputs, SystemTime::now(), 7).is_none(),
            "bare parent must never qualify, even if every signal is hot"
        );
    }

    #[test]
    fn detached_worktree_skips_branch_deleted_check_but_still_idle_eligible() {
        // No branch → branch-deleted-from-origin must NOT fire even if
        // branch_on_origin is Some(false) (the runner shouldn't even
        // call the branch check, but we defend in the policy too).
        let e = entry("/tmp/det", None);
        let now = SystemTime::now();
        let activity = now - Duration::from_secs(30 * 24 * 3600);
        let inputs = CandidateInputs {
            merged_prs: vec![],
            branch_on_origin: Some(false),
            last_activity: Some(activity),
            has_live_process: false,
        };
        let cand = evaluate_candidate(&e, &inputs, now, 7).expect("idle still applies on detached");
        assert!(
            !cand
                .reasons
                .iter()
                .any(|r| matches!(r, PruneReason::BranchDeletedFromOrigin)),
            "detached worktree must not get BranchDeletedFromOrigin: {cand:?}"
        );
        assert!(matches!(cand.reasons[0], PruneReason::IdleTooLong { .. }));
    }

    #[test]
    fn inconclusive_origin_check_does_not_force_prune() {
        // Network error scenario: `branch_on_origin: None`. Must not
        // produce a candidate by itself, even if the branch is set.
        let e = entry("/tmp/wt", Some("feat/q"));
        let inputs = CandidateInputs {
            merged_prs: vec![],
            branch_on_origin: None,
            last_activity: Some(SystemTime::now()),
            has_live_process: false,
        };
        assert!(evaluate_candidate(&e, &inputs, SystemTime::now(), 7).is_none());
    }

    #[test]
    fn primary_reason_prefers_branchmerged() {
        let e = entry("/tmp/wt", Some("feat/q"));
        let now = SystemTime::now();
        let activity = now - Duration::from_secs(40 * 24 * 3600);
        let inputs = CandidateInputs {
            merged_prs: vec![99],
            branch_on_origin: Some(false),
            last_activity: Some(activity),
            has_live_process: false,
        };
        let cand = evaluate_candidate(&e, &inputs, now, 7).expect("hot in 3 ways");
        assert!(matches!(
            cand.primary_reason(),
            Some(PruneReason::BranchMerged { .. })
        ));
    }

    #[test]
    fn primary_reason_prefers_branchdeleted_over_idle() {
        let e = entry("/tmp/wt", Some("feat/q"));
        let now = SystemTime::now();
        let activity = now - Duration::from_secs(40 * 24 * 3600);
        let inputs = CandidateInputs {
            merged_prs: vec![],
            branch_on_origin: Some(false),
            last_activity: Some(activity),
            has_live_process: false,
        };
        let cand = evaluate_candidate(&e, &inputs, now, 7).expect("hot in 2 ways");
        assert!(matches!(
            cand.primary_reason(),
            Some(PruneReason::BranchDeletedFromOrigin)
        ));
    }

    #[test]
    fn worktree_last_activity_uses_dir_mtime_when_no_sentinel() {
        let tmp = tempfile::tempdir().expect("tmp");
        let activity = worktree_last_activity(tmp.path()).expect("dir has mtime");
        // Dir mtime was set at creation; should be ≤ now.
        assert!(activity <= SystemTime::now());
    }

    #[test]
    fn worktree_last_activity_returns_none_for_missing_dir() {
        let path = PathBuf::from("/nonexistent/definitely/not/here-99999");
        assert!(worktree_last_activity(&path).is_none());
    }
}
