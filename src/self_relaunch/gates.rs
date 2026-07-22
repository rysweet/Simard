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
                    sanitize_gate_detail(&stderr, GATE_DETAIL_MAX_BYTES)
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
/// via [`sanitize_gate_detail`] and bounded to [`GATE_DETAIL_MAX_BYTES`], so the
/// canary can surface WHICH test produced the red canary instead of an opaque
/// "exit 101". Returns `None` when no failing-test line is present.
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
            return Some(sanitize_gate_detail(path, GATE_DETAIL_MAX_BYTES));
        }
    }
    None
}

/// Sanitize an untrusted subprocess string for embedding in a `GateResult.detail`
/// (#4470): strip CR/LF and other control characters, collapse to a single line,
/// and bound the result to `max_bytes` (UTF-8-boundary-safe). Prevents a canary
/// test name / stderr from forging log lines or JSON.
pub(crate) fn sanitize_gate_detail(raw: &str, max_bytes: usize) -> String {
    // Collapse every run of control characters (newlines, tabs, ANSI escapes,
    // NUL) to a single space so the result is one readable line with no forgery
    // vectors.
    let mut collapsed = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_control() {
            if !collapsed.ends_with(' ') {
                collapsed.push(' ');
            }
        } else {
            collapsed.push(c);
        }
    }
    let trimmed = collapsed.trim();
    if trimmed.len() <= max_bytes {
        return trimmed.to_string();
    }
    // Bound on a UTF-8 char boundary so we never split a multi-byte char.
    let mut end = max_bytes;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
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

    #[test]
    fn sanitize_gate_detail_strips_control_chars_and_newlines() {
        let raw = "line one\nline two\r\n\ttabbed\x1b[31mred\x00nul";
        let clean = sanitize_gate_detail(raw, GATE_DETAIL_MAX_BYTES);
        assert!(!clean.contains('\n'), "newlines stripped: {clean:?}");
        assert!(
            !clean.contains('\r'),
            "carriage returns stripped: {clean:?}"
        );
        assert!(
            !clean.contains('\x1b'),
            "escape sequences stripped: {clean:?}"
        );
        assert!(!clean.contains('\0'), "NUL stripped: {clean:?}");
        assert!(
            !clean.contains('\t') || clean.contains(' '),
            "no raw tabs: {clean:?}"
        );
    }

    #[test]
    fn sanitize_gate_detail_bounds_length() {
        let raw = "a".repeat(2000);
        let clean = sanitize_gate_detail(&raw, GATE_DETAIL_MAX_BYTES);
        assert!(
            clean.len() <= GATE_DETAIL_MAX_BYTES,
            "must bound to {GATE_DETAIL_MAX_BYTES} bytes, got {}",
            clean.len()
        );
    }

    #[test]
    fn sanitize_gate_detail_utf8_boundary_safe() {
        // Bounding must never split a multi-byte char (no panic, valid UTF-8).
        let raw = "héllo wörld café ".repeat(100);
        let clean = sanitize_gate_detail(&raw, 10);
        assert!(clean.len() <= 10);
        // Round-trips as valid UTF-8 (String is always valid; the point is no panic).
        let _ = clean.chars().count();
    }
}
