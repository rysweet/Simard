//! Per-engineer git worktree isolation (issue #1197).
//!
//! Allocates a dedicated git worktree under `<state_root>/engineer-worktrees/`
//! for each spawned engineer subprocess, so concurrent engineers never share
//! the same git working directory. This eliminates the
//! "worktree state changed during a non-mutating local engineer action"
//! verification race that was preventing the OODA daemon from shipping PRs.
//!
//! See `docs/reference/engineer-worktree-isolation.md` for the full contract.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use std::sync::OnceLock;

#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;

/// Process-wide lock serializing mutating `git worktree` commands against
/// the parent repository. Git's `.git/worktrees/` registry is not safe to
/// mutate concurrently from the same parent (observed: "failed to read
/// .git/worktrees/<other>/commondir: Success" under parallel `worktree add`).
fn worktree_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

use crate::error::SimardError;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_extra;

/// Subdirectory under the supervisor state root that holds all engineer worktrees.
pub const WORKTREES_SUBDIR: &str = "engineer-worktrees";

/// Filename of the per-worktree liveness sentinel (issue #1213). Contains the
/// PID of the process that allocated the worktree, plus its starttime read
/// from `/proc/<pid>/stat` field 22 (issue #1238). The starttime guards
/// against the daemon-restart-with-recycled-PID race: after a daemon restart,
/// the new daemon's PID is unrelated to the old one, but Linux can recycle
/// PIDs over time. Recording (PID, starttime) lets us distinguish "the
/// original claimant is still running" from "a different process happens
/// to occupy that PID slot now."
///
/// File format (line-separated, trailing newline tolerated):
///   line 1: `<pid>` (decimal i32, required)
///   line 2: `<starttime>` (u64 jiffies from /proc/<pid>/stat field 22,
///           optional — absent in pre-#1238 sentinels)
pub const ENGINEER_CLAIM_FILE: &str = ".simard-engineer-claim";

/// Read field 22 (starttime in jiffies since boot) from `/proc/<pid>/stat`.
/// Returns `None` if the file can't be read or is malformed.
///
/// `/proc/<pid>/stat` format: `pid (comm) state ppid ...` where `comm` may
/// itself contain spaces and parentheses. We must therefore find the LAST
/// `)` and split the remainder by whitespace; field 22 (starttime) is the
/// 20th token AFTER `comm` (state=1, ppid=2, ..., starttime=20).
#[cfg(unix)]
fn read_pid_starttime(pid: i32) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    let raw = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close_paren = raw.rfind(')')?;
    let after = raw.get(close_paren + 1..)?.trim_start();
    // After `comm`: state(1) ppid(2) pgrp(3) session(4) tty_nr(5) tpgid(6)
    // flags(7) minflt(8) cminflt(9) majflt(10) cmajflt(11) utime(12) stime(13)
    // cutime(14) cstime(15) priority(16) nice(17) num_threads(18)
    // itrealvalue(19) starttime(20)
    let mut tokens = after.split_ascii_whitespace();
    let starttime = tokens.nth(19)?;
    starttime.parse().ok()
}

#[cfg(not(unix))]
fn read_pid_starttime(_pid: i32) -> Option<u64> {
    None
}

/// Format the contents written into the engineer-claim sentinel file.
fn format_engineer_claim(pid: u32) -> String {
    match read_pid_starttime(pid as i32) {
        Some(st) => format!("{pid}\n{st}\n"),
        // /proc unavailable (test sandboxes, non-Linux): fall back to PID-only.
        // Read path treats absent starttime as "unverifiable", but kill(pid,0)
        // alone is still better than no claim.
        None => format!("{pid}\n"),
    }
}

/// Parsed engineer-claim sentinel contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EngineerClaim {
    pid: i32,
    /// Starttime in /proc jiffies, if recorded (None for pre-#1238 sentinels
    /// or platforms without /proc).
    starttime: Option<u64>,
}

