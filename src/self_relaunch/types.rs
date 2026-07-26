use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct RelaunchConfig {
    pub canary_target_dir: PathBuf,
    pub health_timeout: Duration,
    pub manifest_dir: PathBuf,
    /// Allow-list of environment variable NAMES that gate subprocesses may
    /// inherit from the daemon's ambient environment, on top of the always
    /// re-injected base set required for the gates to run at all (see
    /// `scrub_gate_env` in `gates.rs`). Names only — the values are read from
    /// the live environment at spawn time, never persisted here or logged. Empty
    /// by default: gates then see only the deny-by-default base env floor.
    ///
    /// This is the additive knob (#4440) that lets an operator hand a gate the
    /// one extra variable a healthy candidate legitimately needs — established
    /// empirically from the #4420 `failing_gate`/`failing_detail` diagnostics —
    /// without widening the base floor or inheriting the daemon's whole ambient
    /// env (which could hijack a gate or drift the canary away from the deployed
    /// systemd shape, the observed red-canary non-convergence).
    pub canary_env: Vec<String>,
    /// Bounded retry budget for the RpcHealth probe (issue
    /// `process:self_deploy_blocked`). A TRANSIENT probe fault (a wedged daemon
    /// that timed out, or a socket not yet listening) is retried up to this many
    /// attempts before the gate reddens; DETERMINISTIC faults (empty stats, a
    /// non-zero exit) are never retried. Defaults to `3` — bounded and positive.
    pub health_probe_max_attempts: usize,
    /// Base backoff between RpcHealth probe attempts (capped-exponential). Bounds
    /// total retry wait; a zero value retries immediately. Defaults to `2s`.
    pub health_probe_backoff: Duration,
}

impl Default for RelaunchConfig {
    fn default() -> Self {
        Self {
            canary_target_dir: std::env::temp_dir()
                .join(format!("simard-canary-{}", std::process::id())),
            health_timeout: Duration::from_secs(30),
            manifest_dir: PathBuf::from("."),
            canary_env: Vec::new(),
            health_probe_max_attempts: 3,
            health_probe_backoff: Duration::from_secs(2),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelaunchGate {
    Smoke,
    UnitTest,
    GymBaseline,
    RpcHealth,
}

impl Display for RelaunchGate {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Smoke => "smoke",
            Self::UnitTest => "unit-test",
            Self::GymBaseline => "gym-baseline",
            Self::RpcHealth => "rpc-health",
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
        RelaunchGate::RpcHealth,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunch_gate_display() {
        assert_eq!(RelaunchGate::Smoke.to_string(), "smoke");
        assert_eq!(RelaunchGate::RpcHealth.to_string(), "rpc-health");
    }

    #[test]
    fn default_gates_has_all_four() {
        let gates = default_gates();
        assert_eq!(gates.len(), 4);
        assert_eq!(gates[0], RelaunchGate::Smoke);
        assert_eq!(gates[3], RelaunchGate::RpcHealth);
    }

    #[test]
    fn relaunch_config_default_health_timeout() {
        let config = RelaunchConfig::default();
        assert_eq!(config.health_timeout, Duration::from_secs(30));
    }

    #[test]
    fn relaunch_config_default_manifest_dir() {
        let config = RelaunchConfig::default();
        assert_eq!(config.manifest_dir, PathBuf::from("."));
    }

    #[test]
    fn relaunch_config_default_canary_dir_is_unique_per_process() {
        let config = RelaunchConfig::default();
        let dir = config.canary_target_dir;
        // Must live under the system temp dir, not a hardcoded /tmp path.
        assert!(
            dir.starts_with(std::env::temp_dir()),
            "canary_target_dir must be under temp_dir(), got: {}",
            dir.display()
        );
        // Must contain the PID for per-process uniqueness.
        let pid = std::process::id().to_string();
        let dir_str = dir.to_string_lossy();
        assert!(
            dir_str.contains(&pid),
            "canary_target_dir must include PID ({pid}) for uniqueness, got: {dir_str}"
        );
    }

    #[test]
    fn relaunch_gate_display_all_variants() {
        assert_eq!(RelaunchGate::Smoke.to_string(), "smoke");
        assert_eq!(RelaunchGate::UnitTest.to_string(), "unit-test");
        assert_eq!(RelaunchGate::GymBaseline.to_string(), "gym-baseline");
        assert_eq!(RelaunchGate::RpcHealth.to_string(), "rpc-health");
    }

    #[test]
    fn gate_result_display_pass() {
        let result = GateResult {
            gate: RelaunchGate::Smoke,
            passed: true,
            detail: "version: 1.0.0".to_string(),
        };
        let display = result.to_string();
        assert!(display.contains("[PASS]"), "{display}");
        assert!(display.contains("smoke"), "{display}");
        assert!(display.contains("version: 1.0.0"), "{display}");
    }

    #[test]
    fn gate_result_display_fail() {
        let result = GateResult {
            gate: RelaunchGate::UnitTest,
            passed: false,
            detail: "3 tests failed".to_string(),
        };
        let display = result.to_string();
        assert!(display.contains("[FAIL]"), "{display}");
        assert!(display.contains("unit-test"), "{display}");
        assert!(display.contains("3 tests failed"), "{display}");
    }

    #[test]
    fn relaunch_gate_eq() {
        assert_eq!(RelaunchGate::Smoke, RelaunchGate::Smoke);
        assert_ne!(RelaunchGate::Smoke, RelaunchGate::UnitTest);
    }

    #[test]
    fn gate_result_clone() {
        let result = GateResult {
            gate: RelaunchGate::Smoke,
            passed: true,
            detail: "ok".to_string(),
        };
        let cloned = result.clone();
        assert_eq!(cloned.gate, result.gate);
        assert_eq!(cloned.passed, result.passed);
        assert_eq!(cloned.detail, result.detail);
    }

    #[test]
    fn gate_result_debug() {
        let result = GateResult {
            gate: RelaunchGate::RpcHealth,
            passed: false,
            detail: "err".to_string(),
        };
        let debug = format!("{result:?}");
        assert!(debug.contains("RpcHealth"), "{debug}");
    }

    // ── P3 (process:self_deploy_blocked): additive rpc-health retry knobs ──────
    // (TDD Step 7 — FAILING.) `RelaunchConfig` gains a configurable probe timeout
    // (already `health_timeout`) plus a bounded retry budget and backoff. The new
    // fields are serde-defaulted and additive, so existing configs deserialize
    // unchanged. These reference fields that do not exist yet and MUST fail to
    // compile until the fix lands. See docs/reference/rpc-health-gate-diagnostics.md.

    #[test]
    fn default_health_timeout_preserves_prior_30s_behaviour() {
        // The per-attempt probe timeout default must preserve the prior fixed 30s.
        assert_eq!(
            RelaunchConfig::default().health_timeout,
            Duration::from_secs(30)
        );
    }

    #[test]
    fn default_health_probe_attempts_is_bounded_positive() {
        let config = RelaunchConfig::default();
        assert!(
            config.health_probe_max_attempts >= 1,
            "the probe attempt budget must be a bounded, positive default"
        );
    }

    #[test]
    fn default_health_probe_backoff_is_set() {
        let config = RelaunchConfig::default();
        // A bounded, non-negative base backoff between probe attempts.
        assert!(
            config.health_probe_backoff <= Duration::from_secs(60),
            "the base backoff must have a bounded default (capped exponential)"
        );
    }
}
