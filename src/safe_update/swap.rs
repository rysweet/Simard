//! Phase 4: atomic swap and exec handover.
//!
//! Replaces the live install path with the validated candidate and then
//! exec()s into it. The replacement is `rename(2)` first (atomic on the
//! same filesystem) with a copy-then-rename fallback for cross-filesystem
//! installs. The new binary inherits 0755 permissions.
//!
//! Before exec()ing, the orchestrator writes
//! `state_dir/upgrade-status.json` with `phase=exec_handover` so the
//! incoming binary's startup hook can recognise the handover and enter
//! validation mode.

use std::fs;
use std::path::{Path, PathBuf};

use super::errors::SafeUpdateError;
use super::snapshot::BinarySnapshot;
use super::state::{UpgradeStatus, write_status};

/// Result of the swap phase. Only ever observed in tests, since on a
/// successful real upgrade the call to [`crate::self_relaunch::handover`]
/// replaces the process image.
#[derive(Debug, Clone)]
pub struct SwapOutcome {
    /// Final live install path after the rename.
    pub install_path: PathBuf,
    /// Whether we used `rename(2)` directly (atomic) or had to fall back
    /// to copy-then-rename (cross-filesystem path).
    pub atomic_rename_used: bool,
}

/// Drive the swap phase. On success in a real run this exec()s into the
/// new binary and **does not return**.
pub fn do_swap(
    new_bin: &Path,
    install_path: &Path,
    state_dir: &Path,
    snapshot: &BinarySnapshot,
    validate_required_cycles: u32,
    validate_budget_seconds: u64,
) -> Result<SwapOutcome, SafeUpdateError> {
    let outcome = atomic_install(new_bin, install_path)?;

    let new_version = read_version_from_binary(install_path);

    let status = UpgradeStatus::exec_handover(
        Some(new_version),
        Some(snapshot.version.clone()),
        validate_required_cycles,
        validate_budget_seconds,
    );
    write_status(state_dir, &status)?;

    if !test_skip_handover() {
        let pid = std::process::id();
        crate::self_relaunch::handover(pid, install_path).map_err(|e| {
            SafeUpdateError::SwapHandoverFailed {
                reason: e.to_string(),
            }
        })?;
        // handover() does not return on Unix; we should never reach here.
    }
    Ok(outcome)
}

/// Replace `install_path` with the contents of `new_bin`. Uses rename for
/// atomicity when same-filesystem; falls back to copy+sync+rename when
/// rename returns EXDEV. Always sets 0755 on the result.
pub fn atomic_install(new_bin: &Path, install_path: &Path) -> Result<SwapOutcome, SafeUpdateError> {
    if !new_bin.exists() {
        return Err(SafeUpdateError::SwapFailed {
            reason: format!("candidate binary missing: {}", new_bin.display()),
        });
    }
    if let Some(parent) = install_path.parent() {
        fs::create_dir_all(parent).map_err(|e| SafeUpdateError::SwapFailed {
            reason: format!("mkdir {}: {e}", parent.display()),
        })?;
    }

    // Try the ideal path: rename. Atomic on the same filesystem.
    match fs::rename(new_bin, install_path) {
        Ok(()) => {
            set_executable(install_path)?;
            Ok(SwapOutcome {
                install_path: install_path.to_path_buf(),
                atomic_rename_used: true,
            })
        }
        Err(_) => {
            // Cross-filesystem (EXDEV) or stale-target: copy to sibling, fsync, rename.
            let parent = install_path
                .parent()
                .ok_or_else(|| SafeUpdateError::SwapFailed {
                    reason: "install_path has no parent".into(),
                })?;
            let sibling = parent.join(format!(".simard.swap-{}", std::process::id()));
            fs::copy(new_bin, &sibling).map_err(|e| SafeUpdateError::SwapFailed {
                reason: format!(
                    "copy fallback {} -> {}: {e}",
                    new_bin.display(),
                    sibling.display()
                ),
            })?;
            // fsync the file to make sure the bytes are durable before we
            // rename it into the live position.
            if let Ok(f) = fs::OpenOptions::new().write(true).open(&sibling) {
                let _ = f.sync_all();
            }
            fs::rename(&sibling, install_path).map_err(|e| {
                let _ = fs::remove_file(&sibling);
                SafeUpdateError::SwapFailed {
                    reason: format!(
                        "rename {} -> {}: {e}",
                        sibling.display(),
                        install_path.display()
                    ),
                }
            })?;
            set_executable(install_path)?;
            Ok(SwapOutcome {
                install_path: install_path.to_path_buf(),
                atomic_rename_used: false,
            })
        }
    }
}

