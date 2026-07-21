use std::path::Path;
use std::process::Command;

use super::types::{GateResult, RelaunchConfig, RelaunchGate};
use crate::error::SimardResult;

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

fn run_gate(binary: &Path, gate: RelaunchGate, config: &RelaunchConfig) -> GateResult {
    match gate {
        RelaunchGate::Smoke => run_smoke_gate(binary),
        RelaunchGate::UnitTest => run_unit_test_gate(config),
        RelaunchGate::GymBaseline => run_gym_baseline_gate(binary),
        RelaunchGate::RpcHealth => run_rpc_health_gate(binary, config),
    }
}

fn run_smoke_gate(binary: &Path) -> GateResult {
    match Command::new(binary).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            GateResult::pass(RelaunchGate::Smoke, format!("version: {}", stdout.trim()))
        }
        Ok(output) => GateResult::fail(
            RelaunchGate::Smoke,
            format!(
                "binary exited with {}: {}",
                output.status,
                truncate_output(&String::from_utf8_lossy(&output.stderr), 200)
            ),
        ),
        Err(e) => GateResult::fail(
            RelaunchGate::Smoke,
            format!("failed to execute binary: {e}"),
        ),
    }
}

fn run_unit_test_gate(config: &RelaunchConfig) -> GateResult {
    match Command::new("cargo")
        .arg("test")
        .arg("--manifest-path")
        .arg(config.manifest_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&config.canary_target_dir)
        .env("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs())
        .output()
    {
        Ok(output) if output.status.success() => {
            GateResult::pass(RelaunchGate::UnitTest, "all tests passed")
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let truncated = truncate_output(&stderr, 200);
            GateResult::fail(
                RelaunchGate::UnitTest,
                format!("tests failed (exit {}): {}", output.status, truncated),
            )
        }
        Err(e) => GateResult::fail(
            RelaunchGate::UnitTest,
            format!("cargo test failed to run: {e}"),
        ),
    }
}

fn run_gym_baseline_gate(binary: &Path) -> GateResult {
    match Command::new(binary).args(["gym", "list"]).output() {
        Ok(output) if output.status.success() => {
            GateResult::pass(RelaunchGate::GymBaseline, "gym list succeeded")
        }
        Ok(output) => GateResult::fail(
            RelaunchGate::GymBaseline,
            format!(
                "gym probe failed (exit {}): {}",
                output.status,
                truncate_output(&String::from_utf8_lossy(&output.stderr), 200)
            ),
        ),
        Err(e) => GateResult::fail(
            RelaunchGate::GymBaseline,
            format!("gym probe failed to run: {e}"),
        ),
    }
}

/// FAIL-CLOSED predicate: does this probe result carry a POSITIVELY-recognized
/// signal that the RPC endpoint is simply *absent* (no daemon listening) rather
/// than present-but-unhealthy? Only a recognized absence signal — the dedicated
/// `EX_UNAVAILABLE` (69) exit code, or an explicit connection-refused/no-daemon
/// phrase on stderr — may permit a skip. Everything else (healthy, reachable-
/// but-unhealthy, spawn error, or an unknown non-zero exit) returns `false` and
/// reds the canary, so a genuine RPC regression can never be masked as "absent".
fn endpoint_absent(probe: &std::io::Result<std::process::Output>) -> bool {
    /// `sysexits.h` EX_UNAVAILABLE — the service/endpoint is unavailable.
    const EX_UNAVAILABLE: i32 = 69;
    const ABSENCE_SIGNALS: [&str; 5] = [
        "connection refused",
        "no daemon",
        "could not connect",
        "connection reset",
        "no such file or directory",
    ];

    let output = match probe {
        Ok(output) => output,
        // A binary that cannot execute is a build defect, never endpoint absence.
        Err(_) => return false,
    };
    // A healthy (exit 0) probe is a pass, not an absence signal.
    if output.status.success() {
        return false;
    }
    if output.status.code() == Some(EX_UNAVAILABLE) {
        return true;
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    ABSENCE_SIGNALS.iter().any(|sig| stderr.contains(sig))
}

fn run_rpc_health_gate(binary: &Path, config: &RelaunchConfig) -> GateResult {
    let timeout_secs = config.health_timeout.as_secs().to_string();
    let probe = Command::new(binary)
        .args(["probe", "rpc", "--timeout", &timeout_secs])
        .output();
    match &probe {
        Ok(output) if output.status.success() => {
            GateResult::pass(RelaunchGate::RpcHealth, "rpc health check passed")
        }
        // The RPC endpoint is legitimately absent in an isolated canary (a
        // freshly built binary with no running daemon to answer the probe). A
        // positively-recognized absence signal is a SKIP, not a regression, so
        // an unreachable endpoint never reds a self-deploy canary. A reachable-
        // but-unhealthy RPC still fails closed via the arms below.
        _ if endpoint_absent(&probe) => GateResult::skip(
            RelaunchGate::RpcHealth,
            "rpc endpoint absent in isolated canary (no daemon) — skipped",
        ),
        Ok(output) => GateResult::fail(
            RelaunchGate::RpcHealth,
            format!(
                "rpc health failed (exit {}): {}",
                output.status,
                truncate_output(&String::from_utf8_lossy(&output.stderr), 200)
            ),
        ),
        Err(e) => GateResult::fail(
            RelaunchGate::RpcHealth,
            format!("rpc health probe failed to run: {e}"),
        ),
    }
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
            .map_or(0, |(i, c)| i + c.len_utf8());
        format!("{}...", s[..boundary].trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_gate_handles_missing_binary() {
        let result = run_smoke_gate(Path::new("/tmp/no-such-binary-48291"));
        assert!(!result.passed);
    }

    // --- truncate_output ---

    #[test]
    fn truncate_output_short_string_unchanged() {
        let result = truncate_output("hello world", 100);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn truncate_output_exact_length() {
        let input = "abcde";
        let result = truncate_output(input, 5);
        assert_eq!(result, "abcde");
    }

    #[test]
    fn truncate_output_over_limit_appends_ellipsis() {
        let input = "abcdefghij";
        let result = truncate_output(input, 5);
        assert!(
            result.ends_with("..."),
            "should end with ellipsis: {result}"
        );
        assert!(result.len() <= 8, "should be truncated: {result}");
    }

    #[test]
    fn truncate_output_trims_whitespace() {
        let result = truncate_output("  hello  ", 100);
        assert_eq!(result, "hello");
    }

    #[test]
    fn truncate_output_empty_string() {
        let result = truncate_output("", 100);
        assert_eq!(result, "");
    }

    #[test]
    fn truncate_output_multibyte_utf8_safe() {
        let input = "héllo wörld café";
        let result = truncate_output(input, 8);
        assert!(
            result.ends_with("..."),
            "should end with ellipsis: {result}"
        );
        // Must not panic on multi-byte boundary
    }

    #[test]
    fn truncate_output_zero_max_len() {
        let result = truncate_output("hello", 0);
        assert_eq!(result, "...");
    }

    // --- all_gates_passed ---

    #[test]
    fn all_gates_passed_empty_is_true() {
        assert!(all_gates_passed(&[]));
    }

    #[test]
    fn all_gates_passed_all_true() {
        let results = vec![
            GateResult::pass(RelaunchGate::Smoke, "ok"),
            GateResult::pass(RelaunchGate::UnitTest, "ok"),
        ];
        assert!(all_gates_passed(&results));
    }

    #[test]
    fn all_gates_passed_one_false() {
        let results = vec![
            GateResult::pass(RelaunchGate::Smoke, "ok"),
            GateResult::fail(RelaunchGate::UnitTest, "fail"),
            GateResult::pass(RelaunchGate::GymBaseline, "ok"),
        ];
        assert!(!all_gates_passed(&results));
    }

    // --- verify_canary ---

    #[test]
    fn verify_canary_with_missing_binary() {
        let config = RelaunchConfig::default();
        let results = verify_canary(
            Path::new("/no-such-binary-99999"),
            &[RelaunchGate::Smoke],
            &config,
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert!(
            !results[0].passed,
            "smoke gate should fail for missing binary"
        );
    }

    #[test]
    fn verify_canary_runs_all_gates_without_short_circuit() {
        // Use a curated gate list (excludes RelaunchGate::UnitTest, which
        // would recursively invoke `cargo test` and run for 30+ minutes
        // when this test itself is executed under `cargo test`).
        let config = RelaunchConfig::default();
        let gates = [
            RelaunchGate::Smoke,
            RelaunchGate::GymBaseline,
            RelaunchGate::RpcHealth,
        ];
        let results = verify_canary(Path::new("/no-such-binary-99999"), &gates, &config).unwrap();
        assert_eq!(
            results.len(),
            3,
            "should run all 3 selected gates even if first fails"
        );
        assert!(
            results.iter().all(|r| !r.passed),
            "all gates should fail for missing binary"
        );
    }

    #[test]
    fn verify_canary_empty_gates() {
        let config = RelaunchConfig::default();
        let results = verify_canary(Path::new("/no-such-binary"), &[], &config).unwrap();
        assert!(results.is_empty());
    }

    // ── endpoint_absent predicate + skip flow (canary-gate #2590) ────────────
    //
    // TDD (Step 7): pin the FAIL-CLOSED security contract of the new centralized
    // `endpoint_absent` predicate — only a positively-recognized absence signal
    // may skip; everything else (healthy, reachable-but-unhealthy, spawn error,
    // unknown) FAILS and reds the canary. These FAIL until `gates.rs` gains
    // `endpoint_absent`, the RpcHealth skip branch, and the `GateResult`
    // constructors + `skipped` field (expected RED state).

    fn probe_output(code: i32, stderr: &str) -> Result<std::process::Output, std::io::Error> {
        use std::os::unix::process::ExitStatusExt;
        Ok(std::process::Output {
            // `code << 8` is the wait(2) encoding for a normal exit with the
            // given code (no signal), so `.code() == Some(code)`.
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    #[test]
    fn endpoint_absent_true_for_positively_absent_endpoint() {
        // A positively-recognized "no daemon / connection refused" signal. It
        // carries BOTH a dedicated unavailable exit code (69, EX_UNAVAILABLE)
        // and an absence phrase, so the assertion holds whichever detection
        // mechanism the implementation adopts.
        let probe = probe_output(69, "connection refused");
        assert!(
            endpoint_absent(&probe),
            "a positively-detected absent endpoint must be skippable"
        );
    }

    #[test]
    fn endpoint_absent_false_for_reachable_but_unhealthy() {
        // SECURITY CONTROL: a reachable endpoint reporting unhealthy is a
        // genuine red and must NOT skip (non-zero exit that is not an absence
        // signal).
        let probe = probe_output(1, "rpc responded but reported degraded health");
        assert!(
            !endpoint_absent(&probe),
            "reachable-but-unhealthy must fail closed (red), never skip"
        );
    }

    #[test]
    fn endpoint_absent_false_for_spawn_error() {
        // FAIL-CLOSED: a binary that cannot execute is a build defect, never
        // endpoint absence.
        let probe: Result<std::process::Output, std::io::Error> = Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no binary",
        ));
        assert!(!endpoint_absent(&probe), "spawn error must fail closed");
    }

    #[test]
    fn endpoint_absent_false_for_healthy_success() {
        let probe = probe_output(0, "");
        assert!(
            !endpoint_absent(&probe),
            "a healthy (exit 0) probe is a pass, not an absence signal"
        );
    }

    #[test]
    fn rpc_health_gate_spawn_error_fails_closed_not_skipped() {
        // A missing binary => spawn error. The RpcHealth gate must FAIL closed,
        // never treat an unexecutable binary as endpoint-absence.
        let config = RelaunchConfig::default();
        let result = run_rpc_health_gate(Path::new("/no-such-binary-77123"), &config);
        assert!(!result.passed, "spawn error must FAIL, never skip");
        assert!(
            !result.skipped,
            "a binary that cannot execute is not endpoint-absence"
        );
    }

    #[test]
    fn all_gates_passed_treats_skip_as_non_failing() {
        let results = vec![
            GateResult::pass(RelaunchGate::Smoke, "ok"),
            GateResult::skip(RelaunchGate::RpcHealth, "absent"),
            GateResult::pass(RelaunchGate::GymBaseline, "ok"),
        ];
        assert!(all_gates_passed(&results), "a skip must not red the canary");
    }

    #[test]
    fn all_gates_passed_still_false_when_a_real_gate_fails_alongside_a_skip() {
        let results = vec![
            GateResult::pass(RelaunchGate::Smoke, "ok"),
            GateResult::skip(RelaunchGate::RpcHealth, "absent"),
            GateResult::fail(RelaunchGate::UnitTest, "2 failed"),
        ];
        assert!(
            !all_gates_passed(&results),
            "a genuine failure alongside a skip is still red"
        );
    }
}
