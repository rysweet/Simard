//! Regression tests for issue #2621: `inspect_workspace` must filter the
//! Simard-managed `.simard-engineer-claim` sentinel out of `changed_files`
//! and `worktree_dirty`, so the engineer-loop pre-mutation guard never trips
//! on the untracked sentinel in a target repo that doesn't gitignore it.

use std::path::Path;
use std::process::Command;

use serial_test::serial;
use tempfile::tempdir;

use super::types::RepoInspection;
use super::{inspect_workspace, strip_claim_sentinel, verify_agent_spawn_artifacts};
use crate::engineer_worktree::ENGINEER_CLAIM_FILE;

fn git(repo: &Path, args: &[&str]) {
    let out = crate::util::spawn_retry::retry_spawn_sync(|| {
        Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
    })
    .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed in {}: {}",
        args,
        repo.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Initialise a git repo that, like an external governed repo, does NOT
/// gitignore Simard's private `.simard-engineer-claim` sentinel.
fn init_repo(dir: &Path) {
    git(dir, &["init", "--initial-branch=main", "--quiet"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("README.md"), "seed\n").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-m", "seed", "--quiet"]);
}

#[test]
#[serial(cognitive_memory)]
fn inspect_workspace_treats_claim_only_worktree_as_clean() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    let state_root = dir.path().join("state");
    std::fs::create_dir_all(&state_root).unwrap();

    // The ONLY change is the untracked Simard sentinel — the exact repro from
    // issue #2621 (`?? .simard-engineer-claim`).
    std::fs::write(
        dir.path()
            .join(crate::engineer_worktree::ENGINEER_CLAIM_FILE),
        format!("{}\n", std::process::id()),
    )
    .unwrap();

    let inspection = inspect_workspace(dir.path(), &state_root).expect("inspect_workspace");

    assert!(
        !inspection.worktree_dirty,
        "worktree containing only the claim sentinel must report clean; changed_files={:?}",
        inspection.changed_files
    );
    assert!(
        inspection.changed_files.is_empty(),
        "claim sentinel must be filtered out of changed_files; got {:?}",
        inspection.changed_files
    );
}

#[test]
#[serial(cognitive_memory)]
fn inspect_workspace_still_flags_real_changes_alongside_claim() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());
    let state_root = dir.path().join("state");
    std::fs::create_dir_all(&state_root).unwrap();

    // A genuine user change plus the sentinel: the sentinel must be filtered
    // but the real change must still mark the tree dirty. Guards against an
    // over-broad filter that would swallow real changes.
    std::fs::write(
        dir.path()
            .join(crate::engineer_worktree::ENGINEER_CLAIM_FILE),
        format!("{}\n", std::process::id()),
    )
    .unwrap();
    std::fs::write(dir.path().join("user_change.txt"), "real edit\n").unwrap();

    let inspection = inspect_workspace(dir.path(), &state_root).expect("inspect_workspace");

    assert!(
        inspection.worktree_dirty,
        "a genuine change must still mark the worktree dirty"
    );
    assert_eq!(
        inspection.changed_files,
        vec!["user_change.txt".to_string()],
        "only the real change must be reported; the sentinel must be filtered"
    );
}

/// `strip_claim_sentinel` is the shared filter used by BOTH `git status`
/// consumers (`inspect_workspace` and `verify_agent_spawn_artifacts`). It must
/// remove exactly the root sentinel and preserve every other path in order.
#[test]
fn strip_claim_sentinel_removes_only_the_root_sentinel() {
    let input = vec![
        "src/main.rs".to_string(),
        ENGINEER_CLAIM_FILE.to_string(),
        "Cargo.toml".to_string(),
    ];
    assert_eq!(
        strip_claim_sentinel(input),
        vec!["src/main.rs".to_string(), "Cargo.toml".to_string()],
        "only the claim sentinel is removed; all other paths are preserved in order"
    );
}