/// Tests set `SIMARD_SAFE_UPDATE_SKIP_HANDOVER=1` to exercise [`do_swap`]
/// without actually exec()ing into the candidate.
fn test_skip_handover() -> bool {
    std::env::var("SIMARD_SAFE_UPDATE_SKIP_HANDOVER")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Best-effort version sniff: scans the binary for a "simard <semver>"
/// substring (the format `simard --version` prints). Returns the running
/// crate version if no embedded string is found — matching
/// [`super::snapshot::read_embedded_version`]'s policy.
fn read_version_from_binary(path: &Path) -> String {
    if let Ok(bytes) = fs::read(path) {
        let needle = b"simard ";
        if let Some(pos) = bytes.windows(needle.len()).position(|w| w == needle) {
            let tail = &bytes[pos + needle.len()..];
            let end = tail.iter().position(|&b| !is_version_char(b)).unwrap_or(0);
            if end >= 5 {
                return String::from_utf8_lossy(&tail[..end]).into_owned();
            }
        }
    }
    env!("CARGO_PKG_VERSION").to_string()
}

fn is_version_char(b: u8) -> bool {
    b.is_ascii_digit() || b == b'.' || b == b'-' || b.is_ascii_alphabetic()
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), SafeUpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|e| SafeUpdateError::SwapFailed {
            reason: format!("stat {}: {e}", path.display()),
        })?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).map_err(|e| SafeUpdateError::SwapFailed {
        reason: format!("chmod {}: {e}", path.display()),
    })
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), SafeUpdateError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn atomic_install_replaces_target_with_new_bytes() {
        let dir = tempdir().unwrap();
        let new_bin = dir.path().join("new");
        let install = dir.path().join("simard");
        fs::write(&new_bin, b"NEW BYTES").unwrap();
        fs::write(&install, b"OLD BYTES").unwrap();
        let outcome = atomic_install(&new_bin, &install).unwrap();
        assert!(outcome.atomic_rename_used);
        let after = fs::read(&install).unwrap();
        assert_eq!(after, b"NEW BYTES");
        // The candidate path was consumed by the rename.
        assert!(!new_bin.exists());
    }

    #[test]
    fn atomic_install_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let new_bin = dir.path().join("new");
        let install = dir.path().join("nested").join("simard");
        fs::write(&new_bin, b"NEW").unwrap();
        atomic_install(&new_bin, &install).unwrap();
        assert!(install.exists());
    }

    #[test]
    fn atomic_install_rejects_missing_candidate() {
        let dir = tempdir().unwrap();
        let install = dir.path().join("simard");
        let err = atomic_install(&dir.path().join("nope"), &install).unwrap_err();
        assert!(matches!(err, SafeUpdateError::SwapFailed { .. }));
    }

    #[test]
    fn do_swap_writes_exec_handover_status_when_handover_skipped() {
        // Force the handover to be skipped so the test can observe state_dir.
        unsafe {
            std::env::set_var("SIMARD_SAFE_UPDATE_SKIP_HANDOVER", "1");
        }
        let dir = tempdir().unwrap();
        let state = tempdir().unwrap();
        let new_bin = dir.path().join("new");
        let install = dir.path().join("simard");
        // Embed a version-looking string for the sniff.
        fs::write(&new_bin, b"simard 9.9.9 hello\nrest").unwrap();
        let snap = BinarySnapshot {
            binary_path: install.clone(),
            sha256: "deadbeef".into(),
            mtime: "unknown".into(),
            version: "9.9.8".into(),
            backup_path: dir.path().join("simard.bak.x"),
            captured_at: "now".into(),
        };
        do_swap(&new_bin, &install, state.path(), &snap, 5, 600).unwrap();
        let status = super::super::state::read_status(state.path())
            .unwrap()
            .unwrap();
        assert_eq!(
            status.phase,
            super::super::state::UpgradePhase::ExecHandover
        );
        assert_eq!(status.new_version.as_deref(), Some("9.9.9"));
        assert_eq!(status.previous_version.as_deref(), Some("9.9.8"));
        unsafe {
            std::env::remove_var("SIMARD_SAFE_UPDATE_SKIP_HANDOVER");
        }
    }
}
