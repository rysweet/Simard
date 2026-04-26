//! Unit tests for the per-engineer git worktree allocator (issue #1197).
//!
//! These tests are written against the public contract in
//! `docs/reference/engineer-worktree-isolation.md`. They MUST fail in the
//! red phase (the module is a placeholder) and MUST pass once the real
//! implementation lands without further test edits.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;

use tempfile::tempdir;

use super::{EngineerWorktree, sweep_orphaned_worktrees};
use crate::error::SimardError;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

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

fn init_parent_repo_no_main(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).expect("create dir");
    run_git(dir, &["init", "--initial-branch=trunk", "--quiet"]);
    run_git(dir, &["config", "user.email", "t@e.com"]);
    run_git(dir, &["config", "user.name", "t"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
    fs::write(dir.join("a"), "x").unwrap();
    run_git(dir, &["add", "a"]);
    run_git(dir, &["commit", "-m", "x", "--quiet"]);
    dir.to_path_buf()
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

fn git_output(repo: &Path, args: &[&str]) -> String {
    let out = git_cmd(repo, args).output().expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn worktree_registered(parent_repo: &Path, path: &Path) -> bool {
    let listing = git_output(parent_repo, &["worktree", "list", "--porcelain"]);
    let needle = format!("worktree {}", path.display());
    listing.lines().any(|l| l == needle)
}

fn branch_exists(parent_repo: &Path, branch: &str) -> bool {
    git_cmd(parent_repo, &["rev-parse", "--verify", "--quiet", branch])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Build a `git` command that mirrors production isolation: clear env, then
/// re-inject only PATH and HOME. Required so other tests cannot poison
/// these fixtures via process-global GIT_DIR / GIT_WORK_TREE.
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

// ---------------------------------------------------------------------------
// Test 1 — allocate creates dir + branch + registration
// ---------------------------------------------------------------------------

#[test]
fn allocate_records_full_40hex_main_sha_on_branch() {
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    let main_sha = git_output(&parent_repo, &["rev-parse", "main"]);
    let main_sha = main_sha.trim();
    assert_eq!(main_sha.len(), 40, "fixture invariant: main is 40-hex");

    let wt =
        EngineerWorktree::allocate(&parent_repo, state_dir.path(), "goal-sha").expect("allocate");
    let branch_sha = git_output(&parent_repo, &["rev-parse", wt.branch()]);
    assert_eq!(branch_sha.trim(), main_sha, "branch must point at main sha");
    wt.cleanup().unwrap();
}

// ---------------------------------------------------------------------------
// Test 11 — git_capture ignores inherited GIT_DIR (F5).
// Set a poisoned GIT_DIR in the test process env; allocate must still
// succeed against the explicitly-passed parent_repo, because the child git
// invocations env_clear before running.
//
// NOTE: env::set_var is process-global; this test must be in its own
// process or run serially. cargo test default runs tests in parallel, so
// we restrict the poisoned var to the duration of the call and accept the
// (small) risk that another concurrent test reads GIT_DIR. Mitigated by
// putting GIT_DIR at a non-existent path that git would error on.
// ---------------------------------------------------------------------------

#[test]
fn git_capture_clears_inherited_git_env() {
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    // SAFETY: env mutation under the local mutex.
    let prior = std::env::var_os("GIT_DIR");
    unsafe {
        std::env::set_var("GIT_DIR", "/nonexistent/poisoned-git-dir");
    }
    let result = EngineerWorktree::allocate(&parent_repo, state_dir.path(), "goal-env");
    unsafe {
        match prior {
            Some(v) => std::env::set_var("GIT_DIR", v),
            None => std::env::remove_var("GIT_DIR"),
        }
    }
    let wt = result.expect("allocate must ignore inherited GIT_DIR");
    wt.cleanup().expect("cleanup");
}

// ---------------------------------------------------------------------------
// Issue #1213 — engineer-claim sentinel + sweep liveness check
// ---------------------------------------------------------------------------

#[test]
fn allocate_writes_engineer_claim_sentinel_with_current_pid() {
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    let wt = EngineerWorktree::allocate(&parent_repo, state_dir.path(), "claim-write")
        .expect("allocate");

    let claim = wt.path().join(super::ENGINEER_CLAIM_FILE);
    let raw = fs::read_to_string(&claim).expect("claim file present");
    // First line is the PID; remaining lines (if any) carry starttime metadata
    // added in #1238. Both the legacy single-line PID-only format and the
    // new (PID, starttime) format are accepted by the readers.
    let pid: i32 = raw
        .lines()
        .next()
        .expect("claim file has at least one line")
        .trim()
        .parse()
        .expect("first line is an i32 pid");
    assert_eq!(
        pid,
        std::process::id() as i32,
        "claim file must record the allocating process PID"
    );

    wt.cleanup().expect("cleanup");
}

#[test]
fn allocate_writes_starttime_alongside_pid_on_linux() {
    // On Linux (where /proc/<pid>/stat is available), the sentinel must
    // include the allocating process's starttime as a second line so the
    // sweep can defend against PID-recycling false positives after a
    // daemon restart (issue #1238).
    if !std::path::Path::new("/proc/self/stat").exists() {
        // /proc unavailable — skip on non-Linux test runners.
        return;
    }
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    let wt = EngineerWorktree::allocate(&parent_repo, state_dir.path(), "claim-starttime")
        .expect("allocate");

    let claim = wt.path().join(super::ENGINEER_CLAIM_FILE);
    let raw = fs::read_to_string(&claim).expect("claim file present");
    let mut lines = raw.lines();
    let pid: i32 = lines.next().unwrap().trim().parse().unwrap();
    let recorded_starttime: u64 = lines
        .next()
        .expect("starttime line must be present on Linux")
        .trim()
        .parse()
        .expect("starttime parses as u64");
    let live_starttime =
        super::read_pid_starttime_public(pid).expect("can read starttime for own pid");
    assert_eq!(
        recorded_starttime, live_starttime,
        "recorded starttime must match the live process's current starttime"
    );

    wt.cleanup().expect("cleanup");
}

#[test]
fn sweep_removes_dir_with_recycled_pid_claim() {
    // Regression test for issue #1238: a sentinel that names a live PID
    // whose starttime DOES NOT match the recorded one (i.e. the original
    // process is gone and the PID has been reassigned) must NOT cause the
    // sweep to skip the worktree. Otherwise the daemon-restart-with-recycled-
    // PID race leaks orphan worktrees forever.
    if !std::path::Path::new("/proc/self/stat").exists() {
        return; // /proc not available
    }
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    let worktrees_root = state_dir.path().join(super::WORKTREES_SUBDIR);
    fs::create_dir_all(&worktrees_root).unwrap();
    let recycled = worktrees_root.join("recycled-claim");
    fs::create_dir_all(&recycled).unwrap();
    // Write a claim naming the test process's PID but with a starttime
    // that cannot match the live process (use 0 — the test process started
    // some non-zero number of jiffies after boot).
    fs::write(
        recycled.join(super::ENGINEER_CLAIM_FILE),
        format!("{}\n0\n", std::process::id()),
    )
    .unwrap();

    let report = sweep_orphaned_worktrees(&parent_repo, state_dir.path()).expect("sweep");

    assert!(
        !recycled.exists(),
        "sweep must remove dir whose claim PID is alive but starttime doesn't match"
    );
    assert!(
        report.removed_orphan_dirs.iter().any(|p| p == &recycled),
        "sweep must record the recycled-PID removal; got {:?}",
        report.removed_orphan_dirs
    );
}

#[test]
fn sweep_keeps_legacy_pid_only_claim_with_live_pid() {
    // Pre-#1238 sentinels are PID-only (no starttime line). They must
    // still be honored as live when the PID is alive — otherwise a daemon
    // upgrade would nuke every existing engineer worktree on the first
    // sweep after the upgrade.
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    let worktrees_root = state_dir.path().join(super::WORKTREES_SUBDIR);
    fs::create_dir_all(&worktrees_root).unwrap();
    let legacy = worktrees_root.join("legacy-pid-only-claim");
    fs::create_dir_all(&legacy).unwrap();
    // Single-line PID-only sentinel naming this test process.
    fs::write(
        legacy.join(super::ENGINEER_CLAIM_FILE),
        format!("{}\n", std::process::id()),
    )
    .unwrap();

    let report = sweep_orphaned_worktrees(&parent_repo, state_dir.path()).expect("sweep");

    assert!(
        legacy.exists(),
        "sweep must not delete legacy PID-only claim with live PID"
    );
    assert!(
        report
            .skipped_live_dirs
            .iter()
            .any(|p| p.canonicalize().ok() == legacy.canonicalize().ok()),
        "sweep must record legacy PID-only claim as skipped-live; got {:?}",
        report.skipped_live_dirs
    );
}

#[test]
fn sweep_skips_unregistered_dir_with_live_engineer_claim() {
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    // Create an unregistered worktree-shaped dir with a live (current) PID
    // claim. Sweep MUST NOT delete it: this simulates the race where git's
    // registration transiently disappeared but the engineer subprocess is
    // still actively running in the cwd.
    let worktrees_root = state_dir.path().join(super::WORKTREES_SUBDIR);
    fs::create_dir_all(&worktrees_root).unwrap();
    let live = worktrees_root.join("live-claimed-1");
    fs::create_dir_all(&live).unwrap();
    fs::write(
        live.join(super::ENGINEER_CLAIM_FILE),
        format!("{}\n", std::process::id()),
    )
    .unwrap();

    let report = sweep_orphaned_worktrees(&parent_repo, state_dir.path()).expect("sweep");

    assert!(
        live.exists(),
        "sweep must not delete dir with live PID claim"
    );
    assert!(
        report
            .skipped_live_dirs
            .iter()
            .any(|p| p.canonicalize().ok() == live.canonicalize().ok()),
        "sweep report must record the skipped-live dir; got {:?}",
        report.skipped_live_dirs
    );
    assert!(
        report.removed_orphan_dirs.is_empty(),
        "sweep must not record any removals; got {:?}",
        report.removed_orphan_dirs
    );
}

#[test]
fn sweep_removes_unregistered_dir_with_dead_engineer_claim() {
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    // Create an unregistered dir with a dead-PID claim (PID 1 belongs to
    // init/systemd which we cannot signal — kill(1, 0) returns EPERM and
    // therefore reads as alive — so use PID 2_147_483_646 which is virtually
    // guaranteed not to exist and not to wrap around to a live one).
    let worktrees_root = state_dir.path().join(super::WORKTREES_SUBDIR);
    fs::create_dir_all(&worktrees_root).unwrap();
    let dead = worktrees_root.join("dead-claimed-1");
    fs::create_dir_all(&dead).unwrap();
    fs::write(dead.join(super::ENGINEER_CLAIM_FILE), "2147483646\n").unwrap();

    let report = sweep_orphaned_worktrees(&parent_repo, state_dir.path()).expect("sweep");

    assert!(
        !dead.exists(),
        "sweep must remove unregistered dir whose claim PID is dead"
    );
    assert!(
        report.removed_orphan_dirs.iter().any(|p| p == &dead),
        "sweep report must record the removal; got {:?}",
        report.removed_orphan_dirs
    );
}
