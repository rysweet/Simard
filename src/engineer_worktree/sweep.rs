//! Sweeping orphaned engineer worktrees + helpers used by allocate.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use super::{MAX_GOAL_ID_LEN, WORKTREES_SUBDIR};
use crate::error::SimardError;

use super::{SweepReport, claim_is_live, read_engineer_claim_full};
use crate::worktree_gc::liveness::{LiveProcessProbe, ProcfsLiveProcessProbe};

/// Why the sweep physically removed an orphan directory. Recorded 1:1 with
/// each entry in [`SweepReport::removed_orphan_dirs`] so every deletion is
/// observable and attributable (issue #2553).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalReason {
    /// The directory was an unregistered orphan that passed every safety
    /// guard: it is inside the engineer-worktrees root (SCOPE), is not the CWD
    /// of any live process (LIVE_CWD), has no live engineer claim (LIVE_CLAIM),
    /// and carries no recoverable git work — either it is not a git worktree at
    /// all, or a clean worktree whose HEAD is not ahead of a configured
    /// upstream (WORK_STATE). `had_dead_claim` records whether a stale
    /// (dead-PID) `.simard-engineer-claim` sentinel was present.
    OrphanedNoLiveNoWork { had_dead_claim: bool },
}

/// Sweep `<state_root>/engineer-worktrees/` for orphans on daemon boot and on
/// the periodic timer.
///
/// This is the production entry point used by the OODA daemon. It delegates to
/// [`sweep_orphaned_worktrees_inner`] with the real [`ProcfsLiveProcessProbe`]
/// (which reads `/proc/<pid>/cwd`). See that function for the full guard
/// contract.
pub fn sweep_orphaned_worktrees(
    parent_repo: &Path,
    state_root: &Path,
) -> Result<SweepReport, SimardError> {
    let probe = ProcfsLiveProcessProbe::new();
    sweep_orphaned_worktrees_inner(parent_repo, state_root, &probe)
}

