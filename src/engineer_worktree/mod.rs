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
mod tests_extra;
#[cfg(test)]
#[cfg(test)]
mod tests_more;

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

mod claim;
pub use claim::{is_pid_alive_public, read_pid_starttime_public};
use claim::{EngineerClaim, claim_is_live, format_engineer_claim, is_pid_alive, read_engineer_claim, read_engineer_claim_full, read_pid_starttime};

/// Maximum length of a `goal_id` accepted by [`EngineerWorktree::allocate`].
pub const MAX_GOAL_ID_LEN: usize = 64;

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
mod sweep;
pub use sweep::sweep_orphaned_worktrees;
use sweep::{git_capture, is_valid_sha40, validate_goal_id};


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
