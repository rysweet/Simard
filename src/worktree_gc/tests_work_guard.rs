//! Issue #2553 — operator GC uncommitted/unpushed-work guard (RED phase, TDD).
//!
//! Specifies the contract in `docs/reference/engineer-worktree-sweep-safety.md`
//! for the `simard worktree-gc` path (`src/worktree_gc/`). This is the incident's
//! actual deletion vector: `default_roots()` includes `<HOME>/src/Simard/worktrees`,
//! and `--apply` prunes candidates whose branch is merged / deleted / idle.
//! `worktree_gc` already declines to prune a live-CWD worktree, but had **no**
//! signal for uncommitted or unpushed work, so `--apply` could destroy an
//! in-use operator worktree carrying unsaved edits.
//!
//! These tests MUST fail before the fix lands (`CandidateInputs` has no
//! `has_uncommitted_or_unpushed_work` field, and `gather_inputs` does not
//! compute it) and MUST pass afterwards without further edits.
//!
//! Offline, serial, sleep-free, network-free.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use tempfile::tempdir;

use super::GcConfig;
use super::liveness::FakeLiveProcessProbe;
use super::parse::parse_worktree_list;
use super::policy::{CandidateInputs, PruneReason, evaluate_candidate};
use super::runner::{GhClient, run_gc};

// ---------------------------------------------------------------------------
// Fake GhClient (deterministic merged / on-origin answers)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeGh {
    merged: Mutex<HashMap<String, Vec<u32>>>,
    on_origin: Mutex<HashMap<String, Option<bool>>>,
}

impl GhClient for FakeGh {
    fn merged_prs_for_branch(&self, branch: &str) -> Result<Vec<u32>, String> {
        Ok(self
            .merged
            .lock()
            .unwrap()
            .get(branch)
            .cloned()
            .unwrap_or_default())
    }
    fn branch_exists_on_remote(&self, _remote: &str, branch: &str) -> Result<Option<bool>, String> {
        Ok(self
            .on_origin
            .lock()
            .unwrap()
            .get(branch)
            .copied()
            .unwrap_or(Some(true)))
    }
}

// ---------------------------------------------------------------------------
// git fixtures
// ---------------------------------------------------------------------------