/// The filter is an exact root-path match, so a genuine change in a
/// subdirectory that merely shares the sentinel's basename is NOT swallowed.
#[test]
fn strip_claim_sentinel_keeps_subdir_same_basename() {
    let nested = format!("subdir/{ENGINEER_CLAIM_FILE}");
    assert_eq!(
        strip_claim_sentinel(vec![nested.clone(), ENGINEER_CLAIM_FILE.to_string()]),
        vec![nested],
        "a same-basename file under a subdirectory is a real change and must be kept"
    );
}

/// Build a `RepoInspection` pointed at `repo` with an empty (already-filtered)
/// `changed_files` baseline and `head == "HEAD"` so `HEAD..HEAD` yields zero new
/// commits — isolating `verify_agent_spawn_artifacts` to its `git status` /
/// sentinel-filter behavior.
fn inspection_for(repo: &Path) -> RepoInspection {
    RepoInspection {
        workspace_root: repo.to_path_buf(),
        repo_root: repo.to_path_buf(),
        branch: "main".to_string(),
        head: "HEAD".to_string(),
        worktree_dirty: false,
        changed_files: vec![],
        active_goals: vec![],
        carried_meeting_decisions: vec![],
        architecture_gap_summary: String::new(),
    }
}

/// Issue #2621 (second consumer): `verify_agent_spawn_artifacts` must also strip
/// the sentinel. This exercises the DEGRADED path — the sentinel is present on
/// disk with NO `.git/info/exclude` entry (i.e. the allocation-time append
/// failed), so raw `git status` lists it. Without the filter, the sentinel is a
/// "new changed file" (`post_status \ inspection.changed_files`) and a genuine
/// no-op agent session is falsely reported as `"verified"`. A regression that
/// deleted `strip_claim_sentinel(...)` from the post-status path would compile
/// and pass every other test in the suite — this is the test that catches it.
#[test]
fn verify_agent_spawn_artifacts_ignores_claim_only_no_op_session() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());

    // ONLY the untracked Simard sentinel, no exclude entry (degraded path).
    std::fs::write(
        dir.path().join(ENGINEER_CLAIM_FILE),
        format!("{}\n", std::process::id()),
    )
    .unwrap();

    // Objective deliberately references no issue/PR number so the best-effort
    // `gh` probe is skipped and the test stays hermetic.
    let report =
        verify_agent_spawn_artifacts(&inspection_for(dir.path()), "no-op engineer session");

    assert_eq!(
        report.status, "unverified",
        "a session whose only side-effect is the claim sentinel must NOT be reported verified; summary={}",
        report.summary
    );
    assert!(
        !report.summary.contains(ENGINEER_CLAIM_FILE),
        "the claim sentinel must never surface in the verification summary; got: {}",
        report.summary
    );
}

/// Control for the filter above: a GENUINE agent-created file alongside the
/// sentinel must still flip the report to `"verified"` (and only the real file
/// appears in the evidence). Guards against an over-broad filter that would
/// swallow real work and hide a productive session.
#[test]
fn verify_agent_spawn_artifacts_still_verifies_real_change_alongside_claim() {
    let dir = tempdir().unwrap();
    init_repo(dir.path());

    std::fs::write(
        dir.path().join(ENGINEER_CLAIM_FILE),
        format!("{}\n", std::process::id()),
    )
    .unwrap();
    std::fs::write(dir.path().join("agent_output.txt"), "real work\n").unwrap();

    let report =
        verify_agent_spawn_artifacts(&inspection_for(dir.path()), "no-op engineer session");

    assert_eq!(
        report.status, "verified",
        "a genuine new file must still verify the session; summary={}",
        report.summary
    );
    assert!(
        report.summary.contains("agent_output.txt"),
        "the real change must appear in the verification evidence; got: {}",
        report.summary
    );
    assert!(
        !report.summary.contains(ENGINEER_CLAIM_FILE),
        "the claim sentinel must be filtered even when a real change is present; got: {}",
        report.summary
    );
}
