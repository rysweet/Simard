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
            GateResult {
                gate: RelaunchGate::Smoke,
                passed: true,
                detail: format!("version: {}", stdout.trim()),
            }
        }
        Ok(output) => GateResult {
            gate: RelaunchGate::Smoke,
            passed: false,
            detail: format!(
                "binary exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        },
        Err(e) => GateResult {
            gate: RelaunchGate::Smoke,
            passed: false,
            detail: format!("failed to execute binary: {e}"),
        },
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
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: true,
            detail: "all tests passed".to_string(),
        },
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Surface WHICH test produced the red canary (#4470) instead of an
            // opaque "exit 101". `cargo test` prints the `... FAILED` lines to
            // stdout; fall back to a sanitized stderr tail when none is found
            // (e.g. a compile error rather than a test failure).
            let failing = extract_first_failure(&stdout).or_else(|| extract_first_failure(&stderr));
            let detail = match failing {
                Some(test) => format!(
                    "tests failed (exit {}): first failing test {}",
                    output.status, test
                ),
                None => format!(
                    "tests failed (exit {}): {}",
                    output.status,
                    crate::util::log_sanitize::sanitize_to_single_line(
                        &stderr,
                        GATE_DETAIL_MAX_BYTES
                    )
                ),
            };
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail,
            }
        }
        Err(e) => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: false,
            detail: format!("cargo test failed to run: {e}"),
        },
    }
}

fn run_gym_baseline_gate(binary: &Path) -> GateResult {
    match Command::new(binary).args(["gym", "list"]).output() {
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::GymBaseline,
            passed: true,
            detail: "gym list succeeded".to_string(),
        },
        Ok(output) => GateResult {
            gate: RelaunchGate::GymBaseline,
            passed: false,
            detail: format!(
                "gym probe failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        },
        Err(e) => GateResult {
            gate: RelaunchGate::GymBaseline,
            passed: false,
            detail: format!("gym probe failed to run: {e}"),
        },
    }
}

fn run_rpc_health_gate(binary: &Path, config: &RelaunchConfig) -> GateResult {
    let timeout_secs = config.health_timeout.as_secs().to_string();
    match Command::new(binary)
        .args(["probe", "rpc", "--timeout", &timeout_secs])
        .output()
    {
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::RpcHealth,
            passed: true,
            detail: "rpc health check passed".to_string(),
        },
        Ok(output) => GateResult {
            gate: RelaunchGate::RpcHealth,
            passed: false,
            detail: format!(
                "rpc health failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        },
        Err(e) => GateResult {
            gate: RelaunchGate::RpcHealth,
            passed: false,
            detail: format!("rpc health probe failed to run: {e}"),
        },
    }
}

/// Upper bound (bytes) on any gate `detail` derived from untrusted subprocess
/// output (#4470). Bounds log/JSON size and blast radius of forged content.
pub(crate) const GATE_DETAIL_MAX_BYTES: usize = 512;

/// Extract the fully-qualified path of the FIRST failing test from `cargo test`
/// stdout/stderr (#4470 diagnosability).
///
/// `cargo test` prints `test module::path::name ... FAILED` for each failure and
/// a `failures:` summary block listing `    module::path::name`. This returns the
/// first failing test's path (e.g. `self_deploy::tests_health::foo`), sanitized
/// via [`crate::util::log_sanitize::sanitize_to_single_line`] and bounded to
/// [`GATE_DETAIL_MAX_BYTES`], so the canary can surface WHICH test produced the
/// red canary instead of an opaque "exit 101". Returns `None` when no
/// failing-test line is present.
pub(crate) fn extract_first_failure(cargo_test_output: &str) -> Option<String> {
    for line in cargo_test_output.lines() {
        // Per-test result lines look like `test <path> ... FAILED`. The summary
        // line `test result: FAILED. ...` starts with `test ` too but never ends
        // in ` ... FAILED`, so the suffix match below excludes it.
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("test ") else {
            continue;
        };
        let Some(path) = rest.strip_suffix(" ... FAILED") else {
            continue;
        };
        let path = path.trim();
        if !path.is_empty() {
            return Some(crate::util::log_sanitize::sanitize_to_single_line(
                path,
                GATE_DETAIL_MAX_BYTES,
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_gate_handles_missing_binary() {
        let result = run_smoke_gate(Path::new("/tmp/no-such-binary-48291"));
        assert!(!result.passed);
    }

    // --- all_gates_passed ---

    #[test]
    fn all_gates_passed_empty_is_true() {
        assert!(all_gates_passed(&[]));
    }

    #[test]
    fn all_gates_passed_all_true() {
        let results = vec![
            GateResult {
                gate: RelaunchGate::Smoke,
                passed: true,
                detail: "ok".to_string(),
            },
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: true,
                detail: "ok".to_string(),
            },
        ];
        assert!(all_gates_passed(&results));
    }

    #[test]
    fn all_gates_passed_one_false() {
        let results = vec![
            GateResult {
                gate: RelaunchGate::Smoke,
                passed: true,
                detail: "ok".to_string(),
            },
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: "fail".to_string(),
            },
            GateResult {
                gate: RelaunchGate::GymBaseline,
                passed: true,
                detail: "ok".to_string(),
            },
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

    // ── #4470: failing-test diagnosability (extract_first_failure / sanitize) ──

    #[test]
    fn extract_first_failure_parses_failed_test_path() {
        let output = "\
running 3 tests
test self_deploy::tests_health::report_is_healthy_only_when_every_probe_is_healthy ... ok
test self_deploy::tests_health::any_single_unhealthy_probe_fails_the_report ... FAILED
test self_relaunch::gates::tests::smoke_gate_handles_missing_binary ... FAILED

failures:

failures:
    self_deploy::tests_health::any_single_unhealthy_probe_fails_the_report
    self_relaunch::gates::tests::smoke_gate_handles_missing_binary

test result: FAILED. 1 passed; 2 failed; 0 ignored;
";
        assert_eq!(
            extract_first_failure(output).as_deref(),
            Some("self_deploy::tests_health::any_single_unhealthy_probe_fails_the_report"),
            "must surface the FIRST failing test's fully-qualified path"
        );
    }

    #[test]
    fn extract_first_failure_none_when_all_pass() {
        let output = "\
running 2 tests
test a::b ... ok
test c::d ... ok

test result: ok. 2 passed; 0 failed;
";
        assert_eq!(extract_first_failure(output), None);
    }

    #[test]
    fn extract_first_failure_is_bounded() {
        // A pathological, very long "test path" must be bounded to the cap.
        let long = "x".repeat(5000);
        let line = format!("test {long} ... FAILED\n");
        let extracted = extract_first_failure(&line).expect("a failure was present");
        assert!(
            extracted.len() <= GATE_DETAIL_MAX_BYTES,
            "extracted failure must be bounded to {GATE_DETAIL_MAX_BYTES} bytes, got {}",
            extracted.len()
        );
    }
}
