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
    // Fail-closed recursion sentinel (#4470): refuse to shell `cargo test` when
    // running inside the deploy canary, so the gate can never recurse into the
    // test suite (deterministic exit 101). Pure decision in
    // `unit_test_recursion_refusal` for hermetic testability.
    if let Some(refusal) =
        unit_test_recursion_refusal(std::env::var_os("SIMARD_IN_DEPLOY_CANARY").is_some())
    {
        return refusal;
    }
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
            let stderr = String::from_utf8_lossy(&output.stderr);
            let truncated = truncate_output(&stderr, 200);
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("tests failed (exit {}): {}", output.status, truncated),
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

/// Fail-closed recursion sentinel decision for the `UnitTest` gate (#4470).
///
/// Pure and injectable so the deploy-canary refusal path can be tested without
/// mutating process-global env or spawning `cargo test`. Returns:
/// * `Some(failed GateResult)` when `in_deploy_canary` is true — the gate MUST
///   refuse to shell `cargo test` (fail closed, never silently green);
/// * `None` otherwise — the gate proceeds normally.
///
fn unit_test_recursion_refusal(in_deploy_canary: bool) -> Option<GateResult> {
    if !in_deploy_canary {
        return None;
    }
    Some(GateResult {
        gate: RelaunchGate::UnitTest,
        passed: false,
        detail: "recursion guard: refusing to shell `cargo test` inside the \
                 deploy canary (would recurse into the test suite → exit 101, \
                 #4470); the curated canary_gates() list excludes UnitTest"
            .to_string(),
    })
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

    // ── deploy-canary curation (#4469 / #4470) ─────────────────────────────
    // The deploy canary must run the curated `canary_gates()` list, which
    // excludes `UnitTest`. Running the curated list against a MISSING binary
    // exercises every selected gate's failure path WITHOUT ever shelling
    // `cargo test` — proving the list is recursion-free (no exit-101 hang).

    #[test]
    fn deploy_canary_runs_curated_gates_without_unit_test() {
        let config = RelaunchConfig::default();
        let gates = crate::self_relaunch::canary_gates();

        // The curated list itself must never carry the recursive gate.
        assert!(
            !gates.contains(&super::RelaunchGate::UnitTest),
            "canary_gates() must exclude UnitTest so the deploy canary never recurses"
        );

        // verify_canary runs each selected gate; with a missing binary every
        // gate fails fast (no `cargo test` subprocess is ever spawned).
        let results =
            verify_canary(Path::new("/no-such-binary-canary-99999"), &gates, &config).unwrap();
        assert_eq!(
            results.len(),
            gates.len(),
            "the canary must run exactly the curated gate list"
        );
        assert_eq!(
            results.len(),
            3,
            "curated canary list is [Smoke, GymBaseline, RpcHealth]"
        );
        assert!(
            results
                .iter()
                .all(|r| r.gate != super::RelaunchGate::UnitTest),
            "no result may come from the UnitTest gate"
        );
        assert!(
            results.iter().all(|r| !r.passed),
            "a missing binary reddens every curated gate"
        );
    }

    // ── fail-closed recursion sentinel (#4470) ─────────────────────────────
    // Defense-in-depth: even if a future caller runs the canary with a list
    // that still contains `UnitTest`, the gate runner must REFUSE to shell
    // `cargo test` when the deploy-canary marker is active, returning a
    // *failed* GateResult (fail-closed) rather than recursing. The decision is
    // a pure, injectable seam so the test stays hermetic (no env mutation, no
    // `cargo test` subprocess).

    #[test]
    fn unit_test_gate_refuses_inside_deploy_canary() {
        let refusal = super::unit_test_recursion_refusal(true)
            .expect("inside the deploy canary the unit-test gate must refuse (Some)");
        assert_eq!(refusal.gate, super::RelaunchGate::UnitTest);
        assert!(
            !refusal.passed,
            "the recursion sentinel must FAIL CLOSED — never silently green"
        );
        assert!(
            refusal.detail.to_lowercase().contains("recursion"),
            "the refusal detail must explain the recursion guard, got: {}",
            refusal.detail
        );
    }

    #[test]
    fn unit_test_gate_runs_normally_outside_deploy_canary() {
        // Outside the deploy canary there is no refusal — the gate proceeds to
        // its normal `cargo test` path (None means "no sentinel short-circuit").
        assert!(
            super::unit_test_recursion_refusal(false).is_none(),
            "outside the deploy canary the unit-test gate must NOT be short-circuited"
        );
    }
}