/// Probe whether `pid` refers to a running process via `kill(pid, 0)`. Returns
/// `true` if the process exists (regardless of permission to signal it).
/// Returns `false` if the process is dead (ESRCH) or `pid` is non-positive.
#[cfg(unix)]
fn is_pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill(pid, 0) performs no signal delivery. It is the standard
    // POSIX liveness probe and has no side effects on the target process.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    // EPERM means the process exists but we can't signal it — still alive.
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    errno == libc::EPERM
}

#[cfg(not(unix))]
fn is_pid_alive(_pid: i32) -> bool {
    // Non-Unix platforms don't run the daemon; conservative default.
    true
}

/// Public wrapper over the private liveness probe so other modules
/// (e.g. `ooda_actions::advance_goal::find_live_engineer_for_goal`)
/// can implement their own claim-based checks without duplicating the
/// `kill(pid, 0)` logic.
pub fn is_pid_alive_public(pid: i32) -> bool {
    is_pid_alive(pid)
}

/// Public wrapper over `/proc/<pid>/stat` starttime lookup. Used by
/// `ooda_actions::advance_goal` so it can do the same starttime-validated
/// claim check as the sweep path.
pub fn read_pid_starttime_public(pid: i32) -> Option<u64> {
    read_pid_starttime(pid)
}

/// Read the engineer-claim sentinel out of `worktree_dir/.simard-engineer-claim`.
/// Returns `None` if the file is missing, empty, malformed, or unreadable.
/// Tolerant of all I/O errors — the caller treats `None` as "no claim".
fn read_engineer_claim_full(worktree_dir: &Path) -> Option<EngineerClaim> {
    let path = worktree_dir.join(ENGINEER_CLAIM_FILE);
    let raw = fs::read_to_string(&path).ok()?;
    let mut lines = raw.lines();
    let pid: i32 = lines.next()?.trim().parse().ok()?;
    let starttime = lines.next().and_then(|s| s.trim().parse::<u64>().ok());
    Some(EngineerClaim { pid, starttime })
}

/// Back-compat thin wrapper for callers that only care about the PID.
#[allow(dead_code)]
fn read_engineer_claim(worktree_dir: &Path) -> Option<i32> {
    read_engineer_claim_full(worktree_dir).map(|c| c.pid)
}

/// Decide whether a parsed claim still names the original allocating process.
///
/// Returns `true` only if BOTH:
///   1. `kill(pid, 0)` reports the PID is alive
///   2. The recorded starttime matches the live process's current starttime
///      (or the claim has no starttime, in which case we fall back to PID-only)
///
/// The starttime check defends against the daemon-restart-with-recycled-PID
/// false positive: after a daemon restart the old PID may eventually be
/// reused by an unrelated process, but its starttime will differ.
fn claim_is_live(claim: &EngineerClaim) -> bool {
    if !is_pid_alive(claim.pid) {
        return false;
    }
    match claim.starttime {
        Some(recorded) => match read_pid_starttime(claim.pid) {
            Some(current) => current == recorded,
            // Process exists but we can't read its stat — be conservative
            // and treat as NOT live (better to occasionally re-allocate a
            // worktree than to nuke a live engineer's cwd).
            None => false,
        },
        // Pre-#1238 sentinel: no starttime recorded. Fall back to PID-only.
        None => true,
    }
}

/// Maximum length of a `goal_id` accepted by [`EngineerWorktree::allocate`].
const MAX_GOAL_ID_LEN: usize = 64;

/// A per-engineer git worktree.
///
/// Construct via [`EngineerWorktree::allocate`]. The worktree is registered
/// in the parent repository under a fresh `engineer/<goal-id>-<suffix>`
/// branch and lives at `<state_root>/engineer-worktrees/<goal-id>-<suffix>/`.
///
/// Cleanup is idempotent and runs either via [`EngineerWorktree::cleanup`]
/// (the explicit, observable path) or via [`Drop`] (the safety-net path).
#[derive(Debug)]
pub struct EngineerWorktree {
    path: PathBuf,
    branch: String,
    parent_repo: PathBuf,
    /// Canonicalized `<state_root>/engineer-worktrees/`. Used by cleanup
    /// paths to assert the target dir is contained inside the managed
    /// root before any `fs::remove_dir_all` (defense against bugs that
    /// could let `path` drift outside of the worktrees root).
    worktrees_root_canonical: PathBuf,
    cleaned: AtomicBool,
}

