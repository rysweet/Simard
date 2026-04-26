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

use super::EngineerWorktree;
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
#[serial_test::serial]
fn allocate_creates_unique_worktree_under_state_root() {
    let parent_dir = tempdir().expect("tempdir");
    let state_dir = tempdir().expect("tempdir");
    let parent_repo = init_parent_repo(parent_dir.path());

    let wt = EngineerWorktree::allocate(&parent_repo, state_dir.path(), "goal-abc")
        .expect("allocate must succeed against a healthy parent repo");

    let parent_dir_for_worktrees = state_dir.path().join("engineer-worktrees");
    assert!(
        wt.path().starts_with(&parent_dir_for_worktrees),
        "worktree path {} should live under {}",
        wt.path().display(),
        parent_dir_for_worktrees.display()
    );
    assert!(wt.path().is_dir(), "worktree dir must exist on disk");

    assert!(
        wt.branch().starts_with("engineer/goal-abc-"),
        "branch {} should start with engineer/goal-abc-",
        wt.branch()
    );
    assert!(
        branch_exists(&parent_repo, wt.branch()),
        "branch {} must be present in parent repo",
        wt.branch()
    );
    assert!(
        worktree_registered(&parent_repo, wt.path()),
        "worktree must appear in `git worktree list --porcelain`"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — explicit cleanup is total and idempotent
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn cleanup_removes_dir_branch_and_registration_idempotently() {
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    let wt = EngineerWorktree::allocate(&parent_repo, state_dir.path(), "goal-cleanup")
        .expect("allocate");
    let path = wt.path().to_path_buf();
    let branch = wt.branch().to_string();

    wt.cleanup().expect("first cleanup must succeed");

    assert!(!path.exists(), "worktree dir must be removed");
    assert!(
        !worktree_registered(&parent_repo, &path),
        "worktree registration must be pruned"
    );
    assert!(
        !branch_exists(&parent_repo, &branch),
        "branch {branch} must be deleted by cleanup"
    );

    wt.cleanup()
        .expect("second cleanup must be Ok (idempotent)");
}

// ---------------------------------------------------------------------------
// Test 3 — Drop is the safety net for early-return paths
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn drop_runs_cleanup_when_explicit_cleanup_skipped() {
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    let (path, branch) = {
        let wt = EngineerWorktree::allocate(&parent_repo, state_dir.path(), "goal-drop")
            .expect("allocate");
        (wt.path().to_path_buf(), wt.branch().to_string())
    };

    assert!(
        !path.exists(),
        "Drop must remove worktree dir as a safety net"
    );
    assert!(
        !worktree_registered(&parent_repo, &path),
        "Drop must prune git registration"
    );
    assert!(
        !branch_exists(&parent_repo, &branch),
        "Drop should best-effort delete the branch"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — parallel allocations are race-free
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn parallel_allocations_produce_distinct_paths_and_branches() {
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = Arc::new(init_parent_repo(parent_dir.path()));
    let state_root = Arc::new(state_dir.path().to_path_buf());

    const N: usize = 8;
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let parent = Arc::clone(&parent_repo);
            let state = Arc::clone(&state_root);
            thread::spawn(move || {
                EngineerWorktree::allocate(
                    parent.as_path(),
                    state.as_path(),
                    &format!("goal-par-{i}"),
                )
                .expect("parallel allocate")
            })
        })
        .collect();

    let worktrees: Vec<EngineerWorktree> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    let mut paths: Vec<_> = worktrees.iter().map(|w| w.path().to_path_buf()).collect();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), N, "all parallel paths must be unique");

    let mut branches: Vec<_> = worktrees.iter().map(|w| w.branch().to_string()).collect();
    branches.sort();
    branches.dedup();
    assert_eq!(branches.len(), N, "all parallel branches must be unique");

    for wt in &worktrees {
        wt.cleanup().expect("cleanup parallel worktree");
    }
}

// ---------------------------------------------------------------------------
// Test 5 — fail-loud when `main` is unresolvable; no partial state
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn allocate_without_main_branch_returns_hard_error_and_leaves_no_dir() {
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo_no_main(parent_dir.path());

    let err = EngineerWorktree::allocate(&parent_repo, state_dir.path(), "goal-nofb")
        .expect_err("allocation must fail when main is unresolvable; no fallback to HEAD");

    assert!(
        matches!(err, SimardError::ActionExecutionFailed { .. }),
        "expected ActionExecutionFailed, got {err:?}"
    );

    let worktrees_root = state_dir.path().join("engineer-worktrees");
    if worktrees_root.exists() {
        let entries: Vec<_> = fs::read_dir(&worktrees_root)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(
            entries.is_empty(),
            "no partial worktree dir must be left behind on failure; found {:?}",
            entries.iter().map(|e| e.path()).collect::<Vec<_>>()
        );
    }
}

// ---------------------------------------------------------------------------
// Test 6 — startup sweep removes orphan dirs but leaves live worktrees alone
// ---------------------------------------------------------------------------
