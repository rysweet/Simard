//! Production wiring for the guard's injectable seams (issue #2704).
//!
//! The guard ([`super::guard`]) and executor ([`super::executor`]) are pure over
//! their trait seams so the safety rails can be proven hermetically. This module
//! supplies the **live** implementations that compose the already-audited
//! `worktree_gc` primitives:
//!
//! - [`RealTrackedWorktreeProbe`] — re-derives the merged/closed-PR + uncommitted
//!   /unpushed vetoes for a tracked worktree at vet time, reusing
//!   [`crate::worktree_gc::worktree_has_uncommitted_or_unpushed_work`]. It only
//!   returns [`WorktreeVerdict::Reclaimable`] on a **positively confirmed**
//!   merged/closed PR — never on "merge-base is an ancestor of main" (the old
//!   misfire is structurally impossible here).
//! - [`DerivingPathRemover`] — derives each tracked worktree's parent repo from
//!   `git worktree list` and delegates to the hardened [`RealPathRemover`], so a
//!   single executor remover handles candidates spanning multiple repos.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::worktree_gc::{parse_worktree_list, worktree_has_uncommitted_or_unpushed_work};

use super::executor::{PathRemover, RealPathRemover};
use super::guard::{ReclaimPrimitive, RejectReason, TrackedWorktreeProbe, WorktreeVerdict};

/// Live [`TrackedWorktreeProbe`] re-deriving the tracked-worktree vetoes via
/// `git` / `gh` at vet time. Fail-closed: any inconclusive signal → `Reject`.
pub struct RealTrackedWorktreeProbe;

impl TrackedWorktreeProbe for RealTrackedWorktreeProbe {
    fn assess(&self, worktree: &Path) -> WorktreeVerdict {
        // #2553 veto — a dirty tree or commits not on a configured upstream is
        // recoverable work; refuse regardless of PR state. Reuses the exact
        // audited helper so the two paths never drift.
        if worktree_has_uncommitted_or_unpushed_work(worktree) {
            return WorktreeVerdict::Reject(RejectReason::UncommittedOrUnpushed);
        }

        // Resolve the branch this worktree has checked out. A detached HEAD (or
        // an unreadable git state) cannot be mapped to a PR — refuse.
        let Some(branch) = worktree_branch(worktree) else {
            return WorktreeVerdict::Reject(RejectReason::UnknownPrState);
        };

        // Positively confirmed merged OR closed PR for this branch? Nothing else
        // qualifies — "merge-base is an ancestor of main" is NOT consulted.
        match branch_pr_merged_or_closed(worktree, &branch) {
            Some(true) => WorktreeVerdict::Reclaimable,
            // Confirmed *not* merged/closed, or the check could not answer:
            // both fail-closed to human review.
            Some(false) | None => WorktreeVerdict::Reject(RejectReason::UnknownPrState),
        }
    }
}

