//! Sweeping orphaned engineer worktrees + helpers used by allocate.

use std::path::Path;
use std::process::Command;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use crate::error::SimardError;
use super::{MAX_GOAL_ID_LEN, WORKTREES_SUBDIR};

use super::{
    EngineerClaim, ENGINEER_CLAIM_FILE, SweepReport, claim_is_live, is_pid_alive_public,
    read_engineer_claim_full, read_pid_starttime_public, worktree_mutation_lock,
};


/// Sweep `<state_root>/engineer-worktrees/` for orphans on daemon boot.
///
/// Runs `git worktree prune` first, then removes any directory in the
/// worktrees root that is not registered with the parent repository.
///
/// Symlinks under the worktrees root are NEVER followed: a planted symlink
/// pointing at e.g. `$HOME` would otherwise be classified as an orphan
/// directory and trigger `remove_dir_all` against the symlink target. They
/// are skipped with a WARN so an operator notices.
pub fn sweep_orphaned_worktrees(
    parent_repo: &Path,
    state_root: &Path,
) -> Result<SweepReport, SimardError> {
    const ACTION: &str = "engineer_worktree::sweep_orphaned_worktrees";
    let mut report = SweepReport::default();
    let fail = |reason: String| SimardError::ActionExecutionFailed {
        action: ACTION.to_string(),
        reason,
    };

    // Step 1: prune stale `.git/worktrees/` registrations from the parent.
    git_capture(parent_repo, &["worktree", "prune"])
        .map_err(|r| fail(format!("`git worktree prune` failed: {r}")))?;

    // Step 2: enumerate currently-registered worktree paths (canonicalized).
    // Use a HashSet so the orphan walk below is O(N+M) instead of O(N*M).
    // Canonicalization failure is fail-loud: a non-canonical registered
    // path could miscompare against a canonical orphan and cause us to
    // delete a live worktree.
    let listing = git_capture(parent_repo, &["worktree", "list", "--porcelain"])
        .map_err(|r| fail(format!("`git worktree list` failed: {r}")))?;
    let mut registered: HashSet<PathBuf> = HashSet::new();
    for line in listing.lines() {
        let Some(raw) = line.strip_prefix("worktree ") else {
            continue;
        };
        let p = PathBuf::from(raw);
        let canonical = p.canonicalize().map_err(|e| {
            fail(format!(
                "cannot canonicalize registered worktree path {}: {e}",
                p.display()
            ))
        })?;
        registered.insert(canonical);
    }

    // Step 3: walk the worktrees subdir and remove unregistered entries.
    let worktrees_root = state_root.join(WORKTREES_SUBDIR);
    if !worktrees_root.exists() {
        return Ok(report);
    }
    let worktrees_root_canonical = worktrees_root.canonicalize().map_err(|e| {
        fail(format!(
            "cannot canonicalize worktrees root {}: {e}",
            worktrees_root.display()
        ))
    })?;

    let entries = fs::read_dir(&worktrees_root).map_err(|e| {
        fail(format!(
            "cannot read worktrees root {}: {e}",
            worktrees_root.display()
        ))
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        // Use symlink_metadata so we never traverse a symlink. A symlink
        // planted under the worktrees root is suspicious — log and skip.
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    target: "simard::engineer_worktree",
                    error = %e,
                    entry = %path.display(),
                    "cannot stat entry under worktrees root; skipping",
                );
                continue;
            }
        };
        let ftype = meta.file_type();
        if ftype.is_symlink() {
            tracing::warn!(
                target: "simard::engineer_worktree",
                entry = %path.display(),
                "refusing to follow symlink under engineer-worktrees root; skipping",
            );
            continue;
        }
        if !ftype.is_dir() {
            continue;
        }
        // canonicalize() on a real directory under the worktrees root must
        // succeed; failure here is suspicious (race? perms?). Fail loud
        // rather than silently fall back to the non-canonical path and
        // risk a false-orphan deletion of a live worktree.
        let canonical = path.canonicalize().map_err(|e| {
            fail(format!(
                "cannot canonicalize entry {} under worktrees root: {e}",
                path.display()
            ))
        })?;
        // Defense-in-depth: even after canonicalization, refuse to operate
        // on anything that resolves outside the canonical worktrees root.
        if !canonical.starts_with(&worktrees_root_canonical) {
            tracing::warn!(
                target: "simard::engineer_worktree",
                entry = %path.display(),
                canonical = %canonical.display(),
                "entry under worktrees root canonicalizes outside the root; skipping",
            );
            continue;
        }
        if registered.contains(&canonical) {
            continue;
        }
        // Issue #1213 / #1238: skip dirs whose engineer-claim sentinel
        // names a live PID whose starttime still matches. Git's
        // `worktree prune` can transiently drop a registration (observed
        // during concurrent worktree mutations) and we must not delete
        // a worktree out from under a running engineer subprocess.
        // Starttime validation prevents the recycled-PID false positive
        // after a daemon restart.
        if let Some(claim) = read_engineer_claim_full(&canonical)
            && claim_is_live(&claim)
        {
            tracing::debug!(
                target: "simard::engineer_worktree",
                worktree = %canonical.display(),
                pid = claim.pid,
                starttime = ?claim.starttime,
                "skipping unregistered worktree with live engineer-claim",
            );
            report.skipped_live_dirs.push(canonical);
            continue;
        }
        if let Err(e) = fs::remove_dir_all(&path) {
            tracing::warn!(
                target: "simard::engineer_worktree",
                error = %e,
                orphan = %path.display(),
                "failed to remove orphaned engineer worktree dir",
            );
            continue;
        }
        report.removed_orphan_dirs.push(path);
    }

    Ok(report)
}

/// Run a `git` subcommand in `repo` and return stdout on success.
///
/// `Command::env_clear()` is called before re-injecting only `PATH` and
/// `HOME` — this prevents an attacker who can set the daemon's env from
/// hijacking every git call here via `GIT_DIR`, `GIT_WORK_TREE`,
/// `GIT_INDEX_FILE`, `GIT_CONFIG_GLOBAL`, `LD_PRELOAD`, etc.
pub fn git_capture(repo: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo).env_clear();
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(home) = std::env::var("HOME") {
        cmd.env("HOME", home);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("spawn git {args:?}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {:?} exited with {} in {}: {}",
            args,
            output.status,
            repo.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Validate a `goal_id` is safe to interpolate into both a filesystem path
/// segment and a git ref name.
///
/// Accepts `^[A-Za-z0-9._-]{1,64}$`; rejects empty input, leading `-` (git
/// ref injection / argv injection), and leading `.` (hidden file / `..`
/// path traversal).
pub fn validate_goal_id(goal_id: &str) -> Result<(), String> {
    if goal_id.is_empty() {
        return Err("goal_id must not be empty".to_string());
    }
    if goal_id.len() > MAX_GOAL_ID_LEN {
        return Err(format!(
            "goal_id length {} exceeds max {MAX_GOAL_ID_LEN}",
            goal_id.len()
        ));
    }
    let first = goal_id.as_bytes()[0];
    if first == b'-' || first == b'.' {
        return Err(format!("goal_id must not start with {:?}", first as char));
    }
    for (i, b) in goal_id.bytes().enumerate() {
        let ok = b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-';
        if !ok {
            return Err(format!(
                "goal_id contains disallowed byte {:?} at index {i}",
                b as char
            ));
        }
    }
    Ok(())
}

/// True iff `s` is exactly 40 lowercase-hex characters (a full git SHA-1).
pub fn is_valid_sha40(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
