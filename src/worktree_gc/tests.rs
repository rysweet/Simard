//! Integration tests for the worktree GC.
//!
//! Exercises [`run_gc`] with a real `git worktree list` against a tempdir
//! repo and a fake [`GhClient`] so the policy + parser + runner glue can
//! be tested without touching the real GitHub API.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::SystemTime;

use tempfile::tempdir;

use super::policy::{CandidateInputs, PruneReason, evaluate_candidate};
use super::runner::{GhClient, render_reason, run_gc};
use super::{GcConfig, parse_worktree_list, under_any_root};

// ------------------------------------------------------------------
// Fake GhClient
// ------------------------------------------------------------------

#[derive(Default)]
struct FakeGh {
    /// branch → merged PR numbers
    merged: Mutex<HashMap<String, Vec<u32>>>,
    /// branch → presence on origin (None means "inconclusive")
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

// ------------------------------------------------------------------
// Repo fixtures
// ------------------------------------------------------------------

fn run_git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git spawn");
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        repo.display(),
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

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[test]
fn dry_run_lists_merged_branch_but_does_not_remove() {
    let parent_tmp = tempdir().unwrap();
    let parent = parent_tmp.path();
    init_repo(parent);

    let roots_tmp = tempdir().unwrap();
    let wt_dir = roots_tmp.path().join("eng-1");
    add_worktree(parent, "engineer/eng-1", &wt_dir);

    let gh = FakeGh::default();
    gh.merged
        .lock()
        .unwrap()
        .insert("engineer/eng-1".to_string(), vec![42]);

    let cfg = GcConfig {
        roots: vec![roots_tmp.path().to_path_buf()],
        parent_repo: parent.to_path_buf(),
        apply: false,
        idle_days: 7,
        now: SystemTime::now(),
    };
    let report = run_gc(&cfg, &gh).expect("gc");

    assert_eq!(report.candidates.len(), 1, "{report:?}");
    assert!(matches!(
        report.candidates[0].reasons[0],
        PruneReason::BranchMerged { .. }
    ));
    assert!(report.pruned.is_empty(), "dry-run must not prune");
    assert!(wt_dir.exists(), "dry-run must leave worktree on disk");
}

#[test]
fn apply_prunes_merged_worktree_and_removes_dir() {
    let parent_tmp = tempdir().unwrap();
    let parent = parent_tmp.path();
    init_repo(parent);

    let roots_tmp = tempdir().unwrap();
    let wt_dir = roots_tmp.path().join("eng-merged");
    add_worktree(parent, "engineer/eng-merged", &wt_dir);

    let gh = FakeGh::default();
    gh.merged
        .lock()
        .unwrap()
        .insert("engineer/eng-merged".to_string(), vec![777]);

    let cfg = GcConfig {
        roots: vec![roots_tmp.path().to_path_buf()],
        parent_repo: parent.to_path_buf(),
        apply: true,
        idle_days: 7,
        now: SystemTime::now(),
    };
    let report = run_gc(&cfg, &gh).expect("gc apply");

    assert_eq!(report.pruned.len(), 1);
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(!wt_dir.exists(), "worktree dir must be removed");
}

#[test]
fn worktree_outside_configured_roots_is_skipped() {
    let parent_tmp = tempdir().unwrap();
    let parent = parent_tmp.path();
    init_repo(parent);

    // Worktree placed elsewhere; configured root is the empty subdir.
    let elsewhere = tempdir().unwrap();
    let wt_dir = elsewhere.path().join("eng-outside");
    add_worktree(parent, "engineer/eng-outside", &wt_dir);

    let configured_root = tempdir().unwrap();

    let gh = FakeGh::default();
    gh.merged
        .lock()
        .unwrap()
        .insert("engineer/eng-outside".to_string(), vec![1]);

    let cfg = GcConfig {
        roots: vec![configured_root.path().to_path_buf()],
        parent_repo: parent.to_path_buf(),
        apply: true,
        idle_days: 7,
        now: SystemTime::now(),
    };
    let report = run_gc(&cfg, &gh).expect("gc apply");

    assert_eq!(report.worktrees_examined, 0);
    assert!(report.pruned.is_empty());
    assert!(wt_dir.exists(), "outside-root worktree must not be touched");
}

#[test]
fn fresh_unmerged_worktree_is_not_pruned() {
    let parent_tmp = tempdir().unwrap();
    let parent = parent_tmp.path();
    init_repo(parent);

    let roots_tmp = tempdir().unwrap();
    let wt_dir = roots_tmp.path().join("eng-fresh");
    add_worktree(parent, "engineer/eng-fresh", &wt_dir);

    let gh = FakeGh::default(); // empty: no merged PRs, branch on origin

    let cfg = GcConfig {
        roots: vec![roots_tmp.path().to_path_buf()],
        parent_repo: parent.to_path_buf(),
        apply: true,
        idle_days: 7,
        now: SystemTime::now(),
    };
    let report = run_gc(&cfg, &gh).expect("gc apply");

    assert_eq!(report.worktrees_examined, 1);
    assert_eq!(report.candidates.len(), 0, "{report:?}");
    assert!(report.pruned.is_empty());
    assert!(wt_dir.exists());
}

#[test]
fn deleted_origin_branch_is_pruned() {
    let parent_tmp = tempdir().unwrap();
    let parent = parent_tmp.path();
    init_repo(parent);

    let roots_tmp = tempdir().unwrap();
    let wt_dir = roots_tmp.path().join("eng-gone");
    add_worktree(parent, "engineer/eng-gone", &wt_dir);

    let gh = FakeGh::default();
    gh.on_origin
        .lock()
        .unwrap()
        .insert("engineer/eng-gone".to_string(), Some(false));

    let cfg = GcConfig {
        roots: vec![roots_tmp.path().to_path_buf()],
        parent_repo: parent.to_path_buf(),
        apply: false,
        idle_days: 7,
        now: SystemTime::now(),
    };
    let report = run_gc(&cfg, &gh).expect("gc dry");

    assert_eq!(report.candidates.len(), 1);
    assert!(matches!(
        report.candidates[0].reasons[0],
        PruneReason::BranchDeletedFromOrigin
    ));
}

#[test]
fn parser_then_policy_round_trip() {
    // Synthesize a porcelain block by hand and feed it through both
    // pieces — the runner does this same composition on real `git
    // worktree list` output.
    let raw = "\
worktree /repo
bare

worktree /tmp/wt
HEAD abc
branch refs/heads/feat/x
";
    let entries = parse_worktree_list(raw);
    assert_eq!(entries.len(), 2);

    let inputs = CandidateInputs {
        merged_prs: vec![1],
        branch_on_origin: Some(true),
        last_activity: Some(SystemTime::now()),
    };
    assert!(evaluate_candidate(&entries[0], &inputs, SystemTime::now(), 7).is_none());
    assert!(evaluate_candidate(&entries[1], &inputs, SystemTime::now(), 7).is_some());
}

#[test]
fn render_reason_strings_are_human_readable() {
    let s = render_reason(&PruneReason::BranchMerged {
        pr_numbers: vec![42, 43],
    });
    assert_eq!(s, "branch merged (#42, #43)");
    assert_eq!(
        render_reason(&PruneReason::BranchDeletedFromOrigin),
        "branch deleted from origin"
    );
    assert_eq!(
        render_reason(&PruneReason::IdleTooLong { age_days: 14 }),
        "idle 14d"
    );
}

#[test]
fn under_any_root_handles_missing_root_gracefully() {
    let real = tempdir().unwrap();
    let inside = real.path().join("child");
    std::fs::create_dir_all(&inside).unwrap();

    let roots = vec![
        PathBuf::from("/nonexistent/root/does/not/exist"),
        real.path().to_path_buf(),
    ];
    assert!(under_any_root(&inside, &roots));
    assert!(!under_any_root(Path::new("/etc"), &roots));
}
