//! Red-phase TDD regression tests for the #4578 worktree presence guard at the
//! `advance_goal` reuse sites.
//!
//! Two reuse paths could hand back / reuse a reaped worktree and crash the
//! cycle with a missing-workspace fault:
//!
//!   1. **Discovery reuse** — `find_live_engineer_for_goal` scans
//!      `engineer-worktrees/<goal>-*` for a live claim and returns its path.
//!      The `typed_goal_session` reuse path returns `Succeeded` from that path
//!      without re-verifying the dir still exists at the moment of use.
//!
//!   2. **Stored-map reuse** — consumers (`subordinate.rs`, `ooda_loop::cycle`)
//!      read a worktree back out of `state.engineer_worktrees` and depend on
//!      its `path()` being on disk.
//!
//! `typed_goal_session` is `#[cfg(not(test))]`, so these tests pin the two
//! **observable, always-compiled** contracts the fix relies on:
//!
//!   * `find_live_engineer_for_goal` never returns a path for a worktree whose
//!     dir has been reaped (the discovery-reuse regression guard).
//!   * A worktree stored in `state.engineer_worktrees` can be detected as stale
//!     via the new [`EngineerWorktree::is_present`] seam after an out-of-band
//!     reap (the stored-map-staleness guard the consumers will call).
//!
//! The `is_present()`-based assertions MUST fail in the red phase (method does
//! not exist yet ⇒ crate will not compile) and MUST pass once the guard lands.

#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::tempdir;

use super::find_live_engineer_for_goal;
use crate::engineer_worktree::{ENGINEER_CLAIM_FILE, EngineerWorktree, WORKTREES_SUBDIR};
use crate::goal_curation::GoalBoard;
use crate::ooda_loop::OodaState;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn git_cmd(repo: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo).env_clear();
    if let Ok(p) = std::env::var("PATH") {
        cmd.env("PATH", p);
    }
    if let Ok(h) = std::env::var("HOME") {
        cmd.env("HOME", h);
    }
    cmd
}

