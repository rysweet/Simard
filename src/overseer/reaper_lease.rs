//! Single-writer lease for the claim-reaper sweep (issue #4477).
//!
//! # Why
//! [`claim_reaper::reap_stale_claims`](crate::overseer::claim_reaper::reap_stale_claims)
//! mutates the shared `engineer_claims` ledger (release + worktree cleanup) with
//! NO mutual exclusion between co-resident `simard` daemons. When two daemons run
//! against the SAME host / state-root, both sweep the same ledger concurrently and
//! a naive daemon's `NoWorktree` immediate-reclaim can OVERRIDE another daemon's
//! careful in-flight investigate-before-reap — **false-reclaiming a live engineer
//! mid-recreate** (issue #4477).
//!
//! This module adds a **single-writer lease** the Overseer acquires BEFORE the
//! sweep and holds for its whole critical section, so at most one daemon per
//! state-root ever mutates the ledger at a time. Fail-closed: a daemon that does
//! NOT win the lease SKIPS the sweep this tick (never a partial, racy reclaim).
//!
//! # How
//! An advisory `flock(LOCK_EX | LOCK_NB)` on a sidecar lease file
//! (`<state_root>/claim-reaper.lease`), mirroring the established pattern in
//! [`cognitive_memory::open_guard`](crate::cognitive_memory). `flock` is tied to
//! the open file description, so two INDEPENDENT `open()`s of the same inode —
//! whether in two processes or two daemons — contend, and exactly one wins.
//! Non-blocking (`LOCK_NB`): a loser returns immediately rather than serialising
//! ticks. The lease is released by the kernel on `LOCK_UN` (RAII `Drop`) AND, as a
//! backstop, on process exit — so a crashed holder never wedges the sweep (this is
//! also the release-on-all-paths guarantee relevant to leaked-claim churn #4464).
//!
//! # Seam
//! The lease is injected as a trait ([`SingleWriterLease`]) so the reaper is
//! exercised hermetically: tests inject a fake that grants or withholds the lease
//! deterministically, with no real filesystem. Production wires
//! [`FlockReaperLease`].

use std::path::{Path, PathBuf};

/// RAII guard proving the holder currently owns the single-writer lease. Dropping
/// it releases the lease on EVERY exit path (normal return, early `?`, panic
/// unwind) — the release-on-completion guarantee the sweep's critical section
/// relies on.
pub trait SingleWriterGuard: Send {}

/// Injection seam: acquire the single-writer lease guarding the claim-reaper
/// sweep. `Send + Sync` because it is stored in the Overseer's reaper seam bundle
/// across ticks alongside the ledger/probe/cleanup seams.
///
/// Fail-closed contract: [`try_acquire`](SingleWriterLease::try_acquire) returns
/// `None` when the lease is held by ANOTHER writer, and the caller MUST then skip
/// the sweep — never proceed to mutate the ledger without the guard.
pub trait SingleWriterLease: Send + Sync {
    /// Try to acquire the lease WITHOUT blocking. `Some(guard)` ⇒ this writer won
    /// exclusive access for the guard's lifetime; `None` ⇒ another writer holds it
    /// (skip the sweep this tick).
    fn try_acquire(&self) -> Option<Box<dyn SingleWriterGuard>>;
}

/// A lease that ALWAYS grants access. The fail-safe default used when no
/// single-writer lease is wired (bare constructor / unit tests that do not
/// exercise concurrency) so behaviour is byte-for-byte the pre-#4477 sweep.
#[derive(Debug, Default, Clone, Copy)]
pub struct AlwaysAcquireLease;

/// The trivially-granted guard returned by [`AlwaysAcquireLease`]. Holds nothing;
/// dropping it is a no-op.
#[derive(Debug)]
struct AlwaysGuard;

impl SingleWriterGuard for AlwaysGuard {}

impl SingleWriterLease for AlwaysAcquireLease {
    fn try_acquire(&self) -> Option<Box<dyn SingleWriterGuard>> {
        Some(Box::new(AlwaysGuard))
    }
}

/// A lease that NEVER grants access — models a co-resident daemon that already
/// holds the single-writer lease. Used to exercise the fail-closed skip path of
/// the reaper's single-writer wrapper hermetically (no real filesystem).
#[derive(Debug, Default, Clone, Copy)]
pub struct NeverAcquireLease;

impl SingleWriterLease for NeverAcquireLease {
    fn try_acquire(&self) -> Option<Box<dyn SingleWriterGuard>> {
        None
    }
}

/// Production single-writer lease: an advisory `flock` on
/// `<state_root>/claim-reaper.lease`.
#[derive(Debug, Clone)]
pub struct FlockReaperLease {
    lease_path: PathBuf,
}

impl FlockReaperLease {
    /// Lease-file name under the state-root. A persistent zero-byte sidecar (never
    /// unlinked) so a concurrent opener cannot create a NEW inode and lock a
    /// different file — the classic lock-file + unlink race.
    pub const LEASE_FILE: &'static str = "claim-reaper.lease";

    /// Build a lease rooted at `state_root`. The lease file lives at
    /// `<state_root>/claim-reaper.lease` — the SAME root the engineers spawn under
    /// and the reaper sweeps, so every co-resident daemon contends on one inode.
    pub fn new(state_root: &Path) -> Self {
        Self {
            lease_path: state_root.join(Self::LEASE_FILE),
        }
    }
}

