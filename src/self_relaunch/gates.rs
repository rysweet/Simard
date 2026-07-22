use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use super::types::{GateResult, RelaunchConfig, RelaunchGate};
use crate::error::SimardResult;

/// Outcome of running a gate subprocess under a wall-clock bound (Brick A, #4415).
pub enum GateExec {
    /// The child exited on its own before the timeout; carries its captured output.
    Completed(Output),
    /// The child exceeded its budget and was killed **and** reaped.
    TimedOut,
}

/// Run `cmd` with a wall-clock `timeout`. On expiry the child is killed AND
/// reaped (waited on) so no zombie or runaway build survives the tick; its
/// stdout/stderr are drained on helper threads so a chatty child can never
/// deadlock on a full pipe. Returns `Err` only if the child could not be
/// spawned (an infrastructural spawn fault), never on a timeout.
pub fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> std::io::Result<GateExec> {
    use std::io::Read;

    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;

    // Drain both pipes concurrently so a subprocess that emits more than a pipe
    // buffer's worth of output cannot block before it exits (which would defeat
    // the timeout).
    let mut child_stdout = child.stdout.take();
    let mut child_stderr = child.stderr.take();
    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = child_stdout.as_mut() {
            let _ = s.read_to_end(&mut buf);
        }
        buf
    });
    let err_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = child_stderr.as_mut() {
            let _ = s.read_to_end(&mut buf);
        }
        buf
    });

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            // Kill and reap so no zombie / runaway cargo survives the tick.
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_handle.join();
            let _ = err_handle.join();
            return Ok(GateExec::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    Ok(GateExec::Completed(Output {
        status,
        stdout,
        stderr,
    }))
}

/// Fold a bounded gate execution into a [`GateResult`]. A timeout is a FAILED
/// gate carrying the structured `timed_out: true` flag (never inferred from
/// `detail` text); a spawn fault is a failed, non-timeout gate.
fn finish_gate(
    gate: RelaunchGate,
    exec: std::io::Result<GateExec>,
    timeout: Duration,
    on_success: impl FnOnce(&Output) -> String,
    on_failure: impl FnOnce(&Output) -> String,
    spawn_context: &str,
) -> GateResult {
    match exec {
        Ok(GateExec::Completed(output)) if output.status.success() => GateResult {
            gate,
            passed: true,
            detail: on_success(&output),
            timed_out: false,
        },
        Ok(GateExec::Completed(output)) => GateResult {
            gate,
            passed: false,
            detail: on_failure(&output),
            timed_out: false,
        },
        Ok(GateExec::TimedOut) => GateResult {
            gate,
            passed: false,
            detail: format!("{gate} gate timed out after {}s", timeout.as_secs()),
            timed_out: true,
        },
        Err(e) => GateResult {
            gate,
            passed: false,
            detail: format!("{spawn_context}: {e}"),
            timed_out: false,
        },
    }
}

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
        RelaunchGate::Smoke => run_smoke_gate(binary, config.gate_timeout),
        RelaunchGate::UnitTest => run_unit_test_gate(config),
        RelaunchGate::GymBaseline => run_gym_baseline_gate(binary, config.gate_timeout),
        RelaunchGate::RpcHealth => run_rpc_health_gate(binary, config),
    }
}

