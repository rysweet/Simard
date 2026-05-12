//! Error type for the safe-update orchestration.
//!
//! Kept separate from [`crate::error::SimardError`] so the safe-update
//! state machine has a small, focused error surface that callers can
//! match on phase-by-phase. The orchestrator itself converts these
//! into the appropriate banner / log message and decides whether to
//! roll back.

use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;

/// Errors emitted by the safe-update phases.
#[derive(Debug)]
pub enum SafeUpdateError {
    DrainIo {
        action: String,
        path: PathBuf,
        reason: String,
    },
    DrainTimeout {
        seconds: u64,
        in_flight: usize,
    },
    SnapshotIo {
        action: String,
        path: PathBuf,
        reason: String,
    },
    SnapshotVersionRead {
        path: PathBuf,
        reason: String,
    },
    PretestSpawn {
        path: PathBuf,
        reason: String,
    },
    PretestSelfTestFailed {
        code: Option<i32>,
        detail: String,
    },
    PretestTimeout {
        seconds: u64,
    },
    SwapFailed {
        reason: String,
    },
    SwapHandoverFailed {
        reason: String,
    },
    ValidateReadFailed {
        reason: String,
    },
    ValidateWriteFailed {
        reason: String,
    },
    ValidateMalformed {
        reason: String,
    },
    RollbackBackupMissing {
        path: PathBuf,
        reason: String,
    },
    RollbackBackupCorrupt,
    RollbackRestoreFailed {
        path: PathBuf,
        reason: String,
    },
    RollbackRestartFailed {
        reason: String,
    },
    LockBusy {
        path: PathBuf,
    },
    LockAcquireFailed {
        path: PathBuf,
        reason: String,
    },
}

impl Display for SafeUpdateError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::DrainIo {
                action,
                path,
                reason,
            } => write!(
                f,
                "drain phase i/o: {action} on {}: {reason}",
                path.display()
            ),
            Self::DrainTimeout { seconds, in_flight } => write!(
                f,
                "drain timed out after {seconds}s waiting for {in_flight} in-flight engineer(s)"
            ),
            Self::SnapshotIo {
                action,
                path,
                reason,
            } => write!(
                f,
                "snapshot phase i/o: {action} on {}: {reason}",
                path.display()
            ),
            Self::SnapshotVersionRead { path, reason } => write!(
                f,
                "snapshot phase: cannot read version from {}: {reason}",
                path.display()
            ),
            Self::PretestSpawn { path, reason } => write!(
                f,
                "pre-test phase: failed to spawn {}: {reason}",
                path.display()
            ),
            Self::PretestSelfTestFailed { code, detail } => write!(
                f,
                "pre-test phase: self-test exited non-zero ({code:?}): {detail}"
            ),
            Self::PretestTimeout { seconds } => {
                write!(f, "pre-test phase: self-test timed out after {seconds}s")
            }
            Self::SwapFailed { reason } => write!(
                f,
                "swap phase: rename failed (and copy fallback also failed): {reason}"
            ),
            Self::SwapHandoverFailed { reason } => {
                write!(f, "swap phase: handover/exec failed: {reason}")
            }
            Self::ValidateReadFailed { reason } => {
                write!(
                    f,
                    "validate phase: cannot read upgrade-status.json: {reason}"
                )
            }
            Self::ValidateWriteFailed { reason } => {
                write!(
                    f,
                    "validate phase: cannot write upgrade-status.json: {reason}"
                )
            }
            Self::ValidateMalformed { reason } => {
                write!(f, "validate phase: malformed upgrade-status.json: {reason}")
            }
            Self::RollbackBackupMissing { path, reason } => write!(
                f,
                "rollback phase: backup file is missing or unreadable at {}: {reason}",
                path.display()
            ),
            Self::RollbackBackupCorrupt => write!(
                f,
                "rollback phase: backup integrity check failed (sha256 mismatch); refusing to restore"
            ),
            Self::RollbackRestoreFailed { path, reason } => write!(
                f,
                "rollback phase: failed to restore install path {}: {reason}",
                path.display()
            ),
            Self::RollbackRestartFailed { reason } => {
                write!(
                    f,
                    "rollback phase: service restart command failed: {reason}"
                )
            }
            Self::LockBusy { path } => write!(
                f,
                "safe-update lock: another safe_self_update is in progress (lock at {})",
                path.display()
            ),
            Self::LockAcquireFailed { path, reason } => write!(
                f,
                "safe-update lock: cannot acquire lock at {}: {reason}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for SafeUpdateError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_timeout_is_displayed() {
        let e = SafeUpdateError::DrainTimeout {
            seconds: 10,
            in_flight: 2,
        };
        let s = e.to_string();
        assert!(s.contains("drain timed out"), "got: {s}");
        assert!(s.contains("10"));
        assert!(s.contains("2 in-flight"));
    }

    #[test]
    fn pretest_self_test_failed_is_displayed() {
        let e = SafeUpdateError::PretestSelfTestFailed {
            code: Some(1),
            detail: "boom".to_string(),
        };
        assert!(e.to_string().contains("self-test exited non-zero"));
    }

    #[test]
    fn rollback_backup_corrupt_is_displayed() {
        let e = SafeUpdateError::RollbackBackupCorrupt;
        assert!(e.to_string().contains("integrity check failed"));
    }
}
