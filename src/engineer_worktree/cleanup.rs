//! Cleanup primitive + path helpers used by allocate/drop.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::SimardError;

use super::sweep::git_capture;
use super::worktree_mutation_lock;

/// Cleanup primitive shared by `cleanup()` and `Drop`.
///
/// Hard errors (canonical-prefix guard rejection, dir removal failure)
/// propagate. Best-effort git invocations log at WARN/DEBUG and continue —
/// they describe registry state that the next `worktree prune` will
/// reconcile, so they should not abort cleanup of the on-disk dir.
pub fn cleanup_inner(
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

/// Canonicalize `dir` and verify it lives under `root_canonical`.
/// Returns the canonicalized path so the caller can `fs::remove_dir_all`
/// on the same path that was just verified (avoiding a TOCTOU between
/// check and use through the symlink-resolved view).
pub fn assert_under_root(dir: &Path, root_canonical: &Path) -> Result<PathBuf, String> {
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
pub fn create_worktrees_root(root: &Path) -> std::io::Result<()> {
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
pub fn unique_suffix() -> String {
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
