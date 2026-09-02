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
}

impl Default for RelaunchConfig {
    fn default() -> Self {
        Self {
            canary_target_dir: std::env::temp_dir()
                .join(format!("simard-canary-{}", std::process::id())),
            health_timeout: Duration::from_secs(30),
            manifest_dir: PathBuf::from("."),
            canary_env: Vec::new(),
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
        // Sanitise `detail` through the same redaction+length bound the
        // tracing/OTel sink uses (#4511). `detail` is built from untrusted
        // subprocess stderr and can embed a token-bearing remote URL; the
        // operator-CLI sink (`eprintln!("{r}")`) must not print it raw.
        let detail = super::gates::bound_gate_detail(&self.detail);
        write!(f, "[{}] {}: {}", status, self.gate, detail)
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

    // #4511: the operator-CLI sink (`eprintln!("{r}")`) must sanitise `detail`
    // symmetrically with the tracing/OTel sink — never printing an embedded
    // credential nor an unbounded blob.
    #[test]
    fn gate_result_display_redacts_embedded_credentials() {
        let result = GateResult {
            gate: RelaunchGate::Smoke,
            passed: false,
            detail: "clone failed: https://x-access-token:ghp_SECRETTOKEN123@github.com/o/r.git"
                .to_string(),
        };
        let display = result.to_string();
        assert!(
            !display.contains("ghp_SECRETTOKEN123"),
            "token leaked to operator-CLI sink: {display}"
        );
        assert!(display.contains("***@github.com"), "{display}");
    }

    #[test]
    fn gate_result_display_bounds_oversized_detail() {
        let result = GateResult {
            gate: RelaunchGate::UnitTest,
            passed: false,
            detail: "x".repeat(4096),
        };
        let display = result.to_string();
        // Prefix "[FAIL] unit-test: " + bounded 512-char detail + "..." — the
        // full 4096-char blob must never reach the terminal.
        assert!(
            display.len() < 600,
            "detail not bounded on operator-CLI sink: len={}",
            display.len()
        );
        assert!(display.ends_with("..."), "{display}");
    }
}
