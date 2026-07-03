//! Restart abstraction so the recipe and tests never restart a real daemon.
//!
//! Selecting the restarter is the **only** decision that differs between a live
//! operator deploy and an in-recipe dry run. See
//! `docs/reference/self-deploy-api.md#daemonrestarter`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{SimardError, SimardResult};

/// Default systemd unit name Simard's OODA daemon runs under.
pub const DEFAULT_OODA_UNIT: &str = "simard-ooda";
/// Operator override for the minimum interval between condition-free relaunches.
pub const SELF_RELAUNCH_MIN_INTERVAL_ENV: &str = "SIMARD_SELF_RELAUNCH_MIN_INTERVAL_SECS";
/// Default throttle: one interval-based relaunch per day. Real binary changes bypass it.
pub const DEFAULT_SELF_RELAUNCH_MIN_INTERVAL_SECS: u64 = 86_400;
const SELF_RELAUNCH_STATE_FILE: &str = "self-relaunch-state.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfRelaunchInterval {
    /// Disable interval-only relaunches; relaunch only when the binary changed.
    Off,
    /// Permit interval-only relaunches after this many seconds.
    Seconds(u64),
}

impl SelfRelaunchInterval {
    pub fn label(self) -> String {
        match self {
            Self::Off => "off".to_string(),
            Self::Seconds(secs) => format!("{secs}s"),
        }
    }
}

/// Parse the self-relaunch interval.
///
/// Empty, missing, or garbage values use the sane default. `0` and `off`
/// disable interval-only relaunches while still allowing real binary changes.
pub fn self_relaunch_min_interval_from_env(value: Option<&str>) -> SelfRelaunchInterval {
    let Some(raw) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return SelfRelaunchInterval::Seconds(DEFAULT_SELF_RELAUNCH_MIN_INTERVAL_SECS);
    };
    if raw.eq_ignore_ascii_case("off") || raw == "0" {
        return SelfRelaunchInterval::Off;
    }
    raw.parse::<u64>()
        .ok()
        .filter(|secs| *secs > 0)
        .map(SelfRelaunchInterval::Seconds)
        .unwrap_or(SelfRelaunchInterval::Seconds(
            DEFAULT_SELF_RELAUNCH_MIN_INTERVAL_SECS,
        ))
}

/// Pure decision: relaunch immediately for a real binary change, otherwise only
/// when the configured minimum interval has elapsed.
pub fn should_request_self_relaunch(
    now_secs: u64,
    last_relaunch_secs: Option<u64>,
    interval: SelfRelaunchInterval,
    binary_hash_changed: bool,
) -> bool {
    if binary_hash_changed {
        return true;
    }
    match interval {
        SelfRelaunchInterval::Off => false,
        SelfRelaunchInterval::Seconds(min_secs) => last_relaunch_secs
            .map(|last| now_secs.saturating_sub(last) >= min_secs)
            .unwrap_or(true),
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
struct SelfRelaunchState {
    last_relaunch_unix_secs: u64,
}

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

    fn should_restart_now(&self) -> bool {
        let state_root = crate::state_root::simard_state_root();
        let interval = self_relaunch_min_interval_from_env(
            std::env::var(SELF_RELAUNCH_MIN_INTERVAL_ENV)
                .ok()
                .as_deref(),
        );
        let now = now_unix_secs();
        let last = read_last_relaunch_secs(&state_root);
        let install_path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("simard"));
        let binary_hash_changed = binary_hash_changed_since_snapshot(
            &crate::safe_update::default_state_dir(),
            &install_path,
        );
        let should = should_request_self_relaunch(now, last, interval, binary_hash_changed);
        let last_label = last
            .map(|t| format!("{}s ago", now.saturating_sub(t)))
            .unwrap_or_else(|| "never".into());
        eprintln!(
            "[simard] self-relaunch: min interval = {} ({}={}, 0/off disables interval-only relaunch; binary-hash-changed={}; last={}; decision={})",
            interval.label(),
            SELF_RELAUNCH_MIN_INTERVAL_ENV,
            std::env::var(SELF_RELAUNCH_MIN_INTERVAL_ENV).unwrap_or_else(|_| "<default>".into()),
            binary_hash_changed,
            last_label,
            if should { "restart" } else { "skip" },
        );
        should
    }

    fn record_restart_requested(&self) {
        let state_root = crate::state_root::simard_state_root();
        let _ = write_last_relaunch_secs(&state_root, now_unix_secs());
    }
}

impl DaemonRestarter for SystemdOrExecRestarter {
    fn restart(&self) -> SimardResult<()> {
        if !self.should_restart_now() {
            return Ok(());
        }

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
                self.record_restart_requested();
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
        crate::self_relaunch::coordinated_relaunch(&semaphore_dir, &config).map(|_| {
            self.record_restart_requested();
        })
    }

    fn kind(&self) -> &'static str {
        "systemd-or-exec"
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn relaunch_state_path(state_root: &Path) -> PathBuf {
    state_root.join(SELF_RELAUNCH_STATE_FILE)
}

fn read_last_relaunch_secs(state_root: &Path) -> Option<u64> {
    let path = relaunch_state_path(state_root);
    let raw = std::fs::read(&path).ok()?;
    serde_json::from_slice::<SelfRelaunchState>(&raw)
        .ok()
        .map(|s| s.last_relaunch_unix_secs)
}

fn write_last_relaunch_secs(state_root: &Path, secs: u64) -> std::io::Result<()> {
    std::fs::create_dir_all(state_root)?;
    let body = serde_json::to_vec_pretty(&SelfRelaunchState {
        last_relaunch_unix_secs: secs,
    })
    .map_err(std::io::Error::other)?;
    std::fs::write(relaunch_state_path(state_root), body)
}

fn binary_hash_changed_since_snapshot(state_dir: &Path, install_path: &Path) -> bool {
    let Ok(Some(snapshot)) = crate::safe_update::snapshot::read_snapshot(state_dir) else {
        return false;
    };
    let Ok(current) = sha256_file(install_path) else {
        return false;
    };
    current != snapshot.sha256
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};

    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}
