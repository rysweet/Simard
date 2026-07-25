//! File-footprint FACTS for the dependency/overlap-aware engineer admission
//! gate (issue #2690).
//!
//! This module supplies the *facts only* — the set of files a live engineer is
//! touching, and the intersection of that set with a candidate goal's predicted
//! scope. It contains NO scheduling policy: the reasoning lives in the
//! hot-reloadable admission recipe, and the one load-bearing certain-collision
//! control (the exact-path `is_subset` rail) lives in the seam
//! (`admission.rs`). Keeping the facts here, pure and absent-tolerant, is what
//! lets the seam stay a thin integration + rail over a brain call.
//!
//! **Absent-tolerant by construction.** Every git shell-out degrades to an
//! EMPTY set on any error (no repo, detached HEAD, missing base, git absent).
//! An empty changed-file set means "no overlap knowable" ⇒ the gate fails
//! **open** (admit). Never panics, never blocks, never shells out under the
//! OODA state lock (the caller invokes this off-lock).

use std::path::Path;
use std::process::Command;

/// Default base ref the merge-base is computed against when a caller does not
/// know the goal's target branch. `origin/main` is Simard's trunk; a repo
/// without it simply yields an empty committed-diff (working-tree diff still
/// contributes), keeping the function absent-tolerant.
pub const DEFAULT_BASE_BRANCH: &str = "origin/main";

/// Files a live engineer is touching in its worktree: the committed diff since
/// the merge-base with `base_branch` PLUS the uncommitted working-tree diff,
/// unioned and normalized to repo-relative POSIX paths (sorted, de-duplicated).
///
/// Absent-tolerant: any git error (no repo, detached HEAD, missing base,
/// `git` not installed) yields an EMPTY set — an empty set means "no overlap
/// knowable" ⇒ admit (fail-open). Never panics, never blocks.
pub fn changed_files(worktree: &Path, base_branch: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();

    // Committed diff since the merge-base with the base branch. Using the
    // three-dot form `<base>...HEAD` diffs against the merge-base, so commits
    // that landed on the base branch after this worktree forked are not
    // reported as this engineer's changes.
    if let Some(base) = merge_base(worktree, base_branch) {
        files.extend(git_name_only(worktree, &[&format!("{base}...HEAD")]));
    }

    // Uncommitted working-tree changes (staged + unstaged). `git diff
    // --name-only HEAD` covers both against the current commit.
    files.extend(git_name_only(worktree, &["HEAD"]));
    // Also plain `git diff --name-only` (unstaged only) as a belt-and-suspenders
    // catch on repos where HEAD is unborn.
    files.extend(git_name_only(worktree, &[]));

    normalize_set(files)
}

