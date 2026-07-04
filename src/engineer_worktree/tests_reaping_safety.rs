//! Issue #2553 — daemon engineer-sweep reaping-safety guards (RED phase, TDD).
//!
//! These tests specify the contract in
//! `docs/reference/engineer-worktree-sweep-safety.md` for the OODA daemon's
//! periodic engineer sweep (`src/engineer_worktree/sweep.rs`). They MUST fail
//! before the implementation lands (the injected-probe entry point, the new
//! `SweepReport` skip buckets, and `RemovalReason` do not exist yet) and MUST
//! pass once it lands **without further test edits**.
//!
//! The guards, cheapest-first, most-destructive-last, are:
//!   1. SCOPE — only ever operate under `<state_root>/engineer-worktrees/`.
//!   2. LIVE_CLAIM — skip a live `.simard-engineer-claim` (existing #1213/#1238).
//!   3. LIVE_CWD — skip a dir that is the CWD of any live process (new #2553,
//!      reuses `worktree_gc::liveness`; fail-closed → keep).
//!   4. WORK_STATE — skip a real git worktree with uncommitted / unpushed work
//!      or an unprovable-safe git state (new #2553; fail-safe).
//!   5. REAP — remove only orphan dirs that pass every guard, recording
//!      the observable `RemovalReason`.
//!
//! All tests are offline, serial, sleep-free, and network-free (bare remotes
//! are local `file://` paths). Liveness is injected via
//! `worktree_gc::liveness::FakeLiveProcessProbe` so no process must be spawned.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::{TempDir, tempdir};

use super::{
    ENGINEER_CLAIM_FILE, RemovalReason, WORKTREES_SUBDIR, sweep_orphaned_worktrees,
    sweep_orphaned_worktrees_inner,
};
use crate::worktree_gc::liveness::{FakeLiveProcessProbe, LiveProcessProbe};

// ---------------------------------------------------------------------------
// git fixtures (env-cleared, PATH/HOME-only — mirrors production isolation)
// ---------------------------------------------------------------------------

fn git_cmd(cwd: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(cwd).env_clear();
    if let Ok(p) = std::env::var("PATH") {
        cmd.env("PATH", p);
    }
    if let Ok(h) = std::env::var("HOME") {
        cmd.env("HOME", h);
    }
    cmd
}