/// RAII guard holding the real `flock`. `Drop` issues `LOCK_UN`; the kernel also
/// drops the lock on process exit, so a crash can never wedge the sweep.
#[cfg(unix)]
#[derive(Debug)]
struct FlockGuard {
    file: std::fs::File,
    lease_path: PathBuf,
}

#[cfg(unix)]
impl SingleWriterGuard for FlockGuard {}

#[cfg(unix)]
impl Drop for FlockGuard {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // Release the advisory lock; leave the file on disk (see LEASE_FILE).
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
        tracing::debug!(
            target: "simard::claim_reaper",
            lease_path = %self.lease_path.display(),
            "[simard] claim-reaper single-writer lease released",
        );
    }
}

#[cfg(unix)]
impl SingleWriterLease for FlockReaperLease {
    fn try_acquire(&self) -> Option<Box<dyn SingleWriterGuard>> {
        use std::os::unix::io::AsRawFd;

        // Ensure the parent dir exists so the lease file can be created even on a
        // cold state-root. Best-effort: a create error simply fails the acquire
        // (fail-closed — the sweep is skipped this tick), never a panic.
        if let Some(parent) = self.lease_path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(
                target: "simard::claim_reaper",
                lease_path = %self.lease_path.display(),
                error = %error,
                "[simard] claim-reaper lease: could not create state-root dir; skipping sweep",
            );
            return None;
        }

        let file = match std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.lease_path)
        {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!(
                    target: "simard::claim_reaper",
                    lease_path = %self.lease_path.display(),
                    error = %error,
                    "[simard] claim-reaper lease: could not open lease file; skipping sweep",
                );
                return None;
            }
        };

        // Non-blocking exclusive lock. `0` ⇒ we won; anything else (EWOULDBLOCK on
        // contention, or any error) ⇒ fail-closed: another writer holds it, skip.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret == 0 {
            tracing::debug!(
                target: "simard::claim_reaper",
                lease_path = %self.lease_path.display(),
                "[simard] claim-reaper single-writer lease acquired",
            );
            Some(Box::new(FlockGuard {
                file,
                lease_path: self.lease_path.clone(),
            }))
        } else {
            tracing::info!(
                target: "simard::claim_reaper",
                lease_path = %self.lease_path.display(),
                "[simard] claim-reaper: another daemon holds the single-writer lease; \
                 skipping this sweep (fail-closed, no reclaim)",
            );
            None
        }
    }
}

// Non-unix fallback: no flock available, so the lease always grants (behaviour is
// identical to the pre-#4477 sweep). Simard is deployed on Linux; this keeps the
// crate buildable elsewhere without a real cross-process guarantee.
#[cfg(not(unix))]
impl SingleWriterLease for FlockReaperLease {
    fn try_acquire(&self) -> Option<Box<dyn SingleWriterGuard>> {
        Some(Box::new(AlwaysGuard))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_acquire_lease_always_grants() {
        let lease = AlwaysAcquireLease;
        assert!(lease.try_acquire().is_some());
        // Still grants while a prior guard is live (no exclusion — it is the
        // fail-safe default, not a real single-writer).
        let _g = lease.try_acquire().unwrap();
        assert!(lease.try_acquire().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn flock_lease_is_single_writer_second_acquire_is_denied() {
        let dir = std::env::temp_dir().join(format!("reaper-lease-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lease_a = FlockReaperLease::new(&dir);
        let lease_b = FlockReaperLease::new(&dir);

        // First writer wins the single-writer lease.
        let guard_a = lease_a.try_acquire();
        assert!(guard_a.is_some(), "first acquire must win the lease");

        // A SECOND, independent writer contending for the SAME state-root is
        // DENIED while the first holds it — the exact concurrent-daemon race in
        // issue #4477 (no false-reclaim override can occur).
        assert!(
            lease_b.try_acquire().is_none(),
            "second concurrent acquire must be denied (single-writer)",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn flock_lease_is_released_on_guard_drop_including_scope_exit() {
        let dir = std::env::temp_dir().join(format!("reaper-lease-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lease = FlockReaperLease::new(&dir);

        // Acquire and drop inside an inner scope — the RAII Drop must release the
        // lease on scope exit (the release-on-completion / all-paths guarantee).
        {
            let guard = lease.try_acquire();
            assert!(guard.is_some(), "acquire must win");
        }

        // After the guard dropped, the lease is free again: a fresh acquire wins.
        let reacquired = lease.try_acquire();
        assert!(
            reacquired.is_some(),
            "lease must be re-acquirable after the prior guard dropped (released on all paths)",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn flock_lease_creates_missing_state_root() {
        // A cold state-root (dir does not yet exist) must not fail the acquire:
        // the lease creates it, so a first-ever tick still gets single-writer
        // protection rather than fail-closing forever.
        let dir = std::env::temp_dir()
            .join(format!("reaper-lease-cold-{}", std::process::id()))
            .join("nested");
        let _ = std::fs::remove_dir_all(&dir);
        let lease = FlockReaperLease::new(&dir);
        let guard = lease.try_acquire();
        assert!(
            guard.is_some(),
            "acquire must create the missing state-root and win"
        );
        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
