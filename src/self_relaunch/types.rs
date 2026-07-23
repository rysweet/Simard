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
        write!(f, "[{}] {}: {}", status, self.gate, self.detail)
    }
}

impl GateResult {
    /// Credential-redacted (SEC-D2), length-bounded rendering of this result,
    /// safe for any sink that persists or emits it (logs, OTel, operator
    /// stderr). Unlike [`Display`], which yields the raw `detail` for in-process
    /// debugging, this MUST be used at every emitting sink: a reddening gate's
    /// `detail` can embed a token-bearing remote URL and — since the #4522
    /// diagnosability fix threads up to 16 KiB of `cargo test` output — is no
    /// longer implicitly short (the prior 200-byte cap is gone). Emitting the
    /// raw `detail` at a log/telemetry sink would therefore both leak
    /// credentials and defeat the DoS bound that `gates.rs` enforces internally.
    pub fn redacted_display(&self) -> String {
        const MAX_DETAIL_BYTES: usize = 512;
        let status = if self.passed { "PASS" } else { "FAIL" };
        let redacted = crate::self_deploy::source_prep::redact_credentials(&self.detail);
        let trimmed = redacted.trim();
        let detail = if trimmed.len() <= MAX_DETAIL_BYTES {
            trimmed.to_string()
        } else {
            let end = next_char_boundary(trimmed, MAX_DETAIL_BYTES);
            format!("{}...", &trimmed[..end])
        };
        format!("[{}] {}: {}", status, self.gate, detail)
    }
}

/// Smallest index `>= idx` (clamped to `s.len()`) that lands on a UTF-8 char
/// boundary. Slicing a `&str` at a mid-codepoint byte panics, so every byte-cap
/// truncation in this module (`redacted_display` here, `truncate_output` /
/// `truncate_output_tail` in `gates.rs`) walks forward from its cap to the next
/// boundary before slicing. The back-off is O(1) — at most 3 bytes for any
/// UTF-8 codepoint — never an O(idx) rescan. Extracted so the boundary rule
/// lives in exactly one place (the three sites previously duplicated it).
pub(super) fn next_char_boundary(s: &str, idx: usize) -> usize {
    let mut end = idx.min(s.len());
    while end < s.len() && !s.is_char_boundary(end) {
        end += 1;
    }
    end
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
    fn redacted_display_redacts_credentials_and_bounds_length() {
        // A reddening gate whose detail embeds a token-bearing remote URL and is
        // far larger than the 512-byte emission cap (the #4522 fix threads up to
        // 16 KiB of cargo output through `detail`). `redacted_display` — used at
        // every log/stderr sink — must scrub the credential and bound the size,
        // whereas raw `Display` intentionally does not (in-process debug only).
        let secret = "https://x-access-token:ghs_SUPERSECRETTOKEN@github.com/o/r.git";
        let detail = format!("gate failed: {secret} {}", "A".repeat(2000));
        let result = GateResult {
            gate: RelaunchGate::UnitTest,
            passed: false,
            detail,
        };

        let safe = result.redacted_display();
        assert!(safe.contains("[FAIL]"), "{safe}");
        assert!(safe.contains("unit-test"), "{safe}");
        assert!(
            !safe.contains("ghs_SUPERSECRETTOKEN"),
            "credential leaked through redacted_display: {safe}"
        );
        assert!(
            safe.len() < 600,
            "redacted_display not bounded (len {}): {safe}",
            safe.len()
        );
        assert!(safe.ends_with("..."), "elision marker missing: {safe}");

        // Raw Display is the un-redacted debug path and still exposes the token,
        // proving the two renderers are deliberately distinct.
        assert!(result.to_string().contains("ghs_SUPERSECRETTOKEN"));
    }

    #[test]
    fn redacted_display_short_detail_unmarked() {
        let result = GateResult {
            gate: RelaunchGate::Smoke,
            passed: true,
            detail: "version: 1.0.0".to_string(),
        };
        let safe = result.redacted_display();
        assert_eq!(safe, "[PASS] smoke: version: 1.0.0");
        assert!(!safe.ends_with("..."));
    }

    #[test]
    fn redacted_display_multibyte_boundary_safe() {
        // 512-byte cap must not split a multi-byte codepoint (no panic).
        let result = GateResult {
            gate: RelaunchGate::UnitTest,
            passed: false,
            detail: "λ".repeat(1000),
        };
        let safe = result.redacted_display();
        assert!(safe.is_char_boundary(safe.len()));
        assert!(safe.ends_with("..."));
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
