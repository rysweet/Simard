//! Restart abstraction so the recipe and tests never restart a real daemon.
//!
//! Selecting the restarter is the **only** decision that differs between a live
//! operator deploy and an in-recipe dry run. See
//! `docs/reference/self-deploy-api.md#daemonrestarter`.

use std::process::Command;
use std::sync::Mutex;

use crate::error::{SimardError, SimardResult};

/// Default systemd unit name Simard's OODA daemon runs under.
pub const DEFAULT_OODA_UNIT: &str = "simard-ooda";

/// Restarts the daemon after a binary swap.
pub trait DaemonRestarter: Send + Sync {
    /// Restart the daemon. Returns once the restart has been requested.
    fn restart(&self) -> SimardResult<()>;
    /// Human-readable name for logs (e.g. "systemd", "exec-handover", "fake").
    fn kind(&self) -> &'static str;
}

/// Test/recipe restarter. Records the call count and performs no real restart.
///
/// `FakeRestarter::failing()` simulates a restart that cannot be requested, so
/// the orchestrator's rollback path can be exercised.
#[derive(Default)]
pub struct FakeRestarter {
    calls: Mutex<usize>,
    fail: bool,
}

impl FakeRestarter {
    pub fn new() -> Self {
        Self::default()
    }

    /// A restarter whose [`restart`](DaemonRestarter::restart) always errors.
    pub fn failing() -> Self {
        Self {
            calls: Mutex::new(0),
            fail: true,
        }
    }

    /// Number of times [`restart`](DaemonRestarter::restart) has been called.
    pub fn restart_count(&self) -> usize {
        *self.calls.lock().expect("FakeRestarter mutex poisoned")
    }
}

impl DaemonRestarter for FakeRestarter {
    fn restart(&self) -> SimardResult<()> {
        *self.calls.lock().expect("FakeRestarter mutex poisoned") += 1;
        if self.fail {
            return Err(SimardError::VerificationFailed {
                reason: "FakeRestarter forced restart failure".to_string(),
            });
        }
        Ok(())
    }

    fn kind(&self) -> &'static str {
        "fake"
    }
}

/// Production restarter. Prefers `systemctl --user restart simard-ooda` when the
/// unit is detected; otherwise falls back to the coordinated `exec()` handover
/// (`self_relaunch::coordinated_relaunch`).
pub struct SystemdOrExecRestarter {
    /// systemd unit to restart (default [`DEFAULT_OODA_UNIT`]).
    unit: String,
}

impl Default for SystemdOrExecRestarter {
    fn default() -> Self {
        Self {
            unit: DEFAULT_OODA_UNIT.to_string(),
        }
    }
}

impl SystemdOrExecRestarter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restart against an explicit systemd unit name.
    pub fn with_unit(unit: impl Into<String>) -> Self {
        Self { unit: unit.into() }
    }

    /// Is a user systemd unit named `self.unit` present and known to systemd?
    /// `systemctl --user is-enabled <unit>` exits 0 for enabled units; some
    /// states (e.g. `static`) exit non-zero but still print a recognised state,
    /// so we treat "command ran and did not say `Failed to get unit`" as present.
    fn systemd_unit_present(&self) -> bool {
        let output = Command::new("systemctl")
            .args(["--user", "is-enabled", &self.unit])
            .output();
        match output {
            Ok(out) => {
                if out.status.success() {
                    return true;
                }
                // Non-zero can still mean "known but not enabled" (static,
                // disabled). Only an explicit "not found" means absent.
                let combined = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                )
                .to_ascii_lowercase();
                !combined.contains("not found") && !combined.contains("no such")
            }
            Err(_) => false, // systemctl not installed / no user bus
        }
    }
}

impl DaemonRestarter for SystemdOrExecRestarter {
    fn restart(&self) -> SimardResult<()> {
        // Primary: systemd restart when the unit exists.
        if self.systemd_unit_present() {
            let out = Command::new("systemctl")
                .args(["--user", "restart", &self.unit])
                .output()
                .map_err(|e| SimardError::VerificationFailed {
                    reason: format!(
                        "failed to spawn `systemctl --user restart {}`: {e}",
                        self.unit
                    ),
                })?;
            if out.status.success() {
                return Ok(());
            }
            return Err(SimardError::VerificationFailed {
                reason: format!(
                    "`systemctl --user restart {}` exited {}: {}",
                    self.unit,
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            });
        }

        // Fallback: coordinated exec() handover (the daemon's own relaunch path).
        // Build + gate a canary from source and hand off with leader election so
        // the old process stays up until the new one is verified healthy.
        let semaphore_dir = crate::state_root::simard_state_root();
        let config = crate::self_relaunch::RelaunchConfig::default();
        crate::self_relaunch::coordinated_relaunch(&semaphore_dir, &config).map(|_| ())
    }

    fn kind(&self) -> &'static str {
        "systemd-or-exec"
    }
}
