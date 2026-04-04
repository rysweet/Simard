//! Canary deployment and handover for self-relaunch.
//!
//! Gate sequence: Smoke -> UnitTest -> GymBaseline -> BridgeHealth.
//! All gates must pass before handover. Failures reject the canary (Pillar 11).
//!
//! For coordinated multi-process handoff with leader election, see
//! [`coordinated_relaunch`] which uses [`self_relaunch_semaphore`].

use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::error::{SimardError, SimardResult};
use crate::self_relaunch_semaphore::{HandoffConfig, HandoffResult, LeaderSemaphore};

#[derive(Clone, Debug)]
pub struct RelaunchConfig {
    pub canary_target_dir: PathBuf,
    pub health_timeout: Duration,
    pub manifest_dir: PathBuf,
}

impl Default for RelaunchConfig {
    fn default() -> Self {
        Self {
            canary_target_dir: PathBuf::from("/tmp/simard-canary"),
            health_timeout: Duration::from_secs(30),
            manifest_dir: PathBuf::from("."),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelaunchGate {
    Smoke,
    UnitTest,
    GymBaseline,
    BridgeHealth,
}

impl Display for RelaunchGate {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Smoke => "smoke",
            Self::UnitTest => "unit-test",
            Self::GymBaseline => "gym-baseline",
            Self::BridgeHealth => "bridge-health",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Debug)]
pub struct GateResult {
    pub gate: RelaunchGate,
    pub passed: bool,
    pub detail: String,
}

impl Display for GateResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let status = if self.passed { "PASS" } else { "FAIL" };
        write!(f, "[{}] {}: {}", status, self.gate, self.detail)
    }
}

pub fn default_gates() -> Vec<RelaunchGate> {
    vec![
        RelaunchGate::Smoke,
        RelaunchGate::UnitTest,
        RelaunchGate::GymBaseline,
        RelaunchGate::BridgeHealth,
    ]
}

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

/// Verify a canary binary against a sequence of gates (does not short-circuit).
pub fn verify_canary(
    binary: &Path,
    gates: &[RelaunchGate],
    config: &RelaunchConfig,
) -> SimardResult<Vec<GateResult>> {
    let mut results = Vec::with_capacity(gates.len());

    for &gate in gates {
        let result = run_gate(binary, gate, config);
        results.push(result);
    }

    Ok(results)
}

pub fn all_gates_passed(results: &[GateResult]) -> bool {
    results.iter().all(|r| r.passed)
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

fn run_gate(binary: &Path, gate: RelaunchGate, config: &RelaunchConfig) -> GateResult {
    match gate {
        RelaunchGate::Smoke => run_smoke_gate(binary),
        RelaunchGate::UnitTest => run_unit_test_gate(config),
        RelaunchGate::GymBaseline => run_gym_baseline_gate(binary),
        RelaunchGate::BridgeHealth => run_bridge_health_gate(binary, config),
    }
}

fn run_smoke_gate(binary: &Path) -> GateResult {
    match Command::new(binary).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            GateResult {
                gate: RelaunchGate::Smoke,
                passed: true,
                detail: format!("version: {}", stdout.trim()),
            }
        }
        Ok(output) => GateResult {
            gate: RelaunchGate::Smoke,
            passed: false,
            detail: format!(
                "binary exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        },
        Err(e) => GateResult {
            gate: RelaunchGate::Smoke,
            passed: false,
            detail: format!("failed to execute binary: {e}"),
        },
    }
}

fn run_unit_test_gate(config: &RelaunchConfig) -> GateResult {
    match Command::new("cargo")
        .arg("test")
        .arg("--manifest-path")
        .arg(config.manifest_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&config.canary_target_dir)
        .env("CARGO_BUILD_JOBS", "2")
        .output()
    {
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: true,
            detail: "all tests passed".to_string(),
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let truncated = truncate_output(&stderr, 200);
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("tests failed (exit {}): {}", output.status, truncated),
            }
        }
        Err(e) => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: false,
            detail: format!("cargo test failed to run: {e}"),
        },
    }
}

fn run_gym_baseline_gate(binary: &Path) -> GateResult {
    match Command::new(binary).args(["gym", "list"]).output() {
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::GymBaseline,
            passed: true,
            detail: "gym list succeeded".to_string(),
        },
        Ok(output) => GateResult {
            gate: RelaunchGate::GymBaseline,
            passed: false,
            detail: format!(
                "gym probe failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        },
        Err(e) => GateResult {
            gate: RelaunchGate::GymBaseline,
            passed: false,
            detail: format!("gym probe failed to run: {e}"),
        },
    }
}

fn run_bridge_health_gate(binary: &Path, config: &RelaunchConfig) -> GateResult {
    let timeout_secs = config.health_timeout.as_secs().to_string();
    match Command::new(binary)
        .args(["probe", "bridge", "--timeout", &timeout_secs])
        .output()
    {
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::BridgeHealth,
            passed: true,
            detail: "bridge health check passed".to_string(),
        },
        Ok(output) => GateResult {
            gate: RelaunchGate::BridgeHealth,
            passed: false,
            detail: format!(
                "bridge health failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        },
        Err(e) => GateResult {
            gate: RelaunchGate::BridgeHealth,
            passed: false,
            detail: format!("bridge health probe failed to run: {e}"),
        },
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

fn truncate_output(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.trim().to_string()
    } else {
        // Use char-boundary-safe truncation to avoid panic on multi-byte UTF-8.
        let boundary = s
            .char_indices()
            .take_while(|(i, _)| *i < max_len)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}...", s[..boundary].trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunch_gate_display() {
        assert_eq!(RelaunchGate::Smoke.to_string(), "smoke");
        assert_eq!(RelaunchGate::BridgeHealth.to_string(), "bridge-health");
    }

    #[test]
    fn default_gates_has_all_four() {
        let gates = default_gates();
        assert_eq!(gates.len(), 4);
        assert_eq!(gates[0], RelaunchGate::Smoke);
        assert_eq!(gates[3], RelaunchGate::BridgeHealth);
    }

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
    fn smoke_gate_handles_missing_binary() {
        let result = run_smoke_gate(Path::new("/tmp/no-such-binary-48291"));
        assert!(!result.passed);
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
