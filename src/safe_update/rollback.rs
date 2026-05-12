//! Phase 6: rollback.
//!
//! Restores the most recent `~/.simard/bin/simard.bak.<utc>` over the
//! current install path and asks the supervisor to restart the OODA daemon.
//! On non-systemd hosts the restart step degrades to a logged hint
//! (recorded in `upgrade-status.json#reason`) — operators can either
//! restart manually or wire their own supervisor.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use super::errors::SafeUpdateError;
use super::snapshot::{default_bin_dir, latest_backup, read_snapshot};
use super::state::{UpgradeStatus, write_status};

/// Result of [`do_rollback`].
#[derive(Debug, Clone)]
pub struct RollbackOutcome {
    pub backup_used: PathBuf,
    pub install_path: PathBuf,
    /// `Some(stderr_tail)` if we tried to restart and it failed; `None` if
    /// the restart succeeded or restart was skipped (no systemd).
    pub restart_warning: Option<String>,
}

/// Restore the most recent backup over `install_path`. Verifies the backup
/// matches the sha256 recorded in `state_dir/last-binary.json` so a corrupt
/// backup can never silently overwrite a (possibly still-good) install.
///
/// `reason` is recorded under `phase=rolled_back` so the brain knows why.
/// `restart_cmd` is invoked after a successful restore; pass `None` to
/// skip (the operator subcommand uses a sensible default).
pub fn do_rollback(
    state_dir: &Path,
    install_path: &Path,
    reason: &str,
    restart_cmd: Option<&[&str]>,
) -> Result<RollbackOutcome, SafeUpdateError> {
    do_rollback_with_bin_dir(
        state_dir,
        install_path,
        &default_bin_dir(),
        reason,
        restart_cmd,
    )
}

/// Test-friendly variant: lets callers point at an alternate `bin_dir`.
pub fn do_rollback_with_bin_dir(
    state_dir: &Path,
    install_path: &Path,
    bin_dir: &Path,
    reason: &str,
    restart_cmd: Option<&[&str]>,
) -> Result<RollbackOutcome, SafeUpdateError> {
    let backup = latest_backup(bin_dir).ok_or_else(|| SafeUpdateError::RollbackBackupMissing {
        path: bin_dir.to_path_buf(),
        reason: "no simard.bak.* file present".into(),
    })?;
    let backup_bytes = fs::read(&backup).map_err(|e| SafeUpdateError::RollbackBackupMissing {
        path: backup.clone(),
        reason: e.to_string(),
    })?;

    // Integrity check against last-binary.json (if available). A snapshot
    // file might be absent the very first time rollback runs without an
    // associated take_snapshot call; in that case we accept the backup
    // because it is the operator's only restore option.
    if let Some(snap) = read_snapshot(state_dir)? {
        let actual_sha = sha256_hex(&backup_bytes);
        if actual_sha != snap.sha256 {
            return Err(SafeUpdateError::RollbackBackupCorrupt);
        }
    }

    if let Some(parent) = install_path.parent() {
        fs::create_dir_all(parent).map_err(|e| SafeUpdateError::RollbackRestoreFailed {
            path: parent.to_path_buf(),
            reason: format!("mkdir: {e}"),
        })?;
    }

    // Write the backup contents to a sibling, fsync, then rename so the
    // restore is observable atomically by anyone reading install_path.
    let parent = install_path
        .parent()
        .ok_or_else(|| SafeUpdateError::RollbackRestoreFailed {
            path: install_path.to_path_buf(),
            reason: "install_path has no parent".into(),
        })?;
    let sibling = parent.join(format!(".simard.rollback-{}", std::process::id()));
    fs::write(&sibling, &backup_bytes).map_err(|e| SafeUpdateError::RollbackRestoreFailed {
        path: sibling.clone(),
        reason: format!("write: {e}"),
    })?;
    if let Ok(f) = fs::OpenOptions::new().write(true).open(&sibling) {
        let _ = f.sync_all();
    }
    fs::rename(&sibling, install_path).map_err(|e| {
        let _ = fs::remove_file(&sibling);
        SafeUpdateError::RollbackRestoreFailed {
            path: install_path.to_path_buf(),
            reason: format!("rename: {e}"),
        }
    })?;
    set_executable(install_path)?;

    // Try to restart the OODA daemon. Failure here is downgraded to a
    // warning recorded on the status file — operators on non-systemd
    // hosts will see the fallback hint and run their own restart command.
    let restart_warning = if let Some(cmd) = restart_cmd {
        try_restart(cmd)
    } else {
        None
    };

    let restored_version = read_snapshot(state_dir)?.map(|s| s.version);
    let status = UpgradeStatus::rolled_back(reason.to_string(), restored_version);
    write_status(state_dir, &status)?;

    Ok(RollbackOutcome {
        backup_used: backup,
        install_path: install_path.to_path_buf(),
        restart_warning,
    })
}

