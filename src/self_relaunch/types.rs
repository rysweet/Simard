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
    /// True iff the gate was skipped because a required live endpoint is
    /// legitimately absent in the isolated canary context (e.g. no running
    /// daemon for the RPC health probe). INVARIANT: `skipped ⇒ passed`, so
    /// every existing `.passed` read-site treats a skip as non-failing while
    /// the skip itself stays visible for diagnostics.
    pub skipped: bool,
    pub detail: String,
}

impl GateResult {
    /// A gate that ran and passed.
    pub fn pass(gate: RelaunchGate, detail: impl Into<String>) -> Self {
        Self {
            gate,
            passed: true,
            skipped: false,
            detail: detail.into(),
        }
    }

    /// A gate that ran and genuinely failed — this reds the canary.
    pub fn fail(gate: RelaunchGate, detail: impl Into<String>) -> Self {
        Self {
            gate,
            passed: false,
            skipped: false,
            detail: detail.into(),
        }
    }

    /// A gate skipped because its required endpoint is legitimately absent in
    /// the isolated canary. Upholds the `skipped ⇒ passed` invariant so the
    /// skip never reds the canary, yet remains surfaced for diagnostics.
    pub fn skip(gate: RelaunchGate, detail: impl Into<String>) -> Self {
        Self {
            gate,
            passed: true,
            skipped: true,
            detail: detail.into(),
        }
    }
}

impl Display for GateResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let status = if self.skipped {
            "SKIP"
        } else if self.passed {
            "PASS"
        } else {
            "FAIL"
        };
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
            skipped: false,
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
            skipped: false,
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
            skipped: false,
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
            skipped: false,
            detail: "err".to_string(),
        };
        let debug = format!("{result:?}");
        assert!(debug.contains("RpcHealth"), "{debug}");
    }

    // ── Skipped outcome + pass/fail/skip constructors (canary-gate #2590) ────
    //
    // TDD (Step 7): these specify the additive `skipped` field and the three
    // intent-encoding constructors. They FAIL until `types.rs` gains the field
    // and `GateResult::{pass,fail,skip}` — that is the expected RED state.

    #[test]
    fn gate_result_pass_constructor_runs_and_passes() {
        let r = GateResult::pass(RelaunchGate::Smoke, "version: 1.4.2");
        assert!(r.passed);
        assert!(!r.skipped);
        assert_eq!(r.gate, RelaunchGate::Smoke);
        assert_eq!(r.detail, "version: 1.4.2");
    }

    #[test]
    fn gate_result_fail_constructor_is_red_not_skip() {
        let r = GateResult::fail(RelaunchGate::UnitTest, "2 tests failed");
        assert!(!r.passed);
        assert!(!r.skipped, "a failing gate is never a skip");
        assert_eq!(r.gate, RelaunchGate::UnitTest);
        assert_eq!(r.detail, "2 tests failed");
    }

    #[test]
    fn gate_result_skip_constructor_is_non_failing() {
        let r = GateResult::skip(
            RelaunchGate::RpcHealth,
            "endpoint absent in isolated canary (no daemon) — skipped",
        );
        assert!(r.skipped);
        // INVARIANT: skipped ⇒ passed. A skip must never count as a failure so
        // every existing `.passed` read-site treats it as non-failing.
        assert!(r.passed, "skipped ⇒ passed invariant");
    }

    #[test]
    fn gate_result_skip_upholds_skipped_implies_passed_for_all_gates() {
        for gate in default_gates() {
            let r = GateResult::skip(gate, "absent");
            assert!(
                !r.skipped || r.passed,
                "skipped must imply passed for {gate}"
            );
        }
    }

    #[test]
    fn gate_result_display_skip_renders_skip_tag() {
        let r = GateResult::skip(RelaunchGate::RpcHealth, "no daemon — skipped");
        let s = r.to_string();
        assert!(s.contains("[SKIP]"), "{s}");
        assert!(s.contains("rpc-health"), "{s}");
        assert!(s.contains("no daemon"), "{s}");
    }

    #[test]
    fn gate_result_pass_and_fail_display_unchanged() {
        assert!(
            GateResult::pass(RelaunchGate::Smoke, "ok")
                .to_string()
                .contains("[PASS]")
        );
        assert!(
            GateResult::fail(RelaunchGate::Smoke, "boom")
                .to_string()
                .contains("[FAIL]")
        );
    }
}