fn run_git(repo: &Path, args: &[&str]) {
    let out = git_cmd(repo, args).output().expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed in {}: {}",
        args,
        repo.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_parent_repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create parent repo dir");
    run_git(dir, &["init", "--initial-branch=main", "--quiet"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "test"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("README.md"), "seed\n").expect("seed file");
    run_git(dir, &["add", "README.md"]);
    run_git(dir, &["commit", "-m", "seed", "--quiet"]);
}

/// Allocate a real per-engineer worktree under `state_root` and register it in
/// `state.engineer_worktrees` keyed by `goal_id`. `allocate()` writes the claim
/// sentinel with the current process PID, so discovery treats it as live.
/// Returns the on-disk worktree path for reap simulation.
fn attach_engineer(
    state: &mut OodaState,
    parent_repo: &Path,
    state_root: &Path,
    goal_id: &str,
) -> PathBuf {
    let wt = EngineerWorktree::allocate(parent_repo, state_root, goal_id)
        .expect("allocate engineer worktree");
    let path = wt.path().to_path_buf();
    assert!(
        path.is_dir(),
        "freshly allocated worktree must exist on disk"
    );
    state.engineer_worktrees.insert(goal_id.to_string(), wt);
    path
}

// ---------------------------------------------------------------------------
// Discovery-reuse regression: never return a reaped path
// ---------------------------------------------------------------------------

/// Control: while the worktree exists and carries a live claim (this process's
/// PID), discovery finds it. This is the happy-path reuse the guard must not
/// break.
#[test]
fn discovery_finds_live_worktree_while_present() {
    let parent = tempdir().expect("tempdir");
    let state_dir = tempdir().expect("tempdir");
    init_parent_repo(parent.path());

    let mut state = OodaState::new(GoalBoard::new());
    let goal_id = "disc-live-goal";
    let wt_path = attach_engineer(&mut state, parent.path(), state_dir.path(), goal_id);

    let found = find_live_engineer_for_goal(state_dir.path(), goal_id);
    assert_eq!(
        found.as_deref(),
        Some(wt_path.as_path()),
        "discovery must return the live worktree path while it is present"
    );
}

/// Reuse-after-reap: once the checkout dir is removed out of band, discovery
/// must NOT hand back a path (the reuse site would otherwise return a stale
/// `Succeeded` and the next cycle crashes with a missing-workspace fault).
#[test]
fn discovery_returns_none_after_worktree_reaped() {
    let parent = tempdir().expect("tempdir");
    let state_dir = tempdir().expect("tempdir");
    init_parent_repo(parent.path());

    let mut state = OodaState::new(GoalBoard::new());
    let goal_id = "disc-reaped-goal";
    let wt_path = attach_engineer(&mut state, parent.path(), state_dir.path(), goal_id);

    // Simulate a concurrent GC/reaper deleting the worktree dir (the claim
    // sentinel goes with it).
    std::fs::remove_dir_all(&wt_path).expect("simulate out-of-band reap");

    let found = find_live_engineer_for_goal(state_dir.path(), goal_id);
    assert!(
        found.is_none(),
        "discovery must not return a reaped worktree path, got {found:?}"
    );
}

// ---------------------------------------------------------------------------
// Stored-map-staleness guard: consumers can detect a reaped stored worktree
// ---------------------------------------------------------------------------

/// A worktree freshly stored in `state.engineer_worktrees` reports present, so
/// live consumers keep reusing it.
#[test]
fn stored_map_worktree_reports_present_while_live() {
    let parent = tempdir().expect("tempdir");
    let state_dir = tempdir().expect("tempdir");
    init_parent_repo(parent.path());

    let mut state = OodaState::new(GoalBoard::new());
    let goal_id = "stored-live-goal";
    attach_engineer(&mut state, parent.path(), state_dir.path(), goal_id);

    let stored = state
        .engineer_worktrees
        .get(goal_id)
        .expect("worktree must be tracked in the stored map");
    assert!(
        stored.is_present(),
        "a live stored worktree must report present so consumers reuse it"
    );
}

/// The stored-map-staleness contract: after the checkout dir is reaped out of
/// band, the worktree still sitting in `state.engineer_worktrees` must report
/// NOT present. This is the signal the stored-map consumers use to drop the
/// stale entry and re-provision instead of dereferencing a missing path.
#[test]
fn stored_map_worktree_reports_absent_after_reap() {
    let parent = tempdir().expect("tempdir");
    let state_dir = tempdir().expect("tempdir");
    init_parent_repo(parent.path());

    let mut state = OodaState::new(GoalBoard::new());
    let goal_id = "stored-reaped-goal";
    let wt_path = attach_engineer(&mut state, parent.path(), state_dir.path(), goal_id);

    // Reaper removes the checkout out of band; the map entry is now stale.
    std::fs::remove_dir_all(&wt_path).expect("simulate out-of-band reap");
    assert!(
        !wt_path.exists(),
        "precondition: the stored worktree's dir has been reaped"
    );

    let stored = state
        .engineer_worktrees
        .get(goal_id)
        .expect("stale entry is still in the map until a consumer drops it");
    assert!(
        !stored.is_present(),
        "a reaped stored worktree must report not-present so consumers re-provision"
    );
}

/// Sanity anchor for the fixtures: the reaped dir really lived under the
/// managed `engineer-worktrees/<goal>-*` root, so the guards above exercise the
/// production layout rather than an arbitrary temp path.
#[test]
fn attached_worktree_lives_under_managed_root() {
    let parent = tempdir().expect("tempdir");
    let state_dir = tempdir().expect("tempdir");
    init_parent_repo(parent.path());

    let mut state = OodaState::new(GoalBoard::new());
    let goal_id = "layout-goal";
    let wt_path = attach_engineer(&mut state, parent.path(), state_dir.path(), goal_id);

    let managed_root = state_dir.path().join(WORKTREES_SUBDIR);
    assert!(
        wt_path.starts_with(&managed_root),
        "worktree {} must live under managed root {}",
        wt_path.display(),
        managed_root.display()
    );
    assert!(
        wt_path.join(ENGINEER_CLAIM_FILE).exists(),
        "allocate() must have written the claim sentinel used by discovery"
    );
}
