use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;
use std::time::Duration;

/// Env var overriding the per-gate subprocess wall-clock budget (seconds).
/// Parse-or-default to [`DEFAULT_GATE_TIMEOUT_SECS`], then clamp to
/// `[MIN_GATE_TIMEOUT_SECS, MAX_GATE_TIMEOUT_SECS]`.
pub const GATE_TIMEOUT_ENV: &str = "RELAUNCH_GATE_TIMEOUT_SECS";

/// Default per-gate timeout: 600 s (10 minutes). Long enough for a cold
/// `cargo test` on merged `main`, short enough that a genuinely hung gate is
/// reaped inside one tick rather than wedging the daemon (#4415).
pub const DEFAULT_GATE_TIMEOUT_SECS: u64 = 600;

/// Non-zero floor so a mis-set env can never disable the bound or collapse the
/// timeout into a busy-loop.
pub const MIN_GATE_TIMEOUT_SECS: u64 = 1;

/// Absolute ceiling so a pathological env value can never wedge a gate for an
/// unbounded time.
pub const MAX_GATE_TIMEOUT_SECS: u64 = 3600;

/// Clamp a raw per-gate timeout (seconds) into
/// `[MIN_GATE_TIMEOUT_SECS, MAX_GATE_TIMEOUT_SECS]`.
pub fn clamp_gate_timeout_secs(secs: u64) -> u64 {
    secs.clamp(MIN_GATE_TIMEOUT_SECS, MAX_GATE_TIMEOUT_SECS)
}

/// Resolve the per-gate subprocess timeout from [`GATE_TIMEOUT_ENV`]. Fail-safe:
/// an unset, empty, or unparseable value falls back to
/// [`DEFAULT_GATE_TIMEOUT_SECS`]; any value is then clamped. Never panics.
pub fn resolve_gate_timeout() -> Duration {
    let secs = std::env::var(GATE_TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_GATE_TIMEOUT_SECS);
    Duration::from_secs(clamp_gate_timeout_secs(secs))
}

#[derive(Clone, Debug)]
pub struct RelaunchConfig {
    pub canary_target_dir: PathBuf,
    pub health_timeout: Duration,
    pub manifest_dir: PathBuf,
    /// Wall-clock budget for a single verification gate subprocess (e.g. the
    /// full `cargo test` UnitTest gate). On expiry the child is killed AND
    /// reaped and the gate is recorded as a `timed_out` failure (Brick A,
    /// #4415). Default 600 s; overridable via [`GATE_TIMEOUT_ENV`], clamped to a
    /// non-zero floor and an absolute ceiling.
    pub gate_timeout: Duration,
}

impl Default for RelaunchConfig {
    fn default() -> Self {
        Self {
            canary_target_dir: std::env::temp_dir()
                .join(format!("simard-canary-{}", std::process::id())),
            health_timeout: Duration::from_secs(30),
            manifest_dir: PathBuf::from("."),
            gate_timeout: resolve_gate_timeout(),
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
    /// `true` only when this gate's subprocess was killed for exceeding
    /// `gate_timeout` (Brick A, #4415). A normal assertion failure leaves this
    /// `false`. The transient-red classification (Brick B) keys on THIS flag —
    /// never on `detail` text.
    pub timed_out: bool,
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
            timed_out: false,
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
            timed_out: false,
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
            timed_out: false,
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
            timed_out: false,
        };
        let debug = format!("{result:?}");
        assert!(debug.contains("RpcHealth"), "{debug}");
    }

    // ── STEP 7 TDD (#4415): bounded per-gate timeout config (Brick A) ─────────
    //
    // `RelaunchConfig.gate_timeout` bounds each canary gate subprocess so a hung
    // gate cannot wedge the self-deploy tick (the root of the recurring red
    // canary). The value is env-tunable (`RELAUNCH_GATE_TIMEOUT_SECS`) but is
    // parse-or-default then clamped to a NON-ZERO floor and an absolute ceiling,
    // so a mis-set env can neither disable the bound nor collapse it to a
    // busy-loop. These tests are written FIRST and fail until the `gate_timeout`
    // field, the constants, and the clamp/resolve helpers exist.

