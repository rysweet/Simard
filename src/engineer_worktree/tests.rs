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

#[test]
fn sweep_removes_orphan_dirs_and_preserves_live_worktrees() {
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    let live = EngineerWorktree::allocate(&parent_repo, state_dir.path(), "goal-live")
        .expect("allocate live");

    let orphan = state_dir
        .path()
        .join("engineer-worktrees")
        .join("goal-orphan-9999999999-deadbe");
    fs::create_dir_all(&orphan).expect("create orphan dir");
    fs::write(orphan.join("stale"), b"x").unwrap();

    let report =
        sweep_orphaned_worktrees(&parent_repo, state_dir.path()).expect("sweep must succeed");

    assert!(
        report.removed_orphan_dirs.iter().any(|p| p == &orphan),
        "orphan {} must be reported as removed; got {:?}",
        orphan.display(),
        report.removed_orphan_dirs
    );
    assert!(!orphan.exists(), "orphan dir must be removed from disk");

    assert!(live.path().exists(), "live worktree dir must remain");
    assert!(
        worktree_registered(&parent_repo, live.path()),
        "live worktree registration must survive sweep"
    );

    live.cleanup().unwrap();
}

// ---------------------------------------------------------------------------
// Test 7 — observation scope: parent-repo edits are invisible inside worktree
//
// This is the issue-#1197 root-cause test: a sibling/operator write to the
// shared checkout MUST NOT show up inside an engineer's own worktree.
// ---------------------------------------------------------------------------

#[test]
fn verification_scope_isolates_worktree_from_parent_repo_mutations() {
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    let wt =
        EngineerWorktree::allocate(&parent_repo, state_dir.path(), "goal-iso").expect("allocate");

    let before: Vec<_> = fs::read_dir(wt.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();

    fs::write(parent_repo.join("sibling-write.txt"), b"intruder").unwrap();

    let after: Vec<_> = fs::read_dir(wt.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();

    assert_eq!(
        before, after,
        "parent-repo mutation must NOT be visible from the engineer worktree; \
         this is the root-cause fix for issue #1197"
    );

    wt.cleanup().unwrap();
}

// ---------------------------------------------------------------------------
// Test 8 — goal_id validation (F1): rejects path traversal, ref injection,
// hidden-file leading-dot, and oversized inputs at the boundary.
// ---------------------------------------------------------------------------

#[test]
fn rejects_invalid_goal_id() {
    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    let cases: &[&str] = &[
        "",          // empty
        "../../etc", // path traversal
        "..",        // parent dir
        ".hidden",   // leading dot
        "-rf",       // leading dash (argv injection)
        "has space", // disallowed byte
        "has/slash", // disallowed byte
        "has\nnewl", // control char
    ];
    for bad in cases {
        let err = EngineerWorktree::allocate(&parent_repo, state_dir.path(), bad)
            .expect_err(&format!("goal_id {bad:?} must be rejected"));
        assert!(
            matches!(err, SimardError::ActionExecutionFailed { .. }),
            "expected ActionExecutionFailed for {bad:?}, got {err:?}"
        );
    }

    // 65-byte input must fail; 64-byte must succeed.
    let too_long = "a".repeat(65);
    let err = EngineerWorktree::allocate(&parent_repo, state_dir.path(), &too_long)
        .expect_err("65-byte goal_id must be rejected");
    assert!(
        matches!(err, SimardError::ActionExecutionFailed { .. }),
        "got {err:?}"
    );

    let max_ok = "a".repeat(64);
    let wt = EngineerWorktree::allocate(&parent_repo, state_dir.path(), &max_ok)
        .expect("64-byte goal_id must be accepted");
    wt.cleanup().expect("cleanup max-len worktree");

    // Confirm the worktrees root was NOT polluted by any of the rejected ids.
    let worktrees_root = state_dir.path().join("engineer-worktrees");
    if worktrees_root.exists() {
        for entry in fs::read_dir(&worktrees_root).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            assert!(
                name.starts_with(&max_ok),
                "rejected goal_id leaked to disk as {name:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 9 — sweep skips symlinks (F2/F3).
// A symlink planted under engineer-worktrees/ pointing at an unrelated dir
// must NOT be classified as an orphan and must NOT have its target deleted.
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn sweep_skips_symlinks_and_preserves_targets() {
    use std::os::unix::fs::symlink;

    let parent_dir = tempdir().unwrap();
    let state_dir = tempdir().unwrap();
    let target_dir = tempdir().unwrap();
    let parent_repo = init_parent_repo(parent_dir.path());

    // Create the worktrees root and plant a symlink inside it pointing at
    // a directory whose contents must survive the sweep.
    let worktrees_root = state_dir.path().join("engineer-worktrees");
    fs::create_dir_all(&worktrees_root).unwrap();
    let canary = target_dir.path().join("canary");
    fs::write(&canary, b"do-not-delete").unwrap();

    let link = worktrees_root.join("evil-symlink");
    symlink(target_dir.path(), &link).expect("plant symlink");

    let report = sweep_orphaned_worktrees(&parent_repo, state_dir.path())
        .expect("sweep must succeed even with symlink present");

    assert!(
        report.removed_orphan_dirs.is_empty(),
        "symlink must not be reported as removed orphan; got {:?}",
        report.removed_orphan_dirs
    );
    assert!(
        canary.exists(),
        "symlink target contents must survive sweep"
    );
    // Symlink itself should still be there (skipped, not deleted).
    assert!(
        fs::symlink_metadata(&link).is_ok(),
        "symlink should be left in place for an operator to investigate"
    );
}

// ---------------------------------------------------------------------------
// Test 10 — main_sha must be 40-hex (F7).
// Already covered by the no-main test; add an explicit shape check via the
// happy path: branch must point at the resolved 40-hex sha.
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
