//! Per-engineer git worktree isolation (issue #1197).
//!
//! Allocates a dedicated git worktree under `<state_root>/engineer-worktrees/`
//! for each spawned engineer subprocess, so concurrent engineers never share
//! the same git working directory. This eliminates the
//! "worktree state changed during a non-mutating local engineer action"
//! verification race that was preventing the OODA daemon from shipping PRs.
//!
//! See `docs/reference/engineer-worktree-isolation.md` for the full contract.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::SimardError;

#[cfg(test)]
mod tests;

/// Subdirectory under the supervisor state root that holds all engineer worktrees.
pub const WORKTREES_SUBDIR: &str = "engineer-worktrees";

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
    cleaned: AtomicBool,
}

/// Result of a startup sweep over `<state_root>/engineer-worktrees/`.
#[derive(Debug, Default)]
pub struct SweepReport {
    /// Whether `git worktree prune` succeeded; counts the prune itself.
    pub pruned_registrations: usize,
    /// Directories that were physically removed because they were not
    /// registered with the parent repository.
    pub removed_orphan_dirs: Vec<PathBuf>,
}

impl EngineerWorktree {
    /// Allocate a fresh git worktree for an engineer pursuing `goal_id`.
    ///
    /// Branches off the parent repository's current `main` HEAD. **Fails
    /// loud** if `main` cannot be resolved — there is no fallback to `HEAD`,
    /// per the repo's no-fallback convention.
    pub fn allocate(
        parent_repo: &Path,
        state_root: &Path,
        goal_id: &str,
    ) -> Result<Self, SimardError> {
        // 1. Resolve the parent repo's `main` HEAD. No fallback.
        let main_sha = git_capture(parent_repo, &["rev-parse", "main"]).map_err(|reason| {
            SimardError::ActionExecutionFailed {
                action: format!("engineer_worktree::allocate(goal={goal_id})"),
                reason: format!("cannot resolve `main` in {}: {reason}", parent_repo.display()),
            }
        })?;
        let main_sha = main_sha.trim();
        if main_sha.is_empty() {
            return Err(SimardError::ActionExecutionFailed {
                action: format!("engineer_worktree::allocate(goal={goal_id})"),
                reason: format!(
                    "`git rev-parse main` returned empty output in {}",
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

        // 3. Ensure the worktrees root exists. The leaf dir must NOT exist
        //    yet — `git worktree add` creates it itself.
        fs::create_dir_all(&worktrees_root).map_err(|e| SimardError::ActionExecutionFailed {
            action: format!("engineer_worktree::allocate(goal={goal_id})"),
            reason: format!(
                "cannot create worktrees root {}: {e}",
                worktrees_root.display()
            ),
        })?;

        // 4. `git worktree add -b <branch> <dir> <main_sha>`
        let dir_str = dir.to_string_lossy().into_owned();
        let result = git_capture(
            parent_repo,
            &["worktree", "add", "-b", &branch, &dir_str, main_sha],
        );
        if let Err(reason) = result {
            // Best-effort cleanup of any partial state before failing loud.
            let _ = fs::remove_dir_all(&dir);
            let _ = git_capture(parent_repo, &["worktree", "prune"]);
            let _ = git_capture(parent_repo, &["branch", "-D", &branch]);
            return Err(SimardError::ActionExecutionFailed {
                action: format!("engineer_worktree::allocate(goal={goal_id})"),
                reason: format!("`git worktree add` failed: {reason}"),
            });
        }

        Ok(Self {
            path: dir,
            branch,
            parent_repo: parent_repo.to_path_buf(),
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
    pub fn cleanup(&self) -> Result<(), SimardError> {
        if self.cleaned.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        cleanup_inner(&self.parent_repo, &self.path, &self.branch);
        Ok(())
    }
}

impl Drop for EngineerWorktree {
    fn drop(&mut self) {
        if self.cleaned.swap(true, Ordering::SeqCst) {
            return;
        }
        cleanup_inner(&self.parent_repo, &self.path, &self.branch);
    }
}

/// Best-effort cleanup primitive shared by `cleanup()` and `Drop`.
///
/// Logs but never panics. Returns no error because both call sites have
/// already committed to "cleanup ran". Inspection happens via tracing.
fn cleanup_inner(parent_repo: &Path, dir: &Path, branch: &str) {
    let dir_str = dir.to_string_lossy().into_owned();
    if let Err(e) = git_capture(parent_repo, &["worktree", "remove", "--force", &dir_str]) {
        tracing::debug!(
            target: "simard::engineer_worktree",
            error = %e,
            worktree = %dir.display(),
            "git worktree remove failed (will fall back to manual rmdir+prune)",
        );
    }
    if let Err(e) = git_capture(parent_repo, &["worktree", "prune"]) {
        tracing::debug!(
            target: "simard::engineer_worktree",
            error = %e,
            "git worktree prune failed during cleanup",
        );
    }
    if dir.exists()
        && let Err(e) = fs::remove_dir_all(dir)
    {
        tracing::warn!(
            target: "simard::engineer_worktree",
            error = %e,
            worktree = %dir.display(),
            "failed to remove worktree dir from disk after `git worktree remove`",
        );
    }
    if let Err(e) = git_capture(parent_repo, &["branch", "-D", branch]) {
        tracing::debug!(
            target: "simard::engineer_worktree",
            error = %e,
            branch = %branch,
            "best-effort branch delete failed (branch may already be gone)",
        );
    }
}

/// Sweep `<state_root>/engineer-worktrees/` for orphans on daemon boot.
///
/// Runs `git worktree prune` first, then removes any directory in the
/// worktrees root that is not registered with the parent repository.
pub fn sweep_orphaned_worktrees(
    parent_repo: &Path,
    state_root: &Path,
) -> Result<SweepReport, SimardError> {
    let mut report = SweepReport::default();

    // Step 1: prune stale `.git/worktrees/` registrations from the parent.
    if let Err(reason) = git_capture(parent_repo, &["worktree", "prune"]) {
        return Err(SimardError::ActionExecutionFailed {
            action: "engineer_worktree::sweep_orphaned_worktrees".to_string(),
            reason: format!("`git worktree prune` failed: {reason}"),
        });
    }
    report.pruned_registrations = 1;

    // Step 2: enumerate currently-registered worktree paths.
    let listing = git_capture(parent_repo, &["worktree", "list", "--porcelain"]).map_err(
        |reason| SimardError::ActionExecutionFailed {
            action: "engineer_worktree::sweep_orphaned_worktrees".to_string(),
            reason: format!("`git worktree list` failed: {reason}"),
        },
    )?;
    let registered: Vec<PathBuf> = listing
        .lines()
        .filter_map(|l| l.strip_prefix("worktree ").map(PathBuf::from))
        .collect();

    // Step 3: walk the worktrees subdir and remove unregistered entries.
    let worktrees_root = state_root.join(WORKTREES_SUBDIR);
    if !worktrees_root.exists() {
        return Ok(report);
    }

    let entries = fs::read_dir(&worktrees_root).map_err(|e| SimardError::ActionExecutionFailed {
        action: "engineer_worktree::sweep_orphaned_worktrees".to_string(),
        reason: format!(
            "cannot read worktrees root {}: {e}",
            worktrees_root.display()
        ),
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Compare canonical paths so symlinks don't fool the sweep.
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        let is_registered = registered.iter().any(|r| {
            r == &path
                || r == &canonical
                || r.canonicalize().map(|c| c == canonical).unwrap_or(false)
        });
        if is_registered {
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
fn git_capture(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
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

/// Build a collision-resistant `<epoch_secs>-<6hex>` suffix.
///
/// Combines wall-clock nanos, an in-process atomic counter, the process
/// id, and a thread-local hash to survive same-second parallel allocations
/// without taking a `rand`/`uuid` dependency.
fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id() as u64;
    // Mix the fields with a cheap multiplicative hash so the 24 bits we keep
    // are well distributed even when nanos is small.
    let mix = nanos
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(counter.wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add(pid.wrapping_mul(0x94D0_49BB_1331_11EB));
    let hex = (mix & 0xFF_FFFF) as u32;
    format!("{secs}-{hex:06x}")
}