fn run_git(cwd: &Path, args: &[&str]) {
    let out = crate::util::spawn_retry::retry_spawn_sync(|| {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(cwd).env_clear();
        if let Ok(p) = std::env::var("PATH") {
            cmd.env("PATH", p);
        }
        if let Ok(h) = std::env::var("HOME") {
            cmd.env("HOME", h);
        }
        cmd.output()
    })
    .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    run_git(dir, &["init", "--initial-branch=main", "--quiet"]);
    run_git(dir, &["config", "user.email", "t@e.com"]);
    run_git(dir, &["config", "user.name", "t"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("seed"), "x").unwrap();
    run_git(dir, &["add", "seed"]);
    run_git(dir, &["commit", "-m", "seed", "--quiet"]);
}

fn add_worktree(parent: &Path, branch: &str, dir: &Path) {
    run_git(
        parent,
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

// ===========================================================================
// Policy-level: the new signal overrides every prune reason (fail-safe keep).
// ===========================================================================

#[test]
fn uncommitted_or_unpushed_work_blocks_prune_even_with_all_signals() {
    let raw = "\
worktree /tmp/wt
HEAD abcabcabcabcabcabcabcabcabcabcabcabcabca
branch refs/heads/engineer/x
";
    let entries = parse_worktree_list(raw);
    let now = SystemTime::now();

    // Every prune signal is hot: merged, deleted-from-origin, and long idle.
    let inputs = CandidateInputs {
        merged_prs: vec![1, 2, 3],
        branch_on_origin: Some(false),
        last_activity: Some(now - Duration::from_secs(365 * 24 * 3600)),
        has_live_process: false,
        has_uncommitted_or_unpushed_work: true,
    };

    assert!(
        evaluate_candidate(&entries[0], &inputs, now, 7).is_none(),
        "uncommitted/unpushed work must override merged + deleted + idle signals",
    );
}

#[test]
fn work_guard_is_additive_clean_merged_branch_still_a_candidate() {
    // With the new field FALSE, existing behavior is unchanged: a clean,
    // merged branch remains a prune candidate.
    let raw = "\
worktree /tmp/wt
HEAD abcabcabcabcabcabcabcabcabcabcabcabcabca
branch refs/heads/engineer/x
";
    let entries = parse_worktree_list(raw);
    let now = SystemTime::now();

    let inputs = CandidateInputs {
        merged_prs: vec![42],
        branch_on_origin: Some(true),
        last_activity: Some(now),
        has_live_process: false,
        has_uncommitted_or_unpushed_work: false,
    };

    let cand = evaluate_candidate(&entries[0], &inputs, now, 7)
        .expect("clean merged branch must still be a candidate when the guard is off");
    assert!(
        matches!(cand.reasons[0], PruneReason::BranchMerged { .. }),
        "reason should still be BranchMerged; got {:?}",
        cand.reasons
    );
}

// ===========================================================================
// Integration: gather_inputs computes the guard from real `git` state, so
// `run_gc --apply` refuses to prune a dirty worktree even when its branch is
// merged. This is a behavioral test — it compiles against today's API and
// fails against today's behavior (the dirty worktree is pruned).
// ===========================================================================

#[test]
fn run_gc_does_not_prune_dirty_merged_worktree() {
    let parent_tmp = tempdir().unwrap();
    let parent = parent_tmp.path();
    init_repo(parent);

    let roots_tmp = tempdir().unwrap();
    let wt_dir = roots_tmp.path().join("eng-dirty");
    add_worktree(parent, "engineer/eng-dirty", &wt_dir);

    // Uncommitted change inside the worktree → `git status --porcelain` non-empty.
    std::fs::write(wt_dir.join("uncommitted.txt"), "work in progress").unwrap();

    // Branch is merged: without the work-guard this would be pruned.
    let gh = FakeGh::default();
    gh.merged
        .lock()
        .unwrap()
        .insert("engineer/eng-dirty".to_string(), vec![777]);

    let cfg = GcConfig {
        roots: vec![roots_tmp.path().to_path_buf()],
        parent_repo: parent.to_path_buf(),
        apply: true,
        idle_days: 7,
        now: SystemTime::now(),
    };
    let probe = FakeLiveProcessProbe::default(); // nothing live
    let report = run_gc(&cfg, &gh, &probe).expect("gc apply");

    assert!(
        report.candidates.is_empty(),
        "a dirty worktree must not be a prune candidate even when merged: {report:?}",
    );
    assert!(
        report.pruned.is_empty(),
        "dirty worktree must not be pruned under --apply: {report:?}",
    );
    assert!(
        wt_dir.exists(),
        "dirty merged worktree must survive `worktree-gc --apply`",
    );
}

#[test]
fn run_gc_still_prunes_clean_merged_worktree() {
    // Guard is additive: a CLEAN merged worktree is still pruned. Guards the
    // fix against becoming over-conservative and disabling GC entirely.
    let parent_tmp = tempdir().unwrap();
    let parent = parent_tmp.path();
    init_repo(parent);

    let roots_tmp = tempdir().unwrap();
    let wt_dir = roots_tmp.path().join("eng-clean");
    add_worktree(parent, "engineer/eng-clean", &wt_dir);

    let gh = FakeGh::default();
    gh.merged
        .lock()
        .unwrap()
        .insert("engineer/eng-clean".to_string(), vec![778]);

    let cfg = GcConfig {
        roots: vec![roots_tmp.path().to_path_buf()],
        parent_repo: parent.to_path_buf(),
        apply: true,
        idle_days: 7,
        now: SystemTime::now(),
    };
    let probe = FakeLiveProcessProbe::default();
    let report = run_gc(&cfg, &gh, &probe).expect("gc apply");

    assert_eq!(
        report.pruned.len(),
        1,
        "a clean merged worktree must still be pruned: {report:?}",
    );
    assert!(
        !wt_dir.exists(),
        "clean merged worktree dir must be removed"
    );
}
