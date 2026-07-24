//! Red-phase TDD tests for the engineer-worktree **presence guard**
//! (issue #4578).
//!
//! Context: goal-session cycles crashed with a bare missing-workspace fault
//! because discovery / stored-map reuse handed back a worktree path that a
//! concurrent GC/reaper had already removed (a TOCTOU between "found a live
//! claim" and "use the checkout dir"). The fix introduces a single,
//! side-effect-free presence seam owned by the worktree module —
//! [`EngineerWorktree::is_present`] — that every reuse site consults
//! immediately before it depends on the checkout still being on disk.
//!
//! These tests specify the contract for that new accessor. They MUST fail in
//! the red phase (the method does not exist yet ⇒ the crate will not compile)
//! and MUST pass once `is_present()` lands, without further test edits.
//!
//! Contract (aligned with `docs/reference/engineer-worktree-presence-guard.md`):
//!   `is_present()` performs a single **fail-closed** claim read of
//!   `self.path()/.simard-engineer-claim`. It returns `true` iff the checkout
//!   directory still exists AND its claim sentinel is readable; any absence
//!   (dir reaped, sentinel gone, unreadable) returns `false`. It never mutates
//!   the filesystem and never re-provisions.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::tempdir;

use super::{ENGINEER_CLAIM_FILE, EngineerWorktree};

// ---------------------------------------------------------------------------
// Fixtures (self-contained: the sibling `tests` module's helpers are private).
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

/// A parent repo with a committed `main` so `EngineerWorktree::allocate`
/// (which branches off `main` HEAD) succeeds.
fn init_parent_repo(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).expect("create parent repo dir");
    run_git(dir, &["init", "--initial-branch=main", "--quiet"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "test"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
    fs::write(dir.join("README.md"), "seed\n").expect("seed file");
    run_git(dir, &["add", "README.md"]);
    run_git(dir, &["commit", "-m", "seed", "--quiet"]);
    dir.to_path_buf()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A freshly allocated worktree is present: `allocate()` creates the checkout
/// dir and writes the claim sentinel with the current PID, so the guard must
/// report `true`.
#[test]
fn is_present_true_after_allocate() {
    let parent = tempdir().expect("tempdir");
    let state = tempdir().expect("tempdir");
    init_parent_repo(parent.path());

    let wt = EngineerWorktree::allocate(parent.path(), state.path(), "goal-present")
        .expect("allocate engineer worktree");

    assert!(
        wt.path().is_dir(),
        "precondition: freshly allocated worktree must exist on disk"
    );
    assert!(
        wt.is_present(),
        "a freshly allocated worktree with its claim sentinel must be present"
    );
}

/// After the worktree's own `cleanup()` removes the checkout, the guard must
/// report `false` — the core signal a reuse site needs to re-provision instead
/// of returning a stale success.
#[test]
fn is_present_false_after_cleanup() {
    let parent = tempdir().expect("tempdir");
    let state = tempdir().expect("tempdir");
    init_parent_repo(parent.path());

    let wt = EngineerWorktree::allocate(parent.path(), state.path(), "goal-cleaned")
        .expect("allocate engineer worktree");
    assert!(wt.is_present(), "precondition: present before cleanup");

    wt.cleanup().expect("cleanup engineer worktree");

    assert!(
        !wt.is_present(),
        "after cleanup removes the checkout dir, is_present() must be false"
    );
}

/// The #4578 fault exactly: a concurrent reaper removes the checkout dir out
/// of band (no `cleanup()` call on this handle). The guard must still detect
/// the absence so callers never hand back / reuse a reaped worktree.
#[test]
fn is_present_false_when_dir_reaped_out_of_band() {
    let parent = tempdir().expect("tempdir");
    let state = tempdir().expect("tempdir");
    init_parent_repo(parent.path());

    let wt = EngineerWorktree::allocate(parent.path(), state.path(), "goal-reaped")
        .expect("allocate engineer worktree");
    assert!(wt.is_present(), "precondition: present before reap");

    // Simulate the GC/reaper removing the worktree dir underneath us, WITHOUT
    // going through this handle's cleanup(). This is the TOCTOU window.
    fs::remove_dir_all(wt.path()).expect("simulate out-of-band reap");

    assert!(
        !wt.is_present(),
        "a worktree whose dir was reaped out of band must report not-present"
    );
}

/// Fail-closed: if the checkout dir survives but the claim sentinel is gone
/// (a partially reaped / corrupted worktree), the guard must return `false`
/// rather than optimistically treating the checkout as reusable.
#[test]
fn is_present_false_when_claim_sentinel_removed() {
    let parent = tempdir().expect("tempdir");
    let state = tempdir().expect("tempdir");
    init_parent_repo(parent.path());

    let wt = EngineerWorktree::allocate(parent.path(), state.path(), "goal-noclaim")
        .expect("allocate engineer worktree");
    assert!(wt.is_present(), "precondition: present with claim");

    fs::remove_file(wt.path().join(ENGINEER_CLAIM_FILE)).expect("remove claim sentinel");
    assert!(
        wt.path().is_dir(),
        "precondition: only the sentinel is gone, the dir remains"
    );

    assert!(
        !wt.is_present(),
        "missing claim sentinel must fail closed to not-present"
    );
}

/// The guard is a pure observation: calling it must not create, delete, or
/// otherwise mutate the worktree. Repeated calls are stable.
#[test]
fn is_present_is_side_effect_free_and_idempotent() {
    let parent = tempdir().expect("tempdir");
    let state = tempdir().expect("tempdir");
    init_parent_repo(parent.path());

    let wt = EngineerWorktree::allocate(parent.path(), state.path(), "goal-pure")
        .expect("allocate engineer worktree");

    assert!(wt.is_present());
    assert!(wt.is_present());
    assert!(
        wt.path().is_dir() && wt.path().join(ENGINEER_CLAIM_FILE).exists(),
        "is_present() must not have mutated the worktree or its sentinel"
    );
}