/// Intersection of a candidate's predicted scope with an engineer's changed
/// files (repo-relative POSIX exact match). Non-empty ⇒ overlap. The result is
/// sorted + de-duplicated so callers get a stable, comparable set.
pub fn overlap(candidate_scope: &[String], engineer_changed: &[String]) -> Vec<String> {
    use std::collections::BTreeSet;
    let held: BTreeSet<&str> = engineer_changed.iter().map(String::as_str).collect();
    let mut out: Vec<String> = candidate_scope
        .iter()
        .filter(|p| held.contains(p.as_str()))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Resolve the merge-base commit between `HEAD` and `base_branch` inside
/// `worktree`. `None` on any git error (keeps `changed_files` absent-tolerant).
fn merge_base(worktree: &Path, base_branch: &str) -> Option<String> {
    let out = crate::util::spawn_retry::retry_spawn_sync(|| {
        Command::new("git")
            .arg("-C")
            .arg(worktree)
            .args(["merge-base", "HEAD", base_branch])
            .output()
    })
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let base = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if base.is_empty() { None } else { Some(base) }
}

/// Run `git -C <worktree> diff --name-only <extra args...>` and return the
/// reported paths. Empty vec on any error.
fn git_name_only(worktree: &Path, extra: &[&str]) -> Vec<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(worktree).args(["diff", "--name-only"]);
    for a in extra {
        cmd.arg(a);
    }
    let out = match cmd.output() {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Normalize a bag of paths to a sorted, de-duplicated set of repo-relative
/// POSIX strings (backslashes folded to `/` for cross-platform stability).
fn normalize_set(paths: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = paths.into_iter().map(|p| p.replace('\\', "/")).collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Initialise a throwaway git repo with one commit, returning its path.
    /// Hermetic: no network, uses local `git` only.
    fn init_repo(dir: &Path) {
        let run = |args: &[&str]| {
            let ok = crate::util::spawn_retry::retry_spawn_sync(|| {
                Command::new("git").arg("-C").arg(dir).args(args).output()
            })
            .expect("git runs")
            .status
            .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
    }

    fn commit_all(dir: &Path, msg: &str) {
        let run = |args: &[&str]| {
            crate::util::spawn_retry::retry_spawn_sync(|| {
                Command::new("git").arg("-C").arg(dir).args(args).output()
            })
            .expect("git runs");
        };
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", msg]);
    }

    // T9 — overlap unit: known committed diff ⇒ correct changed_files.
    #[test]
    fn changed_files_reports_committed_diff_since_base() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        std::fs::write(repo.join("base.rs"), "fn a() {}\n").unwrap();
        commit_all(repo, "base");
        // A feature branch that adds a new file on top of main.
        crate::util::spawn_retry::retry_spawn_sync(|| {
            Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["checkout", "-q", "-b", "feature"])
                .output()
        })
        .unwrap();
        std::fs::write(repo.join("feature.rs"), "fn b() {}\n").unwrap();
        commit_all(repo, "feature");

        let files = changed_files(repo, "main");
        assert!(
            files.contains(&"feature.rs".to_string()),
            "expected feature.rs in {files:?}"
        );
        assert!(
            !files.contains(&"base.rs".to_string()),
            "base.rs is on the base branch, not this engineer's change: {files:?}"
        );
    }

    // T9 — overlap unit: uncommitted working-tree edits are reported too.
    #[test]
    fn changed_files_reports_uncommitted_working_tree_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        std::fs::write(repo.join("tracked.rs"), "fn a() {}\n").unwrap();
        commit_all(repo, "base");
        // Edit the tracked file without committing.
        std::fs::write(repo.join("tracked.rs"), "fn a() { /* edit */ }\n").unwrap();

        let files = changed_files(repo, "main");
        assert!(
            files.contains(&"tracked.rs".to_string()),
            "expected uncommitted edit reported: {files:?}"
        );
    }

    // T9 — absent-tolerant: a non-repo dir ⇒ EMPTY set (fail-open).
    #[test]
    fn changed_files_on_non_repo_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let files = changed_files(tmp.path(), "main");
        assert!(files.is_empty(), "non-repo must yield empty set: {files:?}");
    }

    // T9 — absent-tolerant: a missing base branch ⇒ still no panic, working-tree
    // diff (if any) still contributes; here empty because clean tree.
    #[test]
    fn changed_files_missing_base_is_empty_on_clean_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        std::fs::write(repo.join("a.rs"), "fn a() {}\n").unwrap();
        commit_all(repo, "base");
        let files = changed_files(repo, "does-not-exist");
        assert!(files.is_empty(), "clean tree + bad base ⇒ empty: {files:?}");
    }

    #[test]
    fn overlap_is_intersection_exact_match() {
        let scope = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
        let changed = vec!["src/b.rs".to_string(), "src/c.rs".to_string()];
        assert_eq!(overlap(&scope, &changed), vec!["src/b.rs".to_string()]);
    }

    #[test]
    fn overlap_empty_when_disjoint() {
        let scope = vec!["src/a.rs".to_string()];
        let changed = vec!["src/z.rs".to_string()];
        assert!(overlap(&scope, &changed).is_empty());
    }

    #[test]
    fn overlap_empty_scope_is_empty() {
        assert!(overlap(&[], &["x".to_string()]).is_empty());
    }
}
