//! Issue #2553 — outside-in reproduction of the verified worktree-sweep
//! data-loss incident, driven through the crate's PUBLIC API.
//!
//! This is the daemon's-eye view: it calls the exact function the OODA daemon
//! calls on boot and on its periodic timer —
//! [`simard::engineer_worktree::sweep_orphaned_worktrees`] — with the REAL
//! `/proc`-backed liveness probe (not a test double). Where the in-crate unit
//! tests inject a `FakeLiveProcessProbe`, this integration test spawns a REAL
//! child process sitting in an orphan directory so the production
//! `ProcfsLiveProcessProbe` must observe it through `/proc/<pid>/cwd`.
//!
//! The incident (issue #2553): the periodic sweep deleted worktrees out from
//! under active operations — an operator's warm build-target checkout under
//! `~/src/Simard/worktrees/` and an IN-USE worktree removed mid-`cargo build`.
//! This test reproduces that exact shape with temp dirs only (it never touches
//! `~/.simard` or `~/src/Simard/worktrees`) and asserts the three headline
//! guards hold end-to-end:
//!
//! * SCOPE — a directory OUTSIDE `<state_root>/engineer-worktrees/` (standing
//!   in for the operator's own checkout) is never enumerated, so it survives.
//! * LIVE_CWD — an orphan that is the CWD of a real live process survives.
//! * REAP — a genuinely orphaned, dead, work-free directory is reaped, and the
//!   removal is recorded with an observable reason.
//!
//! No sleeps, no network. The live process is a real child we spawn and kill;
//! `/proc/<pid>/cwd` is populated at clone time, so the probe sees it without
//! any timing wait.

use std::fs;
use std::path::Path;
use std::process::{Child, Command};

use simard::engineer_worktree::{RemovalReason, WORKTREES_SUBDIR, sweep_orphaned_worktrees};
use tempfile::tempdir;

/// Run `git` in `cwd` with a cleared environment (only `PATH`/`HOME`
/// re-injected), mirroring the production `git_capture` isolation so this
/// test cannot be perturbed by the caller's `GIT_*` env.
fn run_git(cwd: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(cwd).env_clear();
    if let Ok(p) = std::env::var("PATH") {
        cmd.env("PATH", p);
    }
    if let Ok(h) = std::env::var("HOME") {
        cmd.env("HOME", h);
    }
    let out = cmd.output().expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// A minimal non-bare repo with a single seed commit on `main`. The sweep runs
/// `git worktree prune` / `git worktree list` against this parent.
fn init_parent_repo(dir: &Path) {
    run_git(dir, &["init", "--initial-branch=main", "--quiet"]);
    run_git(dir, &["config", "user.email", "t@e.com"]);
    run_git(dir, &["config", "user.name", "t"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
    fs::write(dir.join("seed"), "x").unwrap();
    run_git(dir, &["add", "seed"]);
    run_git(dir, &["commit", "-m", "seed", "--quiet"]);
}

/// Kills its wrapped child on drop so a panic mid-test never leaks a process.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawn a long-lived child whose current working directory is `dir`, so the
/// production `/proc` liveness probe observes `dir` as live. Returns a guard
/// that terminates the child when dropped.
fn spawn_live_process_in(dir: &Path) -> ChildGuard {
    let child = Command::new("sleep")
        .arg("120")
        .current_dir(dir)
        .spawn()
        .expect("spawn live child (`sleep`) in orphan worktree");
    ChildGuard(child)
}

#[test]
fn daemon_sweep_reproduces_incident_and_holds_all_guards() {
    // The parent repo the daemon sweeps against (isolated temp git repo).
    let parent = tempdir().unwrap();
    init_parent_repo(parent.path());

    // The supervisor state root; the sweep only ever scans its
    // `engineer-worktrees/` subdir.
    let state = tempdir().unwrap();
    let eng_root = state.path().join(WORKTREES_SUBDIR);
    fs::create_dir_all(&eng_root).unwrap();

    // (1) A genuinely orphaned, dead, work-free directory: not a git worktree,
    //     no engineer-claim, no live process. This is the ONLY thing the sweep
    //     is allowed to reap.
    let genuine_orphan = eng_root.join("goal-abandoned-dead");
    fs::create_dir_all(&genuine_orphan).unwrap();
    fs::write(genuine_orphan.join("leftover.txt"), b"junk").unwrap();

    // (2) An IN-USE orphan: same shape as (1) but a real process is sitting in
    //     it, exactly like the worktree removed mid-`cargo build` in the
    //     incident. Canonicalize first so both the child's `/proc` cwd link and
    //     the probe's canonicalized target agree.
    let in_use = eng_root.join("goal-mid-build-inuse");
    fs::create_dir_all(&in_use).unwrap();
    let in_use = in_use.canonicalize().unwrap();
    let _live = spawn_live_process_in(&in_use);

    // (3) The operator's own checkout, OUTSIDE the engineer-worktrees root
    //     (stands in for `~/src/Simard/worktrees/meeting-ux-762`). The sweep
    //     must never enumerate — let alone delete — anything here.
    let operator_checkout = tempdir().unwrap();
    let operator_worktree = operator_checkout.path().join("meeting-ux-762");
    fs::create_dir_all(&operator_worktree).unwrap();
    fs::write(operator_worktree.join("Cargo.toml"), b"[package]").unwrap();

    // Drive the EXACT production entry point the daemon uses (real `/proc`
    // probe inside).
    let report = sweep_orphaned_worktrees(parent.path(), state.path())
        .expect("daemon sweep entry point should succeed");

    // SCOPE: the operator's out-of-scope checkout is untouched.
    assert!(
        operator_worktree.exists(),
        "SCOPE guard breached: operator checkout outside engineer-worktrees was removed",
    );

    // LIVE_CWD: the in-use worktree with a real live process survives and is
    // recorded as a live-CWD skip.
    assert!(
        in_use.exists(),
        "LIVE_CWD guard breached: an in-use worktree (live process CWD) was reaped",
    );
    assert!(
        report
            .skipped_live_cwd_dirs
            .iter()
            .any(|p| p.canonicalize().ok() == in_use.canonicalize().ok()),
        "expected the in-use worktree in skipped_live_cwd_dirs; got {:?}",
        report.skipped_live_cwd_dirs,
    );

    // REAP: only the genuine orphan is gone, and the removal carries an
    // observable, attributable reason.
    assert!(
        !genuine_orphan.exists(),
        "genuine dead orphan should have been reaped",
    );
    assert!(
        report
            .removed_orphan_dirs
            .iter()
            .any(|p| p.file_name() == genuine_orphan.file_name()),
        "expected the genuine orphan in removed_orphan_dirs; got {:?}",
        report.removed_orphan_dirs,
    );
    let reason_recorded = report.removal_reasons.iter().any(|(p, reason)| {
        p.file_name() == genuine_orphan.file_name()
            && matches!(reason, RemovalReason::OrphanedNoLiveNoWork { .. })
    });
    assert!(
        reason_recorded,
        "every removal must record an observable RemovalReason; got {:?}",
        report.removal_reasons,
    );

    // Exactly one directory was reaped — the sweep is not deleting broadly.
    assert_eq!(
        report.removed_orphan_dirs.len(),
        1,
        "sweep must reap ONLY the single genuine orphan; got {:?}",
        report.removed_orphan_dirs,
    );
}