fn run_smoke_gate(binary: &Path, timeout: Duration) -> GateResult {
    let mut cmd = Command::new(binary);
    cmd.arg("--version");
    finish_gate(
        RelaunchGate::Smoke,
        run_with_timeout(&mut cmd, timeout),
        timeout,
        |output| {
            let stdout = String::from_utf8_lossy(&output.stdout);
            format!("version: {}", stdout.trim())
        },
        |output| {
            format!(
                "binary exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        },
        "failed to execute binary",
    )
}

fn run_unit_test_gate(config: &RelaunchConfig) -> GateResult {
    let mut cmd = Command::new("cargo");
    cmd.arg("test")
        .arg("--manifest-path")
        .arg(config.manifest_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&config.canary_target_dir)
        .env("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs());
    finish_gate(
        RelaunchGate::UnitTest,
        run_with_timeout(&mut cmd, config.gate_timeout),
        config.gate_timeout,
        |_output| "all tests passed".to_string(),
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let truncated = truncate_output(&stderr, 200);
            format!("tests failed (exit {}): {}", output.status, truncated)
        },
        "cargo test failed to run",
    )
}

fn run_gym_baseline_gate(binary: &Path, timeout: Duration) -> GateResult {
    let mut cmd = Command::new(binary);
    cmd.args(["gym", "list"]);
    finish_gate(
        RelaunchGate::GymBaseline,
        run_with_timeout(&mut cmd, timeout),
        timeout,
        |_output| "gym list succeeded".to_string(),
        |output| {
            format!(
                "gym probe failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        },
        "gym probe failed to run",
    )
}

fn run_rpc_health_gate(binary: &Path, config: &RelaunchConfig) -> GateResult {
    let timeout_secs = config.health_timeout.as_secs().to_string();
    let mut cmd = Command::new(binary);
    cmd.args(["probe", "rpc", "--timeout", &timeout_secs]);
    finish_gate(
        RelaunchGate::RpcHealth,
        run_with_timeout(&mut cmd, config.gate_timeout),
        config.gate_timeout,
        |_output| "rpc health check passed".to_string(),
        |output| {
            format!(
                "rpc health failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        },
        "rpc health probe failed to run",
    )
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
        let result = run_smoke_gate(
            Path::new("/tmp/no-such-binary-48291"),
            Duration::from_secs(600),
        );
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
                timed_out: false,
            },
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: true,
                detail: "ok".to_string(),
                timed_out: false,
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
                timed_out: false,
            },
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: "fail".to_string(),
                timed_out: false,
            },
            GateResult {
                gate: RelaunchGate::GymBaseline,
                passed: true,
                detail: "ok".to_string(),
                timed_out: false,
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

    // ── STEP 7 TDD (#4415): bounded per-gate subprocess timeout (Brick A) ─────
    //
    // A hung or flaky gate subprocess (a `cargo test` that never returns, a
    // wedged rpc probe) must not wedge the whole self-deploy tick — the recurring
    // "red canary" the OODA loop keeps re-observing. `run_with_timeout` bounds
    // each gate subprocess: on expiry it KILLS and REAPS the child (no zombie, no
    // runaway cargo) and reports a timeout, which the gate encodes as a
    // failed-but-`timed_out` result. A normal (non-timeout) failure leaves
    // `timed_out == false`. These tests are written FIRST and fail until the
    // `run_with_timeout` helper, the `GateExec` outcome, and the
    // `GateResult.timed_out` field exist.

    #[test]
    fn run_with_timeout_kills_and_reaps_a_slow_child_promptly() {
        let start = std::time::Instant::now();
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let exec = run_with_timeout(&mut cmd, std::time::Duration::from_millis(100))
            .expect("spawning `sleep` must succeed");
        assert!(
            matches!(exec, GateExec::TimedOut),
            "a child exceeding the timeout must report TimedOut"
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "must kill+reap the child promptly, not block for its full 30s runtime"
        );
    }

    #[test]
    fn run_with_timeout_returns_completed_for_a_fast_child() {
        let mut cmd = Command::new("sleep");
        cmd.arg("0");
        let exec = run_with_timeout(&mut cmd, std::time::Duration::from_secs(10))
            .expect("spawning `sleep` must succeed");
        match exec {
            GateExec::Completed(output) => {
                assert!(output.status.success(), "`sleep 0` must exit 0")
            }
            GateExec::TimedOut => {
                panic!("a fast child must complete within the timeout, not time out")
            }
        }
    }

    #[test]
    fn normal_gate_failure_is_not_flagged_as_timed_out() {
        // A plain gate failure (missing binary) must set `passed = false` but
        // leave `timed_out = false` — only an actual timeout sets the flag, so a
        // genuine regression is never misclassified as a flaky/transient timeout.
        let result = run_smoke_gate(
            Path::new("/tmp/no-such-binary-48291"),
            Duration::from_secs(600),
        );
        assert!(!result.passed);
        assert!(
            !result.timed_out,
            "a normal (non-timeout) failure must not be flagged timed_out"
        );
    }

    #[test]
    fn gate_result_carries_a_timed_out_flag() {
        let timed = GateResult {
            gate: RelaunchGate::UnitTest,
            passed: false,
            detail: "gate timed out after 600s".to_string(),
            timed_out: true,
        };
        assert!(timed.timed_out);
        assert!(!timed.passed, "a timed-out gate is a failed gate");
    }
}
