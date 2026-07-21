use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct RelaunchConfig {
    pub canary_target_dir: PathBuf,
    pub health_timeout: Duration,
    pub manifest_dir: PathBuf,
}

impl Default for RelaunchConfig {
    fn default() -> Self {
        Self {
            canary_target_dir: std::env::temp_dir()
                .join(format!("simard-canary-{}", std::process::id())),
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

/// The first failing gate in gate order, or `None` when every gate passed.
///
/// Pure, read-only accessor over the existing results: it does not re-run gates
/// and does not change `all_gates_passed`'s verdict. Deterministic — identical
/// result sets always name the same (first, in slice order) failing gate.
pub fn first_failure(results: &[GateResult]) -> Option<&GateResult> {
    results.iter().find(|r| !r.passed)
}

/// Upper bound (chars) on a surfaced gate `detail`, per the telemetry-hygiene
/// contract. Candidate output is untrusted; the surfaced form is length-bounded.
const MAX_DETAIL_CHARS: usize = 512;

/// Diagnostic payload attached to a red-canary deploy refusal.
///
/// Additive and `Default`-able: a `Default` value (no failing gate named) is
/// equivalent to the prior, detail-free red-canary refusal, so existing
/// constructors and tests compile and behave unchanged. It is a pure side
/// channel — it never influences the deploy-gate verdict.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RedCanaryDetail {
    /// Slug of the first failing gate in gate order (e.g. `"rpc-health"`).
    /// Empty when no specific gate is known.
    pub failed_gate: String,
    /// Sanitized, length-bounded detail from that gate's `GateResult`.
    pub detail: String,
}

impl RedCanaryDetail {
    /// Build from the first failing `GateResult` in an ordered slice. Returns
    /// `Default` (empty) when every gate passed. The gate `detail` is treated as
    /// untrusted candidate output: it is trimmed and length-bounded (char-boundary
    /// safe) before it is retained for surfacing.
    pub fn from_results(results: &[GateResult]) -> Self {
        match first_failure(results) {
            None => Self::default(),
            Some(failed) => Self {
                failed_gate: failed.gate.to_string(),
                detail: super::gates::truncate_output(&failed.detail, MAX_DETAIL_CHARS),
            },
        }
    }

    /// One-line summary for the deploy notification / refusal `Display`.
    ///
    /// Names the failing gate and its reason (e.g. `` gate `rpc-health` failed:
    /// … ``). A `Default` (empty) value falls back to the legacy
    /// `"one or more gates failed"` wording the operator already knows.
    pub fn summary(&self) -> String {
        if self.failed_gate.is_empty() {
            return "one or more gates failed".to_string();
        }
        format!("gate `{}` failed: {}", self.failed_gate, self.detail)
    }
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
}