/// Core sweep with an injectable liveness probe (issue #2553).
///
/// Runs `git worktree prune` first, then removes directories under the
/// engineer-worktrees root that are not registered with the parent repository
/// — but ONLY after each candidate passes every safety guard, applied
/// cheapest-first / most-destructive-last:
///
///   1. **SCOPE** — the sweep only ever reads
///      `<state_root>/engineer-worktrees/`; every candidate is canonicalized
///      and asserted to resolve *inside* that root. A directory that
///      canonicalizes outside the root (symlink, race) is skipped, never
///      removed. Directories elsewhere on disk — e.g. the operator's own
///      `~/src/Simard/worktrees/` checkouts — are never even enumerated.
///   2. **LIVE_CLAIM** — a directory whose `.simard-engineer-claim` sentinel
///      names a live PID (with matching starttime) is skipped (issues
///      #1213 / #1238): `git worktree prune` can transiently drop a
///      registration while an engineer subprocess is still running in it.
///   3. **LIVE_CWD** — a directory that is the current working directory of ANY
///      live process is skipped (reuses [`crate::worktree_gc::liveness`]). The
///      probe fail-closes: if it cannot answer authoritatively it reports
///      "live" and we keep the worktree. This is the direct fix for the
///      verified incident (an in-use rebase/build worktree removed mid-op).
///   4. **WORK_STATE** — a real git worktree with uncommitted changes, unpushed
///      commits, no provable upstream, or an unverifiable git state is skipped
///      so no unsaved work is ever destroyed. Only a directory that is NOT a
///      git worktree, or a clean worktree whose HEAD is not ahead of a
///      configured upstream, is eligible to be reaped.
///   5. **REAP** — the surviving orphan is removed with `remove_dir_all`, logged
///      at INFO with its [`RemovalReason`], and recorded in the report.
///
/// Symlinks under the worktrees root are NEVER followed: a planted symlink
/// pointing at e.g. `$HOME` would otherwise be classified as an orphan
/// directory and trigger `remove_dir_all` against the symlink target. They are
/// skipped with a WARN so an operator notices.
pub fn sweep_orphaned_worktrees_inner(
    parent_repo: &Path,
    state_root: &Path,
    probe: &dyn LiveProcessProbe,
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
        // SCOPE guard: even after canonicalization, refuse to operate on
        // anything that resolves outside the canonical worktrees root.
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
        // LIVE_CLAIM guard (issues #1213 / #1238): skip dirs whose
        // engineer-claim sentinel names a live PID whose starttime still
        // matches. Git's `worktree prune` can transiently drop a
        // registration (observed during concurrent worktree mutations) and
        // we must not delete a worktree out from under a running engineer
        // subprocess. Starttime validation prevents the recycled-PID false
        // positive after a daemon restart. We keep the parsed claim to
        // record `had_dead_claim` on the eventual removal reason.
        let claim = read_engineer_claim_full(&canonical);
        if let Some(ref c) = claim
            && claim_is_live(c)
        {
            tracing::debug!(
                target: "simard::engineer_worktree",
                worktree = %canonical.display(),
                pid = c.pid,
                starttime = ?c.starttime,
                "skipping unregistered worktree with live engineer-claim",
            );
            report.skipped_live_dirs.push(canonical);
            continue;
        }
        let had_dead_claim = claim.is_some();

        // LIVE_CWD guard (issue #2553): never remove a worktree that is the
        // CWD of any live process. The probe fail-closes — on any error it
        // reports "live" and we keep the directory.
        if probe.worktree_has_live_process(&path) {
            tracing::debug!(
                target: "simard::engineer_worktree",
                worktree = %canonical.display(),
                "skipping unregistered worktree: a live process has its CWD here (#2553)",
            );
            report.skipped_live_cwd_dirs.push(canonical);
            continue;
        }

        // WORK_STATE guard (issue #2553): never destroy uncommitted or
        // unpushed work. A directory that is not a git worktree has no
        // git-tracked work to lose and falls through to REAP.
        if worktree_has_recoverable_work(&path) {
            tracing::debug!(
                target: "simard::engineer_worktree",
                worktree = %canonical.display(),
                "skipping unregistered worktree: uncommitted / unpushed / unverifiable work (#2553)",
            );
            report.skipped_dirty_dirs.push(canonical);
            continue;
        }

        // REAP: every guard passed. Remove and record an observable reason.
        if let Err(e) = fs::remove_dir_all(&path) {
            tracing::warn!(
                target: "simard::engineer_worktree",
                error = %e,
                orphan = %path.display(),
                "failed to remove orphaned engineer worktree dir",
            );
            continue;
        }
        let reason = RemovalReason::OrphanedNoLiveNoWork { had_dead_claim };
        tracing::info!(
            target: "simard::engineer_worktree",
            orphan = %path.display(),
            had_dead_claim,
            reason = ?reason,
            "reaped orphaned engineer worktree \
             (scope+live-claim+live-cwd+work-state guards passed)",
        );
        report.removed_orphan_dirs.push(path.clone());
        report.removal_reasons.push((path, reason));
    }

    Ok(report)
}

/// Return `true` if the git worktree at `dir` holds work that a sweep must not
/// destroy (issue #2553): uncommitted changes, unpushed commits, no provable
/// upstream, or an unverifiable git state.
///
/// Returns `false` only when the directory is provably safe to remove: either
/// it is not a git worktree at all (no `.git` entry — a plain leftover
/// directory), or a clean worktree whose HEAD is not ahead of a configured
/// upstream.
///
/// The daemon sweep is deliberately MORE conservative than the operator GC:
/// it has no merged-PR / branch-deletion policy to independently prove a
/// branch's commits are safe, so a clean worktree with **no** configured
/// upstream is kept (we cannot prove its commits were pushed). Every error is
/// treated as "has work" (fail-safe keep).
fn worktree_has_recoverable_work(dir: &Path) -> bool {
    // Not a git worktree (no `.git` file or dir) → nothing git-tracked to lose.
    if !dir.join(".git").exists() {
        return false;
    }
    // Uncommitted changes (tracked or untracked) → keep.
    match git_capture(dir, &["status", "--porcelain"]) {
        Ok(out) => {
            if !out.trim().is_empty() {
                return true;
            }
        }
        // Broken / unreadable git state → conservative keep.
        Err(_) => return true,
    }
    // Clean tree. Prove every commit is pushed: HEAD must not be ahead of a
    // configured upstream. A missing upstream errors here → cannot prove
    // pushed → keep.
    match git_capture(dir, &["rev-list", "--count", "@{u}..HEAD"]) {
        Ok(count) => count.trim() != "0",
        Err(_) => true,
    }
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