/// Result of a startup sweep over `<state_root>/engineer-worktrees/`.
#[derive(Debug, Default)]
pub struct SweepReport {
    /// Directories that were physically removed because they were not
    /// registered with the parent repository.
    pub removed_orphan_dirs: Vec<PathBuf>,
    /// Directories that were unregistered with the parent repo but skipped
    /// because their `.simard-engineer-claim` sentinel named a live PID
    /// (issue #1213). Useful for diagnostics and tests.
    pub skipped_live_dirs: Vec<PathBuf>,
}

impl EngineerWorktree {
    /// Allocate a fresh git worktree for an engineer pursuing `goal_id`.
    ///
    /// Branches off the parent repository's current `main` HEAD. **Fails
    /// loud** if `main` cannot be resolved or if `goal_id` is not a safe
    /// identifier — there is no fallback to `HEAD`, per the repo's
    /// no-fallback convention.
    pub fn allocate(
        parent_repo: &Path,
        state_root: &Path,
        goal_id: &str,
    ) -> Result<Self, SimardError> {
        // 0. Validate goal_id at the boundary. Rejects path traversal,
        //    git ref-injection, and oversized inputs before they hit
        //    the filesystem or git ref namespace.
        validate_goal_id(goal_id).map_err(|reason| SimardError::ActionExecutionFailed {
            action: format!("engineer_worktree::allocate(goal={goal_id:?})"),
            reason,
        })?;

        // 1. Resolve the parent repo's `main` HEAD. No fallback.
        let main_sha = git_capture(parent_repo, &["rev-parse", "main"]).map_err(|reason| {
            SimardError::ActionExecutionFailed {
                action: format!("engineer_worktree::allocate(goal={goal_id})"),
                reason: format!(
                    "cannot resolve `main` in {}: {reason}",
                    parent_repo.display()
                ),
            }
        })?;
        let main_sha = main_sha.trim();
        if !is_valid_sha40(main_sha) {
            return Err(SimardError::ActionExecutionFailed {
                action: format!("engineer_worktree::allocate(goal={goal_id})"),
                reason: format!(
                    "`git rev-parse main` returned non-40-hex output {main_sha:?} in {}",
                    parent_repo.display()
                ),
            });
        }

        // 2. Build a unique suffix.
        let suffix = unique_suffix();
        let dir_name = format!("{goal_id}-{suffix}");
        let worktrees_root = state_root.join(WORKTREES_SUBDIR);
        let dir = worktrees_root.join(&dir_name);
        let branch = format!("engineer/{dir_name}");

        // 3. Ensure the worktrees root exists with mode 0700 on Unix.
        //    Worktrees may transiently hold credentials or .env files;
        //    do not expose them to other local users.
        create_worktrees_root(&worktrees_root).map_err(|e| SimardError::ActionExecutionFailed {
            action: format!("engineer_worktree::allocate(goal={goal_id})"),
            reason: format!(
                "cannot create worktrees root {}: {e}",
                worktrees_root.display()
            ),
        })?;

        // Canonicalize the worktrees root once now that it exists. Used by
        // cleanup_inner / the failure-recovery path below to refuse any
        // `remove_dir_all` whose canonical path is not contained here.
        let worktrees_root_canonical =
            worktrees_root
                .canonicalize()
                .map_err(|e| SimardError::ActionExecutionFailed {
                    action: format!("engineer_worktree::allocate(goal={goal_id})"),
                    reason: format!(
                        "cannot canonicalize worktrees root {}: {e}",
                        worktrees_root.display()
                    ),
                })?;

        // 4. `git worktree add -b <branch> <dir> <main_sha>` — serialized
        //    against the parent repo because git's worktree registry races.
        let dir_str = dir.to_string_lossy();
        let result = {
            let _guard = worktree_mutation_lock()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            git_capture(
                parent_repo,
                &["worktree", "add", "-b", &branch, &dir_str, main_sha],
            )
        };
        if let Err(reason) = result {
            // Best-effort cleanup of any partial state before failing loud.
            // Each failure is logged at WARN — never silently swallowed.
            // The dir-removal is gated on the canonical-prefix check so a
            // future bug that lets `dir` drift outside the worktrees root
            // cannot escalate to out-of-root deletion.
            if dir.exists() {
                match assert_under_root(&dir, &worktrees_root_canonical) {
                    Ok(safe_dir) => {
                        if let Err(e) = fs::remove_dir_all(&safe_dir) {
                            tracing::warn!(
                                target: "simard::engineer_worktree",
                                error = %e,
                                worktree = %safe_dir.display(),
                                "failed to clean up partial worktree dir after `git worktree add` failure",
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "simard::engineer_worktree",
                            error = %e,
                            worktree = %dir.display(),
                            "refusing to remove partial worktree dir: not contained in canonical worktrees root",
                        );
                    }
                }
            }
            if let Err(e) = git_capture(parent_repo, &["worktree", "prune"]) {
                tracing::warn!(
                    target: "simard::engineer_worktree",
                    error = %e,
                    "git worktree prune failed during allocate-failure recovery",
                );
            }
            if let Err(e) = git_capture(parent_repo, &["branch", "-D", &branch]) {
                tracing::warn!(
                    target: "simard::engineer_worktree",
                    error = %e,
                    branch = %branch,
                    "best-effort branch delete failed during allocate-failure recovery",
                );
            }
            return Err(SimardError::ActionExecutionFailed {
                action: format!("engineer_worktree::allocate(goal={goal_id})"),
                reason: format!("`git worktree add` failed: {reason}"),
            });
        }

        // 5. Write the per-worktree liveness sentinel (issue #1213, refined
        //    in #1238). If the sweep ever runs against this worktree while
        //    git's registration is transiently missing, the live PID +
        //    starttime guard prevents the cwd-deletion-under-the-engineer's-
        //    feet bug. Recording starttime alongside the PID closes the
        //    daemon-restart-with-recycled-PID race.
        let claim_path = dir.join(ENGINEER_CLAIM_FILE);
        let claim_pid = std::process::id();
        if let Err(e) = fs::write(&claim_path, format_engineer_claim(claim_pid)) {
            // Sentinel write failure is non-fatal: the AtomicBool guard plus
            // the existing canonical-prefix safety still protect us. Log loud
            // so the regression is visible.
            tracing::warn!(
                target: "simard::engineer_worktree",
                error = %e,
                claim = %claim_path.display(),
                "failed to write engineer-claim sentinel; sweep falls back to git-registration check only",
            );
        }

        Ok(Self {
            path: dir,
            branch,
            parent_repo: parent_repo.to_path_buf(),
            worktrees_root_canonical,
            cleaned: AtomicBool::new(false),
        })
    }

    /// Path to the worktree on disk.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Name of the branch checked out in this worktree.
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// Remove the worktree, prune its registration, delete its branch.
    ///
    /// Idempotent — second and subsequent calls are `Ok(())` no-ops.
    /// Returns the first hard error encountered (canonical-prefix guard
    /// rejection or filesystem failure on the worktree dir). Best-effort
    /// git registry/branch failures are logged but do not propagate, so
    /// a partially-cleaned worktree still drives the call to a result.
    pub fn cleanup(&self) -> Result<(), SimardError> {
        if self.cleaned.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        cleanup_inner(
            &self.parent_repo,
            &self.path,
            &self.branch,
            &self.worktrees_root_canonical,
        )
    }
}

impl Drop for EngineerWorktree {
    fn drop(&mut self) {
        if self.cleaned.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Err(e) = cleanup_inner(
            &self.parent_repo,
            &self.path,
            &self.branch,
            &self.worktrees_root_canonical,
        ) {
            tracing::warn!(
                target: "simard::engineer_worktree",
                error = %e,
                worktree = %self.path.display(),
                "Drop-path cleanup of engineer worktree returned a hard error",
            );
        }
    }
}

/// Cleanup primitive shared by `cleanup()` and `Drop`.
///
/// Hard errors (canonical-prefix guard rejection, dir removal failure)
/// propagate. Best-effort git invocations log at WARN/DEBUG and continue —
/// they describe registry state that the next `worktree prune` will
/// reconcile, so they should not abort cleanup of the on-disk dir.
fn cleanup_inner(
    parent_repo: &Path,
    dir: &Path,
    branch: &str,
    worktrees_root_canonical: &Path,
) -> Result<(), SimardError> {
    const ACTION: &str = "engineer_worktree::cleanup";
    let dir_str = dir.to_string_lossy();
    // Serialize all mutations to the parent's `.git/worktrees/` registry.
    let _guard = worktree_mutation_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Err(e) = git_capture(parent_repo, &["worktree", "remove", "--force", &dir_str]) {
        tracing::debug!(
            target: "simard::engineer_worktree",
            error = %e,
            worktree = %dir.display(),
            "git worktree remove failed (will fall through to manual rmdir+prune)",
        );
    }
    if let Err(e) = git_capture(parent_repo, &["worktree", "prune"]) {
        tracing::debug!(
            target: "simard::engineer_worktree",
            error = %e,
            "git worktree prune failed during cleanup",
        );
    }
    if dir.exists() {
        // Refuse to delete anything whose canonical path is not contained
        // inside the canonical worktrees root we recorded at allocate-time.
        let safe_dir = assert_under_root(dir, worktrees_root_canonical).map_err(|reason| {
            SimardError::ActionExecutionFailed {
                action: ACTION.to_string(),
                reason,
            }
        })?;
        fs::remove_dir_all(&safe_dir).map_err(|e| SimardError::ActionExecutionFailed {
            action: ACTION.to_string(),
            reason: format!("failed to remove worktree dir {}: {e}", safe_dir.display()),
        })?;
    }
    if let Err(e) = git_capture(parent_repo, &["branch", "-D", branch]) {
        tracing::debug!(
            target: "simard::engineer_worktree",
            error = %e,
            branch = %branch,
            "best-effort branch delete failed (branch may already be gone)",
        );
    }
    Ok(())
}

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
fn git_capture(repo: &Path, args: &[&str]) -> Result<String, String> {
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
fn validate_goal_id(goal_id: &str) -> Result<(), String> {
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
fn is_valid_sha40(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Canonicalize `dir` and verify it lives under `root_canonical`.
/// Returns the canonicalized path so the caller can `fs::remove_dir_all`
/// on the same path that was just verified (avoiding a TOCTOU between
/// check and use through the symlink-resolved view).
fn assert_under_root(dir: &Path, root_canonical: &Path) -> Result<PathBuf, String> {
    let canonical = dir.canonicalize().map_err(|e| {
        format!(
            "cannot canonicalize {} for prefix-check against {}: {e}",
            dir.display(),
            root_canonical.display()
        )
    })?;
    if !canonical.starts_with(root_canonical) {
        return Err(format!(
            "refusing to operate on {} (canonical {}): not contained in worktrees root {}",
            dir.display(),
            canonical.display(),
            root_canonical.display()
        ));
    }
    Ok(canonical)
}

/// Create the worktrees root with restrictive permissions on Unix (`0o700`).
/// On non-Unix, falls back to the platform default (umask-controlled).
fn create_worktrees_root(root: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut b = fs::DirBuilder::new();
        b.recursive(true).mode(0o700);
        b.create(root)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(root)
    }
}

/// Build a collision-resistant `<epoch_secs>-<6hex>` suffix.
///
/// Combines wall-clock nanos, an in-process atomic counter, and the process
/// id to survive same-second parallel allocations without taking a `rand` or
/// `uuid` dependency.
fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id() as u64;
    // Multiplicative hash so the 24 bits we keep are well distributed even
    // when nanos is small.
    let mix = (now.subsec_nanos() as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(counter.wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add(pid.wrapping_mul(0x94D0_49BB_1331_11EB));
    let hex = (mix & 0xFF_FFFF) as u32;
    format!("{}-{hex:06x}", now.as_secs())
}
