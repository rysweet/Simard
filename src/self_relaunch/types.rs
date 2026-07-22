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

/// The curated, recursion-free deploy-canary gate list (#4469/#4470).
///
/// Returns `[Smoke, GymBaseline, RpcHealth]` — deliberately **excluding**
/// `UnitTest`. The `UnitTest` gate shells `cargo test`; run inside a deploy
/// canary that the overseer itself spawned, it recurses into the test suite and
/// returns a deterministic exit 101, which was the root cause of the red-canary
/// self-deploy crash-loop. The exhaustive [`default_gates`] suite (which still
/// includes `UnitTest`) remains the list used by CI / manual verification.
pub fn canary_gates() -> Vec<RelaunchGate> {
    vec![
        RelaunchGate::Smoke,
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

    // ── canary_gates() — curated, recursion-free deploy-canary list ─────────
    // Root cause of the red-canary exit-101 crash-loop (#4469 / #4470): the
    // deploy canary ran `default_gates()`, which includes `UnitTest`; that
    // gate shells `cargo test` and, run inside a canary already spawned by the
    // overseer, recurses into the test suite and returns a deterministic
    // exit 101. `canary_gates()` is the curated list that omits `UnitTest`.

    #[test]
    fn canary_gates_excludes_unit_test() {
        // INVARIANT: adding `UnitTest` back is a regression that re-opens the
        // recursion crash-loop — this assertion must fail if that ever happens.
        let gates = canary_gates();
        assert!(
            !gates.contains(&RelaunchGate::UnitTest),
            "canary_gates() must NEVER contain UnitTest (recursion → exit 101, #4469/#4470); got {gates:?}"
        );
    }

    #[test]
    fn canary_gates_order_is_stable() {
        // Stable order keeps `failing_gate` attribution deterministic in the
        // #4420 red-canary diagnostics.
        assert_eq!(
            canary_gates(),
            vec![
                RelaunchGate::Smoke,
                RelaunchGate::GymBaseline,
                RelaunchGate::RpcHealth,
            ],
            "canary_gates() must be exactly [Smoke, GymBaseline, RpcHealth] in order"
        );
    }

    #[test]
    fn canary_gates_every_member_is_a_default_gate() {
        // The curated list is a subset of the exhaustive suite — it removes a
        // gate, it never invents a new one.
        let full = default_gates();
        for gate in canary_gates() {
            assert!(
                full.contains(&gate),
                "canary gate {gate:?} must also be a member of default_gates()"
            );
        }
    }

    #[test]
    fn default_gates_still_includes_unit_test() {
        // The exhaustive suite (used by CI / manual verification) is UNCHANGED
        // and still exercises the unit-test gate.
        assert!(
            default_gates().contains(&RelaunchGate::UnitTest),
            "default_gates() must remain the exhaustive suite including UnitTest"
        );
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
