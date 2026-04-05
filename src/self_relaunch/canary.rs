use std::path::{Path, PathBuf};
use std::process::Command;

use super::types::RelaunchConfig;
use crate::error::{SimardError, SimardResult};
use crate::self_relaunch_semaphore::{HandoffConfig, HandoffResult, LeaderSemaphore};

/// Build a canary binary via `cargo build --release` in a separate target directory.
pub fn build_canary(config: &RelaunchConfig) -> SimardResult<PathBuf> {
    let target_dir = &config.canary_target_dir;

    std::fs::create_dir_all(target_dir).map_err(|e| SimardError::PersistentStoreIo {
        store: "canary-build".to_string(),
        action: "create target directory".to_string(),
        path: target_dir.clone(),
        reason: e.to_string(),
    })?;

    let output = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--target-dir")
        .arg(target_dir)
        .arg("--manifest-path")
        .arg(config.manifest_dir.join("Cargo.toml"))
        .output()
        .map_err(|e| SimardError::BridgeSpawnFailed {
            bridge: "canary-build".to_string(),
            reason: format!("cargo build failed to start: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SimardError::BridgeCallFailed {
            bridge: "canary-build".to_string(),
            method: "cargo build --release".to_string(),
            reason: format!("build failed (exit {}): {}", output.status, stderr),
        });
    }

    let binary_path = target_dir.join("release").join("simard");
    if !binary_path.exists() {
        return Err(SimardError::ArtifactIo {
            path: binary_path,
            reason: "canary binary not found after successful build".to_string(),
        });
    }

    Ok(binary_path)
}

/// Validate preconditions and hand over execution to the canary binary.
///
/// On Unix, this uses `CommandExt::exec()` to replace the current process
/// image with the canary binary. This function does not return on success.
/// Returns error if pid is 0 or binary does not exist.
pub fn handover(current_pid: u32, canary_binary: &Path) -> SimardResult<()> {
    if current_pid == 0 {
        return Err(SimardError::BridgeCallFailed {
            bridge: "self-relaunch".to_string(),
            method: "handover".to_string(),
            reason: "current_pid cannot be 0".to_string(),
        });
    }

    if !canary_binary.exists() {
        return Err(SimardError::ArtifactIo {
            path: canary_binary.to_path_buf(),
            reason: "canary binary does not exist at handover time".to_string(),
        });
    }

    let metadata = std::fs::metadata(canary_binary).map_err(|e| SimardError::ArtifactIo {
        path: canary_binary.to_path_buf(),
        reason: format!("cannot read canary binary metadata: {e}"),
    })?;

    if !metadata.is_file() {
        return Err(SimardError::ArtifactIo {
            path: canary_binary.to_path_buf(),
            reason: "canary path is not a regular file".to_string(),
        });
    }

    // Replace the current process with the canary binary.
    // On Unix, exec() replaces the process image — this does not return on success.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = Command::new(canary_binary).exec();
        // exec() only returns on error.
        Err(SimardError::BridgeCallFailed {
            bridge: "self-relaunch".to_string(),
            method: "handover".to_string(),
            reason: format!("exec failed for '{}': {err}", canary_binary.display()),
        })
    }

    // On non-Unix platforms, spawn the canary and exit the current process.
    #[cfg(not(unix))]
    {
        Command::new(canary_binary)
            .spawn()
            .map_err(|e| SimardError::BridgeCallFailed {
                bridge: "self-relaunch".to_string(),
                method: "handover".to_string(),
                reason: format!("failed to spawn canary '{}': {e}", canary_binary.display()),
            })?;
        std::process::exit(0);
    }
}

/// Perform a coordinated self-relaunch using the leader semaphore.
///
/// This is the recommended relaunch path for production use. It:
/// 1. Acquires the leader semaphore (or confirms we already hold it)
/// 2. Delegates to [`coordinated_handoff`] which builds, gates, spawns, and transfers
/// 3. Returns the handoff result so the caller can shut down gracefully
///
/// Unlike [`handover`] which replaces the process image immediately,
/// this function keeps the old process alive until the new one is verified healthy.
pub fn coordinated_relaunch(
    semaphore_dir: &Path,
    config: &RelaunchConfig,
) -> SimardResult<HandoffResult> {
    let my_pid = std::process::id();
    let lock_path = semaphore_dir.join("simard-leader.lock");
    let semaphore = LeaderSemaphore::new(lock_path);

    // Ensure we are the leader before attempting handoff.
    semaphore.try_acquire(my_pid)?;

    let handoff_config = HandoffConfig::new(semaphore, config.clone());
    crate::self_relaunch_semaphore::coordinated_handoff(my_pid, &handoff_config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handover_rejects_zero_pid() {
        let err = handover(0, Path::new("/usr/bin/true")).unwrap_err();
        assert!(err.to_string().contains("current_pid"));
    }

    #[test]
    fn handover_rejects_missing_binary() {
        let err = handover(12345, Path::new("/tmp/no-such-canary-82719")).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn handover_rejects_directory_as_binary() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
        if dir.exists() {
            let err = handover(12345, &dir).unwrap_err();
            assert!(err.to_string().contains("not a regular file"), "{}", err);
        }
    }

    #[test]
    fn coordinated_relaunch_acquires_semaphore() {
        let dir = std::env::temp_dir().join(format!("simard-relaunch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = RelaunchConfig {
            canary_target_dir: PathBuf::from("/tmp/no-such-canary-dir"),
            manifest_dir: PathBuf::from("/tmp/no-such-manifest"),
            ..Default::default()
        };
        // coordinated_relaunch will acquire the semaphore, then fail at build_canary
        // because manifest_dir doesn't exist — that's fine, we're testing the wiring.
        let err = coordinated_relaunch(&dir, &config).unwrap_err();
        // The error should come from build_canary (not from semaphore acquisition).
        let msg = err.to_string();
        assert!(
            msg.contains("canary") || msg.contains("cargo") || msg.contains("build"),
            "expected build error, got: {msg}"
        );
        // Semaphore should have been acquired — verify the lock file exists.
        assert!(dir.join("simard-leader.lock").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