fn run_git(cwd: &Path, args: &[&str]) {
    let out = git_cmd(cwd, args).output().expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A normal (non-bare) repo with `main` and one seed commit.
fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    run_git(dir, &["init", "--initial-branch=main", "--quiet"]);
    run_git(dir, &["config", "user.email", "t@e.com"]);
    run_git(dir, &["config", "user.name", "t"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
    fs::write(dir.join("seed"), "x").unwrap();
    run_git(dir, &["add", "seed"]);
    run_git(dir, &["commit", "-m", "seed", "--quiet"]);
}

/// A bare repo usable as a push remote via its local filesystem path.
fn init_bare(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    run_git(dir, &["init", "--bare", "--initial-branch=main", "--quiet"]);
}

/// The `<state_root>/engineer-worktrees/` root, created on disk.
fn worktrees_root(state_root: &Path) -> PathBuf {
    let root = state_root.join(WORKTREES_SUBDIR);
    fs::create_dir_all(&root).unwrap();
    root
}

/// Add a worktree of `repo` at `dir` on a fresh `branch` off `main`.
fn add_worktree(repo: &Path, branch: &str, dir: &Path) {
    run_git(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            &dir.to_string_lossy(),
            "main",
        ],
    );
}

/// Write a dead-PID engineer claim (a crashed engineer's leftover). PID
/// 2_147_483_646 is virtually guaranteed not to be a live process.
fn write_dead_claim(dir: &Path) {
    fs::write(dir.join(ENGINEER_CLAIM_FILE), "2147483646\n").unwrap();
}

/// A probe that always answers "live" — models the fail-closed case where the
/// liveness check cannot authoritatively answer (non-Linux, `/proc` unreadable,
/// canonicalize failure). The sweep must treat this as "keep".
struct AlwaysLiveProbe;
impl LiveProcessProbe for AlwaysLiveProbe {
    fn worktree_has_live_process(&self, _dir: &Path) -> bool {
        true
    }
}

/// Build a bare remote + a normal repo wired to it (`origin`, `main` pushed).
/// Returned `TempDir`s must be kept alive for the duration of the test.
fn repo_with_remote() -> (TempDir, TempDir) {
    let remote = tempdir().unwrap();
    init_bare(remote.path());
    let repo = tempdir().unwrap();
    init_repo(repo.path());
    run_git(
        repo.path(),
        &["remote", "add", "origin", &remote.path().to_string_lossy()],
    );
    run_git(repo.path(), &["push", "-u", "origin", "main"]);
    (remote, repo)
}

// ===========================================================================
// LIVE_CWD guard (new #2553) — reuses worktree_gc::liveness
// ===========================================================================

#[test]
#[serial_test::serial]
fn sweep_skips_worktree_that_is_a_live_process_cwd() {
    // An unregistered orphan whose engineer-claim is DEAD (so the existing
    // LIVE_CLAIM guard does NOT fire) but which a live process is sitting in.
    // Only the new LIVE_CWD guard can save it.
    let parent = tempdir().unwrap();
    init_repo(parent.path());
    let state = tempdir().unwrap();
    let root = worktrees_root(state.path());

    let live = root.join("goal-live-cwd-dead-claim");
    fs::create_dir_all(&live).unwrap();
    write_dead_claim(&live);

    let probe = FakeLiveProcessProbe::default();
    probe.mark_live(&live); // a process has its CWD here

    let report = sweep_orphaned_worktrees_inner(parent.path(), state.path(), &probe)
        .expect("sweep must succeed");

    assert!(live.exists(), "live-CWD worktree must remain on disk");
    assert!(
        report.removed_orphan_dirs.is_empty(),
        "must not remove a live-CWD worktree; got {:?}",
        report.removed_orphan_dirs
    );
    assert!(
        report
            .skipped_live_cwd_dirs
            .iter()
            .any(|p| p.canonicalize().ok() == live.canonicalize().ok()),
        "must record the live-CWD skip; got {:?}",
        report.skipped_live_cwd_dirs
    );
}

#[test]
#[serial_test::serial]
fn sweep_fail_closed_liveness_probe_keeps_worktree() {
    // A dead-claim junk orphan that the old code would reap. A probe that
    // cannot answer (models `/proc` unreadable) reports "live" for every path;
    // the sweep must fail-closed and KEEP it.
    let parent = tempdir().unwrap();
    init_repo(parent.path());
    let state = tempdir().unwrap();
    let root = worktrees_root(state.path());

    let orphan = root.join("goal-fail-closed");
    fs::create_dir_all(&orphan).unwrap();
    write_dead_claim(&orphan);

    let report = sweep_orphaned_worktrees_inner(parent.path(), state.path(), &AlwaysLiveProbe)
        .expect("sweep must succeed");

    assert!(orphan.exists(), "fail-closed probe must keep the worktree");
    assert!(
        report.removed_orphan_dirs.is_empty(),
        "fail-closed liveness must prevent every removal; got {:?}",
        report.removed_orphan_dirs
    );
    assert!(
        report
            .skipped_live_cwd_dirs
            .iter()
            .any(|p| p.canonicalize().ok() == orphan.canonicalize().ok()),
        "fail-closed keep must be recorded as a live-CWD skip; got {:?}",
        report.skipped_live_cwd_dirs
    );
}

// ===========================================================================
// WORK_STATE guard (new #2553) — never destroy uncommitted / unpushed work
// ===========================================================================

#[test]
#[serial_test::serial]
fn sweep_keeps_orphan_worktree_with_uncommitted_changes() {
    // A REAL git worktree (owned by a different repo `other`, but physically
    // placed under this daemon's engineer-worktrees root) with a dirty working
    // tree. It is unregistered from `parent`, has no live claim and no live
    // CWD — the OLD sweep would `remove_dir_all` it and destroy the edits.
    // WORK_STATE must keep it.
    let parent = tempdir().unwrap();
    init_repo(parent.path());
    let other = tempdir().unwrap();
    init_repo(other.path());
    let state = tempdir().unwrap();
    let root = worktrees_root(state.path());

    let dirty = root.join("goal-dirty");
    add_worktree(other.path(), "engineer/goal-dirty", &dirty);
    // Uncommitted change: an untracked file → `git status --porcelain` non-empty.
    fs::write(dirty.join("wip.txt"), "unsaved work").unwrap();

    let probe = FakeLiveProcessProbe::default(); // nothing live
    let report = sweep_orphaned_worktrees_inner(parent.path(), state.path(), &probe)
        .expect("sweep must succeed");

    assert!(dirty.exists(), "dirty worktree must survive the sweep");
    assert!(
        report.removed_orphan_dirs.is_empty(),
        "must not remove a dirty worktree; got {:?}",
        report.removed_orphan_dirs
    );
    assert!(
        report
            .skipped_dirty_dirs
            .iter()
            .any(|p| p.canonicalize().ok() == dirty.canonicalize().ok()),
        "must record the uncommitted-work skip; got {:?}",
        report.skipped_dirty_dirs
    );
}

#[test]
#[serial_test::serial]
fn sweep_keeps_orphan_worktree_with_no_upstream() {
    // A clean real git worktree with NO upstream configured: we cannot prove
    // its commits were pushed, so it must be kept (fail-safe).
    let parent = tempdir().unwrap();
    init_repo(parent.path());
    let other = tempdir().unwrap();
    init_repo(other.path());
    let state = tempdir().unwrap();
    let root = worktrees_root(state.path());

    let no_upstream = root.join("goal-no-upstream");
    add_worktree(other.path(), "engineer/goal-no-upstream", &no_upstream);
    // No `push -u`, so `@{u}` does not resolve; tree is otherwise clean.

    let probe = FakeLiveProcessProbe::default();
    let report = sweep_orphaned_worktrees_inner(parent.path(), state.path(), &probe)
        .expect("sweep must succeed");

    assert!(no_upstream.exists(), "no-upstream worktree must be kept");
    assert!(
        report.removed_orphan_dirs.is_empty(),
        "must not remove a worktree whose pushed-state is unprovable; got {:?}",
        report.removed_orphan_dirs
    );
    assert!(
        report
            .skipped_dirty_dirs
            .iter()
            .any(|p| p.canonicalize().ok() == no_upstream.canonicalize().ok()),
        "no-upstream keep must be recorded as a work-state skip; got {:?}",
        report.skipped_dirty_dirs
    );
}

#[test]
#[serial_test::serial]
fn sweep_keeps_orphan_worktree_with_unpushed_commit() {
    // A clean worktree whose branch has an upstream but is AHEAD of it (an
    // unpushed commit). `git rev-list --count @{u}..HEAD` > 0 → keep.
    let (_remote, other) = repo_with_remote();
    let parent = tempdir().unwrap();
    init_repo(parent.path());
    let state = tempdir().unwrap();
    let root = worktrees_root(state.path());

    let ahead = root.join("goal-ahead");
    add_worktree(other.path(), "engineer/goal-ahead", &ahead);
    // Establish an upstream, then commit locally without pushing.
    run_git(&ahead, &["push", "-u", "origin", "engineer/goal-ahead"]);
    fs::write(ahead.join("committed-not-pushed"), "y").unwrap();
    run_git(&ahead, &["add", "committed-not-pushed"]);
    run_git(&ahead, &["commit", "-m", "local only", "--quiet"]);

    let probe = FakeLiveProcessProbe::default();
    let report = sweep_orphaned_worktrees_inner(parent.path(), state.path(), &probe)
        .expect("sweep must succeed");

    assert!(ahead.exists(), "worktree with unpushed commit must be kept");
    assert!(
        report.removed_orphan_dirs.is_empty(),
        "must not remove a worktree that is ahead of its upstream; got {:?}",
        report.removed_orphan_dirs
    );
    assert!(
        report
            .skipped_dirty_dirs
            .iter()
            .any(|p| p.canonicalize().ok() == ahead.canonicalize().ok()),
        "unpushed-commit keep must be recorded; got {:?}",
        report.skipped_dirty_dirs
    );
}

// ===========================================================================
// REAP — genuinely orphaned + dead/absent claim + clean + not live → removed,
//        with the observable RemovalReason recorded.
// ===========================================================================

#[test]
#[serial_test::serial]
fn sweep_reaps_dead_claim_orphan_and_records_reason() {
    // A crashed engineer's leftover: unregistered dir, no `.git` (nothing to
    // lose), a DEAD claim. It must be reaped and paired with a
    // RemovalReason::OrphanedNoLiveNoWork { had_dead_claim: true }.
    let parent = tempdir().unwrap();
    init_repo(parent.path());
    let state = tempdir().unwrap();
    let root = worktrees_root(state.path());

    let orphan = root.join("goal-dead-orphan");
    fs::create_dir_all(&orphan).unwrap();
    write_dead_claim(&orphan);

    let probe = FakeLiveProcessProbe::default(); // nothing live
    let report = sweep_orphaned_worktrees_inner(parent.path(), state.path(), &probe)
        .expect("sweep must succeed");

    assert!(!orphan.exists(), "dead-claim junk orphan must be removed");
    assert_eq!(
        report.removed_orphan_dirs.len(),
        1,
        "exactly one removal expected; got {:?}",
        report.removed_orphan_dirs
    );
    assert_eq!(
        report.removal_reasons.len(),
        report.removed_orphan_dirs.len(),
        "every removal must carry a reason (1:1 pairing)"
    );
    let (path, reason) = &report.removal_reasons[0];
    assert_eq!(
        path, &report.removed_orphan_dirs[0],
        "removal_reasons must be paired 1:1 with removed_orphan_dirs"
    );
    assert!(
        matches!(
            reason,
            RemovalReason::OrphanedNoLiveNoWork {
                had_dead_claim: true
            }
        ),
        "dead-claim orphan must record had_dead_claim=true; got {reason:?}"
    );
}

#[test]
#[serial_test::serial]
fn sweep_reaps_claimless_orphan_and_records_reason() {
    // A plain leftover directory: unregistered, no `.git`, NO claim at all.
    // Reaped with had_dead_claim=false.
    let parent = tempdir().unwrap();
    init_repo(parent.path());
    let state = tempdir().unwrap();
    let root = worktrees_root(state.path());

    let orphan = root.join("goal-claimless-orphan");
    fs::create_dir_all(&orphan).unwrap();
    fs::write(orphan.join("stale"), b"junk").unwrap();

    let probe = FakeLiveProcessProbe::default();
    let report = sweep_orphaned_worktrees_inner(parent.path(), state.path(), &probe)
        .expect("sweep must succeed");

    assert!(!orphan.exists(), "claimless junk orphan must be removed");
    let (path, reason) = report
        .removal_reasons
        .iter()
        .find(|(p, _)| p.canonicalize().ok() == orphan.canonicalize().ok())
        .expect("removed orphan must have a recorded reason");
    assert!(
        report.removed_orphan_dirs.contains(path),
        "reason path must also appear in removed_orphan_dirs"
    );
    assert!(
        matches!(
            reason,
            RemovalReason::OrphanedNoLiveNoWork {
                had_dead_claim: false
            }
        ),
        "claimless orphan must record had_dead_claim=false; got {reason:?}"
    );
}

#[test]
#[serial_test::serial]
fn sweep_reaps_clean_pushed_orphan_worktree() {
    // Proves WORK_STATE is not over-conservative: a REAL git worktree that is
    // clean AND fully pushed (upstream configured, not ahead) carries no
    // recoverable work and MUST still be reaped.
    let (_remote, other) = repo_with_remote();
    let parent = tempdir().unwrap();
    init_repo(parent.path());
    let state = tempdir().unwrap();
    let root = worktrees_root(state.path());

    let clean = root.join("goal-clean-pushed");
    add_worktree(other.path(), "engineer/goal-clean-pushed", &clean);
    run_git(
        &clean,
        &["push", "-u", "origin", "engineer/goal-clean-pushed"],
    );

    let probe = FakeLiveProcessProbe::default();
    let report = sweep_orphaned_worktrees_inner(parent.path(), state.path(), &probe)
        .expect("sweep must succeed");

    assert!(
        !clean.exists(),
        "clean, fully-pushed orphan worktree must be reaped"
    );
    assert!(
        report
            .removed_orphan_dirs
            .iter()
            .any(|p| p.canonicalize().ok() == clean.canonicalize().ok()),
        "clean+pushed worktree must be recorded as removed; got {:?}",
        report.removed_orphan_dirs
    );
    assert!(
        report.skipped_dirty_dirs.is_empty(),
        "clean+pushed worktree must NOT be treated as having work; got {:?}",
        report.skipped_dirty_dirs
    );
}

// ===========================================================================
// SCOPE — the sweep only ever operates under <state_root>/engineer-worktrees/.
// ===========================================================================

#[test]
#[serial_test::serial]
fn sweep_does_not_touch_dirs_outside_engineer_worktrees_root() {
    let parent = tempdir().unwrap();
    init_repo(parent.path());
    let state = tempdir().unwrap();
    let root = worktrees_root(state.path());

    // A sibling directory under state_root but OUTSIDE engineer-worktrees/.
    let sibling = state.path().join("not-engineer-worktrees");
    fs::create_dir_all(&sibling).unwrap();
    fs::write(sibling.join("precious"), b"keep me").unwrap();

    // A genuine junk orphan inside the root to prove the sweep still ran.
    let orphan = root.join("goal-in-scope-orphan");
    fs::create_dir_all(&orphan).unwrap();

    let probe = FakeLiveProcessProbe::default();
    let report = sweep_orphaned_worktrees_inner(parent.path(), state.path(), &probe)
        .expect("sweep must succeed");

    assert!(
        sibling.exists() && sibling.join("precious").exists(),
        "a dir outside engineer-worktrees/ must never be touched"
    );
    assert!(
        !report
            .removed_orphan_dirs
            .iter()
            .any(|p| p.canonicalize().ok() == sibling.canonicalize().ok()),
        "out-of-scope dir must never be recorded as removed"
    );
    assert!(
        !orphan.exists(),
        "in-scope junk orphan should still be reaped"
    );
}

// ===========================================================================
// Public wrapper delegates to the inner core (production ProcfsLiveProcessProbe).
// ===========================================================================

#[test]
#[serial_test::serial]
fn public_sweep_wrapper_reaps_junk_orphan() {
    let parent = tempdir().unwrap();
    init_repo(parent.path());
    let state = tempdir().unwrap();
    let root = worktrees_root(state.path());

    let orphan = root.join("goal-public-wrapper");
    fs::create_dir_all(&orphan).unwrap();
    write_dead_claim(&orphan);

    // No live process has its CWD inside a fresh tempdir, so the production
    // probe reports "not live" and the junk orphan is reaped.
    let report = sweep_orphaned_worktrees(parent.path(), state.path()).expect("sweep must succeed");

    assert!(!orphan.exists(), "public wrapper must reap the junk orphan");
    assert!(
        report.removed_orphan_dirs.iter().any(|p| p == &orphan),
        "public wrapper must record the removal; got {:?}",
        report.removed_orphan_dirs
    );
}