/// Read the branch name (without `refs/heads/`) checked out in `worktree`.
/// Returns `None` for a detached HEAD or an unreadable git state.
fn worktree_branch(worktree: &Path) -> Option<String> {
    let out = git_capture(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = out.trim();
    if branch.is_empty() || branch == "HEAD" {
        None
    } else {
        Some(branch.to_string())
    }
}

/// `Some(true)` iff `gh` reports **at least one** PR whose head is `branch` and
/// **every** such PR is MERGED or CLOSED (none OPEN). `Some(false)` when there
/// are no PRs, or *any* PR is still OPEN — an actively-reviewed worktree must
/// never be reclaimed. `None` when `gh` could not be consulted or its output
/// could not be parsed (fail-closed at the call site). `gh` is invoked in the
/// worktree so it infers the repo from the checkout; it is read-only and its
/// token is never logged.
fn branch_pr_merged_or_closed(worktree: &Path, branch: &str) -> Option<bool> {
    let out = Command::new("gh")
        .arg("-C")
        .arg(worktree)
        .args([
            "pr", "list", "--head", branch, "--state", "all", "--json", "state",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    all_prs_merged_or_closed(&out.stdout)
}

/// Decide reclaimability from the raw `gh pr list --json state` stdout (a JSON
/// array of `{"state": "..."}`). `Some(true)` iff at least one PR exists and
/// every PR is MERGED or CLOSED; `Some(false)` for no PRs or any OPEN/unexpected
/// state; `None` if the JSON cannot be parsed (fail-closed at the call site).
///
/// This is a real parse rather than a substring scan on purpose: a single branch
/// head can carry **both** a historical MERGED/CLOSED PR **and** a currently
/// OPEN one (merged-then-reused, or closed-then-reopened as a new PR). A
/// substring OR over the whole payload would see `"MERGED"` and reclaim the
/// still-open PR's worktree. The safety question is "are *all* PRs for this head
/// done?", never "is *some* PR done?".
fn all_prs_merged_or_closed(gh_stdout: &[u8]) -> Option<bool> {
    #[derive(serde::Deserialize)]
    struct PrState {
        state: String,
    }
    let prs: Vec<PrState> = serde_json::from_slice(gh_stdout).ok()?;
    // At least one PR must exist AND none may be OPEN. Any non-terminal state
    // (OPEN, or anything unexpected) fails closed to human review.
    let all_terminal = prs
        .iter()
        .all(|p| matches!(p.state.to_uppercase().as_str(), "MERGED" | "CLOSED"));
    Some(!prs.is_empty() && all_terminal)
}

/// Capture stdout of a hardened `git -C <dir> <args>` invocation, or `None` on
/// spawn failure / non-zero exit.
fn git_capture(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// A [`PathRemover`] that derives each tracked worktree's parent repo from
/// `git worktree list` and delegates to the hardened [`RealPathRemover`]. Lets a
/// single executor-level remover handle candidates spanning multiple managed
/// repos without the caller pre-computing a per-repo remover.
pub struct DerivingPathRemover {
    pub allow_roots: Vec<PathBuf>,
}

impl PathRemover for DerivingPathRemover {
    fn remove(&self, primitive: ReclaimPrimitive, path: &Path) -> Result<(), String> {
        let parent_repo = match primitive {
            // `git worktree remove` must run from a working tree connected to
            // the same repository; resolve the main worktree from the porcelain
            // listing (the first `worktree` entry).
            ReclaimPrimitive::GitWorktreeRemoveForce => main_worktree_of(path)
                .ok_or_else(|| format!("cannot resolve parent repo for {}", path.display()))?,
            // `rm -rf` does not consult parent_repo.
            ReclaimPrimitive::RemoveDir => PathBuf::new(),
        };
        RealPathRemover {
            parent_repo,
            allow_roots: self.allow_roots.clone(),
        }
        .remove(primitive, path)
    }
}

/// Resolve the main worktree (parent repo) for a given worktree via
/// `git -C <worktree> worktree list --porcelain` — the first entry is the main
/// worktree. Returns `None` if the listing cannot be read or parsed.
pub fn main_worktree_of(worktree: &Path) -> Option<PathBuf> {
    let listing = git_capture(worktree, &["worktree", "list", "--porcelain"])?;
    parse_worktree_list(&listing)
        .into_iter()
        .next()
        .map(|e| e.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_branch_none_on_missing_repo() {
        // A path that is not a git repo yields no branch (fail-closed).
        let tmp = tempfile::tempdir().unwrap();
        assert!(worktree_branch(tmp.path()).is_none());
    }

    /// Regression (issue #4722): the tracked-worktree routing must decide
    /// staleness from a **positively-confirmed merged/closed PR** and must NEVER
    /// use a `git merge-base` branch-ancestry test against origin/main. A fresh
    /// worktree at origin/main is an ancestor of origin/main, so an ancestry test
    /// would wrongly flag it and delete its live build cache. This source-scan
    /// locks the invariant: if anyone reintroduces the ancestry staleness flag
    /// into this module, this test goes red.
    ///
    /// The forbidden flag needle is assembled with `concat!` so this test's own
    /// source text never contains the contiguous literal (which would make the
    /// scan self-match).
    #[test]
    fn routing_never_uses_merge_base_ancestry_flag() {
        let src = include_str!("prod.rs");
        let forbidden_flag = concat!("--is", "-ancestor");
        let forbidden_bare = concat!("is", "-ancestor");
        assert!(
            !src.contains(forbidden_flag),
            "prod.rs must not reintroduce the `git merge-base` ancestry staleness flag ({forbidden_flag})",
        );
        assert!(
            !src.contains(forbidden_bare),
            "no branch-ancestry staleness check may appear in the reclaim routing",
        );
    }

    #[test]
    fn pr_state_no_prs_is_not_reclaimable() {
        // An empty array means no PR references this head — never reclaimable.
        assert_eq!(all_prs_merged_or_closed(b"[]"), Some(false));
    }

    #[test]
    fn pr_state_all_terminal_is_reclaimable() {
        // At least one PR and every PR MERGED/CLOSED → reclaimable.
        assert_eq!(
            all_prs_merged_or_closed(br#"[{"state":"MERGED"}]"#),
            Some(true)
        );
        assert_eq!(
            all_prs_merged_or_closed(br#"[{"state":"CLOSED"}]"#),
            Some(true)
        );
        assert_eq!(
            all_prs_merged_or_closed(br#"[{"state":"MERGED"},{"state":"CLOSED"}]"#),
            Some(true)
        );
    }

    #[test]
    fn pr_state_any_open_is_not_reclaimable() {
        // THE BYPASS THIS FIX CLOSES: a head with a historical MERGED PR AND a
        // currently OPEN PR must NOT be reclaimed — the substring OR used to see
        // "MERGED" and remove the in-review worktree.
        assert_eq!(
            all_prs_merged_or_closed(br#"[{"state":"MERGED"},{"state":"OPEN"}]"#),
            Some(false)
        );
        assert_eq!(
            all_prs_merged_or_closed(br#"[{"state":"OPEN"}]"#),
            Some(false)
        );
    }

    #[test]
    fn pr_state_unparseable_is_inconclusive() {
        // Malformed / non-JSON output cannot answer the safety question → None,
        // which the call site maps to UnknownPrState (fail-closed).
        assert_eq!(all_prs_merged_or_closed(b"not json"), None);
        assert_eq!(all_prs_merged_or_closed(b""), None);
    }

    #[test]
    fn main_worktree_of_parses_first_entry() {
        // Drive the porcelain parser directly to prove the "first entry is the
        // main worktree" contract without spawning git.
        let listing = "worktree /home/u/src/Simard\nHEAD abc\nbranch refs/heads/main\n\n\
                       worktree /home/u/src/Simard/worktrees/feat-x\nHEAD def\nbranch refs/heads/feat-x\n";
        let first = parse_worktree_list(listing).into_iter().next().unwrap();
        assert_eq!(first.path, PathBuf::from("/home/u/src/Simard"));
    }

    #[test]
    fn deriving_remover_rejects_unresolvable_worktree_removal() {
        // GitWorktreeRemoveForce on a non-git tempdir cannot resolve a parent
        // repo → hard error, never a silent delete.
        let tmp = tempfile::tempdir().unwrap();
        let remover = DerivingPathRemover {
            allow_roots: vec![tmp.path().to_path_buf()],
        };
        let child = tmp.path().join("wt");
        std::fs::create_dir_all(&child).unwrap();
        let err = remover
            .remove(ReclaimPrimitive::GitWorktreeRemoveForce, &child)
            .expect_err("must not resolve a parent repo for a non-git dir");
        assert!(err.contains("cannot resolve parent repo"), "got: {err}");
        assert!(
            child.exists(),
            "nothing must be removed on resolution failure"
        );
    }
}