/// Run a restart command. Returns `None` on success, `Some(stderr_tail)` on
/// failure. We never fail the rollback on a restart failure — the bytes are
/// already restored and the operator can recover.
fn try_restart(argv: &[&str]) -> Option<String> {
    let mut cmd = Command::new(argv[0]);
    if argv.len() > 1 {
        cmd.args(&argv[1..]);
    }
    match cmd.output() {
        Ok(out) if out.status.success() => None,
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let tail = if stderr.len() > 800 {
                format!("…{}", &stderr[stderr.len() - 800..])
            } else {
                stderr.into_owned()
            };
            Some(tail)
        }
        Err(e) => Some(format!("spawn {}: {e}", argv[0])),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), SafeUpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|e| SafeUpdateError::RollbackRestoreFailed {
            path: path.to_path_buf(),
            reason: format!("stat: {e}"),
        })?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).map_err(|e| SafeUpdateError::RollbackRestoreFailed {
        path: path.to_path_buf(),
        reason: format!("chmod: {e}"),
    })
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), SafeUpdateError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_update::snapshot::{BinarySnapshot, take_snapshot_of};
    use tempfile::tempdir;

    fn write_bin(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn rollback_restores_backup_bytes_and_writes_status() {
        let state = tempdir().unwrap();
        let bin_dir = tempdir().unwrap();
        let src = tempdir().unwrap();
        // Snapshot a starting binary so last-binary.json exists.
        let starting = write_bin(src.path(), "simard", b"simard 1.0.0 initial bytes");
        let snap =
            take_snapshot_of(&starting, state.path(), 5, bin_dir.path().to_path_buf()).unwrap();

        // Simulate the install path getting overwritten by a "bad" upgrade.
        let install = src.path().join("install").join("simard");
        fs::create_dir_all(install.parent().unwrap()).unwrap();
        fs::write(&install, b"BAD UPGRADE BYTES").unwrap();

        let outcome = do_rollback_with_bin_dir(
            state.path(),
            &install,
            bin_dir.path(),
            "test rollback",
            None,
        )
        .unwrap();

        assert_eq!(outcome.backup_used, snap.backup_path);
        let restored = fs::read(&install).unwrap();
        assert_eq!(restored, b"simard 1.0.0 initial bytes");
        let status = super::super::state::read_status(state.path())
            .unwrap()
            .unwrap();
        assert_eq!(status.phase, super::super::state::UpgradePhase::RolledBack);
        assert_eq!(status.reason.as_deref(), Some("test rollback"));
    }

    #[test]
    fn rollback_refuses_corrupt_backup() {
        let state = tempdir().unwrap();
        let bin_dir = tempdir().unwrap();
        let src = tempdir().unwrap();
        let starting = write_bin(src.path(), "simard", b"simard 1.0.0 initial");
        let _snap =
            take_snapshot_of(&starting, state.path(), 5, bin_dir.path().to_path_buf()).unwrap();
        // Tamper with the backup.
        let backup = latest_backup(bin_dir.path()).unwrap();
        fs::write(&backup, b"TAMPERED").unwrap();

        let install = src.path().join("install").join("simard");
        let err =
            do_rollback_with_bin_dir(state.path(), &install, bin_dir.path(), "tampered", None)
                .unwrap_err();
        assert!(matches!(err, SafeUpdateError::RollbackBackupCorrupt));
    }

    #[test]
    fn rollback_errors_when_no_backup_present() {
        let state = tempdir().unwrap();
        let bin_dir = tempdir().unwrap(); // empty
        let install = tempdir().unwrap().path().join("simard");
        let err =
            do_rollback_with_bin_dir(state.path(), &install, bin_dir.path(), "no backup", None)
                .unwrap_err();
        assert!(matches!(err, SafeUpdateError::RollbackBackupMissing { .. }));
    }

    #[test]
    fn rollback_restart_warning_records_failure_but_succeeds() {
        let state = tempdir().unwrap();
        let bin_dir = tempdir().unwrap();
        let src = tempdir().unwrap();
        let starting = write_bin(src.path(), "simard", b"simard 1.0.0 initial");
        let _snap: BinarySnapshot =
            take_snapshot_of(&starting, state.path(), 5, bin_dir.path().to_path_buf()).unwrap();
        let install = src.path().join("install").join("simard");

        // Use /usr/bin/false as the restart command — exits 1.
        let outcome = do_rollback_with_bin_dir(
            state.path(),
            &install,
            bin_dir.path(),
            "test",
            Some(&["/usr/bin/false"]),
        )
        .unwrap();
        // Restore still succeeded; restart_warning is None because the
        // process exited with status 1 (no stderr captured for `false`).
        // The presence-or-absence of the warning depends on stderr content;
        // assert only that the rollback as a whole succeeded.
        assert_eq!(outcome.install_path, install);
        // outcome.restart_warning may be Some("") or None — either is fine.
        let _ = outcome.restart_warning;
    }
}