    /// Restore a previously-observed env value (or clear it) after a test that
    /// mutated `RELAUNCH_GATE_TIMEOUT_SECS`.
    fn restore_gate_timeout_env(prev: Option<String>) {
        // SAFETY: single-threaded test-local env toggle, serialized by the
        // `relaunch_gate_timeout_env` serial key so no sibling test races it.
        unsafe {
            match prev {
                Some(v) => std::env::set_var(GATE_TIMEOUT_ENV, v),
                None => std::env::remove_var(GATE_TIMEOUT_ENV),
            }
        }
    }

    #[test]
    #[serial_test::serial(relaunch_gate_timeout_env, cognitive_memory)]
    fn relaunch_config_default_gate_timeout_is_ten_minutes() {
        let prev = std::env::var(GATE_TIMEOUT_ENV).ok();
        // SAFETY: serialized env toggle, restored below.
        unsafe {
            std::env::remove_var(GATE_TIMEOUT_ENV);
        }
        let config = RelaunchConfig::default();
        assert_eq!(
            config.gate_timeout,
            Duration::from_secs(DEFAULT_GATE_TIMEOUT_SECS),
            "the default per-gate timeout must be the 10-minute default"
        );
        assert_eq!(
            DEFAULT_GATE_TIMEOUT_SECS, 600,
            "the documented default per-gate timeout is 10 minutes"
        );
        restore_gate_timeout_env(prev);
    }

    #[test]
    fn clamp_gate_timeout_leaves_in_range_values_unchanged() {
        assert_eq!(clamp_gate_timeout_secs(120), 120);
        assert_eq!(
            clamp_gate_timeout_secs(DEFAULT_GATE_TIMEOUT_SECS),
            DEFAULT_GATE_TIMEOUT_SECS
        );
    }

    #[test]
    fn clamp_gate_timeout_raises_zero_to_a_nonzero_floor() {
        let clamped = clamp_gate_timeout_secs(0);
        assert_eq!(clamped, MIN_GATE_TIMEOUT_SECS);
        assert!(
            clamped > 0,
            "a zero timeout would disable the bound / busy-loop — it must clamp up"
        );
    }

    #[test]
    fn clamp_gate_timeout_caps_pathological_values() {
        assert_eq!(clamp_gate_timeout_secs(u64::MAX), MAX_GATE_TIMEOUT_SECS);
        const {
            assert!(
                MAX_GATE_TIMEOUT_SECS >= MIN_GATE_TIMEOUT_SECS,
                "the ceiling must not fall below the floor"
            )
        };
    }

    #[test]
    #[serial_test::serial(relaunch_gate_timeout_env, cognitive_memory)]
    fn resolve_gate_timeout_parses_a_valid_env_value() {
        let prev = std::env::var(GATE_TIMEOUT_ENV).ok();
        // SAFETY: serialized env toggle, restored below.
        unsafe {
            std::env::set_var(GATE_TIMEOUT_ENV, "45");
        }
        assert_eq!(resolve_gate_timeout(), Duration::from_secs(45));
        restore_gate_timeout_env(prev);
    }

    #[test]
    #[serial_test::serial(relaunch_gate_timeout_env, cognitive_memory)]
    fn resolve_gate_timeout_defaults_on_unparseable_env() {
        let prev = std::env::var(GATE_TIMEOUT_ENV).ok();
        // SAFETY: serialized env toggle, restored below.
        unsafe {
            std::env::set_var(GATE_TIMEOUT_ENV, "not-a-number");
        }
        assert_eq!(
            resolve_gate_timeout(),
            Duration::from_secs(DEFAULT_GATE_TIMEOUT_SECS),
            "an unparseable env value falls back to the default, never panics"
        );
        restore_gate_timeout_env(prev);
    }

    #[test]
    #[serial_test::serial(relaunch_gate_timeout_env, cognitive_memory)]
    fn resolve_gate_timeout_clamps_a_zero_env_to_the_floor() {
        let prev = std::env::var(GATE_TIMEOUT_ENV).ok();
        // SAFETY: serialized env toggle, restored below.
        unsafe {
            std::env::set_var(GATE_TIMEOUT_ENV, "0");
        }
        let resolved = resolve_gate_timeout();
        assert_eq!(resolved, Duration::from_secs(MIN_GATE_TIMEOUT_SECS));
        assert!(
            resolved >= Duration::from_secs(1),
            "env=0 must never disable the timeout bound"
        );
        restore_gate_timeout_env(prev);
    }
}
