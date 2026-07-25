use std::path::Path;
use std::process::Command;

use super::types::{GateResult, RelaunchConfig, RelaunchGate};
use crate::error::SimardResult;

/// The Simard-specific environment the canary gates legitimately need to render
/// a **true** verdict for a healthy candidate — the deploy-shape signals that
/// the deployed binary itself runs under (systemd sets `SIMARD_HOME` /
/// `SIMARD_PROMPT_ASSETS_DIR`; see [`crate::install::systemd`]) plus the state
/// root the `rpc-health` probe dials to reach the **currently running** daemon.
///
/// This is populated into [`RelaunchConfig::canary_env`] by the canary build
/// wiring (`prepare_build_and_verify_canary`) so the root-cause repair for the
/// #4440 red-canary stall "supplies the missing signal" through an audited
/// **allow-list of names** rather than by widening the deny-by-default base
/// floor or inheriting the daemon's whole ambient env. Names only — values are
/// read live at spawn time; a name absent from the environment is skipped, so a
/// gate still fails closed on a genuinely missing signal.
///
/// These are deliberately **not** in [`scrub_gate_env`]'s universal base floor:
/// that floor is the minimum for *any* gate to run at all, whereas these are
/// Simard-candidate policy, derived from the #4420 `failing_gate` diagnostics.
pub fn canary_gate_env_allowlist() -> Vec<String> {
    [
        "SIMARD_HOME",
        "SIMARD_PROMPT_ASSETS_DIR",
        "SIMARD_STATE_ROOT",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// `env_clear()` + selective re-injection of the always-required base variables
/// and any names in `config.canary_env`, for **every** canary gate subprocess
/// (`smoke`, `unit-test`, `gym-baseline`, `rpc-health`).
///
/// Why (root cause of the #4440 red-canary non-convergence): the running
/// Overseer spawns these gates with its own (possibly hostile or dev-polluted)
/// ambient environment. Inheriting it wholesale causes two failures:
///   * **Hijack** — an ambient `LD_PRELOAD`, `GIT_SSH_COMMAND`, or an injected
///     `SIMARD_*` toggle could steer a gate into a false verdict (mirrors the
///     [`scrub_git_env`](crate::self_deploy::source_prep) defense).
///   * **Shape drift** — the deployed binary ships under a *clean* systemd env
///     (`SIMARD_HOME`, `SIMARD_PROMPT_ASSETS_DIR`, `PATH`; see
///     [`crate::install::systemd`]). Verifying the canary under a fatter ambient
///     env can pass a binary that then reddens once deployed — or redden a
///     healthy binary every tick (the observed 928cd7da stall).
///
/// The base set is the **universal floor**: enough for *any* gate to run at all.
/// It must keep every gate functional so a genuinely healthy candidate stays
/// GREEN (no false RED that would perpetuate the stall). It therefore spans the
/// candidate binary's core runtime needs and the `cargo test` toolchain the
/// `unit-test` gate shells out to (`CARGO_HOME`/`RUSTUP_HOME`/`RUSTUP_TOOLCHAIN`).
/// Simard deploy-shape signals (`SIMARD_HOME`, …) are **not** in this floor; they
/// arrive as the explicit [`canary_gate_env_allowlist`] via `config.canary_env`.
/// Anything outside this base set and `config.canary_env` is dropped
/// (deny-by-default); `LD_PRELOAD`-class variables are never allow-listable —
/// [`is_hijack_class_env`] enforces this in code (SEC-D3 defense-in-depth), so
/// the guarantee holds even if a future caller populates `config.canary_env`
/// from a less-trusted source than [`canary_gate_env_allowlist`].
/// Names absent from the environment are skipped. Nothing is logged here.
fn scrub_gate_env(cmd: &mut Command, config: &RelaunchConfig) {
    cmd.env_clear();
    const BASE: &[&str] = &[
        // Core process env.
        "PATH",
        "HOME",
        // Cargo/rustup toolchain — load-bearing for the `unit-test` gate, which
        // shells out to `cargo test`. Without these `env_clear()` would falsely
        // redden a healthy candidate (a self-inflicted stall).
        "CARGO_HOME",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        // ssh-agent for any git the binary shells out to (mirrors scrub_git_env).
        "SSH_AUTH_SOCK",
        // User / locale basics so a gate does not misbehave on a bare env.
        "USER",
        "LOGNAME",
        "LANG",
        "LC_ALL",
        "TZ",
        "TERM",
    ];
    for var in BASE {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }
    // Operator/canary-build allow-list: names only, values read live at spawn.
    // A hijack-class name (`LD_*`, `DYLD_*`, `GIT_SSH*`, `BASH_ENV`, …) is
    // refused even if it appears here (SEC-D3): re-injecting one would reopen
    // exactly the ambient-env hijack this scrub exists to close.
    for name in &config.canary_env {
        if is_hijack_class_env(name) {
            continue;
        }
        if let Ok(val) = std::env::var(name) {
            cmd.env(name, val);
        }
    }
}

/// True when `name` is an execution-hijack environment variable that must never
/// be re-injected into a canary gate subprocess, regardless of whether an
/// operator or build step listed it in [`RelaunchConfig::canary_env`]. These
/// steer a dynamic loader, shell, or git transport into running attacker code
/// (`LD_PRELOAD` / `LD_LIBRARY_PATH`, macOS `DYLD_*`, `GIT_SSH_COMMAND`,
/// `BASH_ENV` / `ENV`, `SHELLOPTS` / `BASHOPTS`, `IFS`). Matching is
/// case-insensitive so a lower/mixed-case spelling cannot slip a variant past
/// the floor. This is the code-enforced counterpart to the docstring guarantee
/// on [`scrub_gate_env`] — the deny-by-default floor already omits them; this
/// prevents the allow-list re-injection loop from restoring one.
fn is_hijack_class_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    const HIJACK_PREFIXES: &[&str] = &["LD_", "DYLD_", "GIT_SSH"];
    const HIJACK_EXACT: &[&str] = &["BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS", "IFS"];
    HIJACK_PREFIXES.iter().any(|p| upper.starts_with(p)) || HIJACK_EXACT.iter().any(|n| upper == *n)
}

/// Construct a [`Command`] for `program` already scrubbed to the canary gate
/// environment (see [`scrub_gate_env`]). Every gate spawns through this, which
/// makes "a gate subprocess is *always* run under the scrubbed env" a
/// structural invariant: a gate cannot silently inherit the daemon's ambient
/// env by forgetting the scrub call. `env_clear()` runs at construction, so any
/// gate-specific `.env(...)` (e.g. `CARGO_BUILD_JOBS`) added afterwards survives.
fn scrubbed_command(program: impl AsRef<std::ffi::OsStr>, config: &RelaunchConfig) -> Command {
    let mut cmd = Command::new(program);
    scrub_gate_env(&mut cmd, config);
    cmd
}

/// Verify a canary binary against a sequence of gates (does not short-circuit).
pub fn verify_canary(
    binary: &Path,
    gates: &[RelaunchGate],
    config: &RelaunchConfig,
) -> SimardResult<Vec<GateResult>> {
    let mut results = Vec::with_capacity(gates.len());

    for &gate in gates {
        // Per-gate tracing span (#4440): the exact gate that reddens — and its
        // bounded, credential-redacted detail — is emitted structurally as it
        // runs, not only reconstructed after the fact from the aggregate report.
        let span = tracing::info_span!(target: "self_relaunch::gate", "canary_gate", gate = %gate);
        let _enter = span.enter();
        let result = run_gate(binary, gate, config);
        tracing::info!(
            target: "self_relaunch::gate",
            gate = %result.gate,
            passed = result.passed,
            detail = %bound_gate_detail(&result.detail),
            "canary gate evaluated"
        );
        // Surface a red gate at ERROR level so a relaunch refusal is not buried
        // among the per-gate INFO lines. The detail is built to lead with the
        // failing test name(s) (see `summarize_test_failure`), so the actionable
        // signal survives the credential-redacted length bound. Additive only —
        // the fail-closed verdict is entirely carried by `result`.
        if !result.passed {
            tracing::error!(
                target: "self_relaunch::gate",
                gate = %result.gate,
                detail = %bound_gate_detail(&result.detail),
                "canary gate FAILED — relaunch refused"
            );
        }
        results.push(result);
    }

    Ok(results)
}

/// Redact URL-embedded credentials (SEC-D2) and bound the length of a gate
/// detail before it is emitted to `tracing`/OTel — a gate's stderr can embed a
/// token-bearing remote URL and can be arbitrarily long.
fn bound_gate_detail(detail: &str) -> String {
    truncate_output(
        &crate::self_deploy::source_prep::redact_credentials(detail),
        512,
    )
}

pub fn all_gates_passed(results: &[GateResult]) -> bool {
    results.iter().all(|r| r.passed)
}

fn run_gate(binary: &Path, gate: RelaunchGate, config: &RelaunchConfig) -> GateResult {
    match gate {
        RelaunchGate::Smoke => run_smoke_gate(binary, config),
        RelaunchGate::UnitTest => run_unit_test_gate(config),
        RelaunchGate::GymBaseline => run_gym_baseline_gate(binary, config),
        RelaunchGate::RpcHealth => run_rpc_health_gate(binary, config),
    }
}

fn run_smoke_gate(binary: &Path, config: &RelaunchConfig) -> GateResult {
    let mut cmd = scrubbed_command(binary, config);
    cmd.arg("--version");
    match cmd.output() {
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
    let mut cmd = scrubbed_command("cargo", config);
    cmd.arg("test")
        .arg("--manifest-path")
        .arg(config.manifest_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&config.canary_target_dir)
        .env("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs());
    match cmd.output() {
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: true,
            detail: "all tests passed".to_string(),
        },
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Diagnosability fix (#4558): libtest prints the failing-test names and
            // the "has been running for over N seconds" banners to STDOUT, which the
            // old gate discarded (it truncated only stderr to 200 chars). Capture
            // BOTH streams, name the failing tests, and sanitize control/ANSI bytes
            // so the tracing event in `evaluate_gates` is actually actionable.
            let summary = summarize_test_failure(&stdout, &stderr);
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("tests failed (exit {}): {}", output.status, summary),
            }
        }
        Err(e) => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: false,
            detail: format!("cargo test failed to run: {e}"),
        },
    }
}

fn run_gym_baseline_gate(binary: &Path, config: &RelaunchConfig) -> GateResult {
    let mut cmd = scrubbed_command(binary, config);
    cmd.args(["gym", "list"]);
    match cmd.output() {
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
    let mut cmd = scrubbed_command(binary, config);
    cmd.args(["probe", "rpc", "--timeout", &timeout_secs]);
    match cmd.output() {
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

/// Append `name` to `names` if non-empty and not already present, preserving
/// first-seen order. Used by the libtest capture parsers to dedup a failing
/// test that appears both on its `… FAILED` running line and in the trailing
/// `failures:` block.
fn push_unique(names: &mut Vec<String>, name: &str) {
    let name = name.trim();
    if !name.is_empty() && !names.iter().any(|n| n == name) {
        names.push(name.to_string());
    }
}

/// Extract the names of assertion-failed tests from a libtest capture.
///
/// Combines the two places libtest names a failure — the `test <name> ... FAILED`
/// running line and the indented entries under the trailing `failures:` block —
/// and dedups them in first-seen order. A single bounded forward line-scan (no
/// regex, no backtracking) keeps this linear on pathological input (ReDoS-safe).
/// Slow-test banners are intentionally NOT collected here (see
/// [`parse_slow_test_banners`]) so a timeout-red stays distinguishable from an
/// assertion-red.
fn parse_failing_test_names(capture: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut in_failures_block = false;
    for line in capture.lines() {
        let trimmed = line.trim();

        // Source A: the per-test running line, e.g. `test foo::beta ... FAILED`.
        if let Some(rest) = trimmed.strip_prefix("test ")
            && let Some(name) = rest.strip_suffix(" ... FAILED")
        {
            push_unique(&mut names, name);
            continue;
        }

        // Source B: the trailing `failures:` block lists the bare names, indented.
        if trimmed == "failures:" {
            in_failures_block = true;
            continue;
        }
        if in_failures_block {
            // The block ends at a blank line or the `test result:` summary line.
            if trimmed.is_empty() || trimmed.starts_with("test result:") {
                in_failures_block = false;
                continue;
            }
            // Real names are single tokens; skip the `---- name stdout ----`
            // sub-headers (which contain spaces) that precede panic output.
            if !trimmed.starts_with("----") && !trimmed.contains(' ') {
                push_unique(&mut names, trimmed);
            }
        }
    }
    names
}

/// Extract the names of tests that tripped libtest's
/// `test <name> has been running for over N seconds` banner. Kept separate from
/// [`parse_failing_test_names`] so a canary red caused by a slow/hung test is
/// reported as a timeout rather than being conflated with an assertion failure.
fn parse_slow_test_banners(capture: &str) -> Vec<String> {
    const MARKER: &str = " has been running for over ";
    let mut names: Vec<String> = Vec::new();
    for line in capture.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("test ")
            && let Some(idx) = rest.find(MARKER)
        {
            push_unique(&mut names, &rest[..idx]);
        }
    }
    names
}

/// Strip ANSI/VT escape sequences and C0 control bytes (except newline) from a
/// captured test stream before it is surfaced in a gate detail / tracing event.
///
/// Test output is untrusted data: a token embedded in a panic message could
/// carry ANSI escapes to spoof the terminal or `\r` to overwrite/forge a log
/// line (log injection). Printable text and newlines are preserved so the
/// diagnostic stays readable.
fn sanitize_gate_capture(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // ESC: drop a CSI sequence (`ESC [ … final`) or a lone escape.
            '\u{1b}' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    // Consume parameter/intermediate bytes up to and including the
                    // final byte in the 0x40..=0x7E range.
                    for nc in chars.by_ref() {
                        if ('@'..='~').contains(&nc) {
                            break;
                        }
                    }
                }
            }
            '\n' => out.push('\n'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Build the compact, fail-closed `detail` for a failed `UnitTest` gate from the
/// FULL captured stdout+stderr.
///
/// Names the failing (and separately, any slow/timed-out) tests, then appends a
/// generously-bounded tail of the sanitized capture so a red with no parseable
/// test names — a link error or a panic before the harness starts — is still
/// diagnosable. The result is never empty and never reads as a success, so the
/// gate's fail-closed contract holds regardless of capture content.
fn summarize_test_failure(stdout: &str, stderr: &str) -> String {
    let combined = format!("{stdout}\n{stderr}");
    let clean = sanitize_gate_capture(&combined);

    let failing = parse_failing_test_names(&clean);
    let slow = parse_slow_test_banners(&clean);

    let mut parts: Vec<String> = Vec::new();
    if !failing.is_empty() {
        parts.push(format!("failing tests: {}", failing.join(", ")));
    }
    if !slow.is_empty() {
        parts.push(format!("slow/timed-out tests: {}", slow.join(", ")));
    }

    // Generous bound (full libtest failure blocks are large) that still protects
    // the tracing sink / journal from unbounded disk use.
    let tail = truncate_output(clean.trim(), 4096);

    if parts.is_empty() {
        if tail.is_empty() {
            "cargo test reported failure with no captured output".to_string()
        } else {
            tail
        }
    } else {
        format!("{} | output: {}", parts.join("; "), tail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_gate_handles_missing_binary() {
        let result = run_smoke_gate(
            Path::new("/tmp/no-such-binary-48291"),
            &RelaunchConfig::default(),
        );
        assert!(!result.passed);
    }

    #[test]
    fn canary_gate_env_allowlist_carries_deploy_shape_names_not_hijack_vars() {
        let allow = canary_gate_env_allowlist();
        // Deploy-shape signals the healthy candidate's gates legitimately need.
        assert!(allow.iter().any(|n| n == "SIMARD_HOME"));
        assert!(allow.iter().any(|n| n == "SIMARD_PROMPT_ASSETS_DIR"));
        assert!(allow.iter().any(|n| n == "SIMARD_STATE_ROOT"));
        // Never an injection vector: an `LD_PRELOAD`-class var is not allow-listed.
        assert!(!allow.iter().any(|n| n == "LD_PRELOAD"));
        assert!(!allow.iter().any(|n| n == "GIT_SSH_COMMAND"));
    }

    #[test]
    fn is_hijack_class_env_flags_execution_hijack_vars() {
        // Loader / shell / git-transport steering vars — refused regardless of case.
        for name in [
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "ld_preload",
            "DYLD_INSERT_LIBRARIES",
            "GIT_SSH",
            "GIT_SSH_COMMAND",
            "BASH_ENV",
            "ENV",
            "SHELLOPTS",
            "BASHOPTS",
            "IFS",
        ] {
            assert!(
                is_hijack_class_env(name),
                "must refuse hijack-class var: {name}"
            );
        }
        // Legitimate deploy-shape / toolchain names are never treated as hijacks.
        for name in [
            "SIMARD_HOME",
            "SIMARD_PROMPT_ASSETS_DIR",
            "SIMARD_STATE_ROOT",
            "PATH",
            "CARGO_HOME",
            "ENVOY", // superstring of ENV must not false-positive
        ] {
            assert!(
                !is_hijack_class_env(name),
                "must not refuse a legitimate name: {name}"
            );
        }
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
}

// ─────────────────────────────────────────────────────────────────────────────
// TDD (Problem 1 — canary-gate convergence): FAILING tests, written first.
//
// These specify the fix for the persistently-RED self-deploy canary (deploy
// 928cd7da reddens identically every tick, blocking self-deploy convergence).
// They MUST fail against the current code and pass once the fix lands.
//
// The fix must run gate subprocesses under an `env_clear()` + narrow allow-list
// (mirroring `self_deploy::source_prep::scrub_git_env`) so (a) a hostile ambient
// env cannot hijack a gate and (b) the canary is verified in the same scrubbed
// shape the deployed binary will ship in. That contract is asserted here purely
// through OBSERVABLE gate behavior — not by coupling to an internal helper name:
//   * Bidirectional gate verdict: a healthy candidate goes GREEN *because* the
//     ambient hijack was stripped; an unhealthy candidate stays fail-closed RED.
//
// Constraints honoured: additive, fail-closed preserved, intent-revealing names only,
// `tracing`/OTel only (no `print!`/`println!`).
#[cfg(all(test, unix))]
mod convergence_tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    fn write_exe(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        {
            let mut f = fs::File::create(&path).expect("create fake candidate binary");
            f.write_all(body.as_bytes())
                .expect("write candidate script");
        }
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn unique_tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "simard-gate-tdd-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // GREEN side of the bidirectional verdict AND the load-bearing convergence
    // proof: a HEALTHY candidate that refuses to run under a hijacked env passes
    // ONLY when the gate spawned it in a scrubbed env. Current code inherits the
    // full ambient env (no scrub) → the probe leaks → gate FAILS (RED). After the
    // fix wires `scrub_gate_env` into the gate spawn → probe stripped → PASS.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn healthy_candidate_passes_only_in_a_scrubbed_gate_env() {
        let dir = unique_tmp("healthy");
        let bin = write_exe(
            &dir,
            "candidate",
            "#!/bin/sh\n\
             if [ -n \"$SIMARD_GATE_HIJACK_PROBE\" ]; then\n\
             echo 'ambient env leaked into gate' >&2; exit 3; fi\n\
             exit 0\n",
        );
        let config = RelaunchConfig::default();

        // SAFETY: serialized by the cognitive_memory serial key (whole-binary);
        // no concurrent test reads this var.
        unsafe { std::env::set_var("SIMARD_GATE_HIJACK_PROBE", "leak") };
        let results = verify_canary(&bin, &[RelaunchGate::Smoke], &config).unwrap();
        unsafe { std::env::remove_var("SIMARD_GATE_HIJACK_PROBE") };

        assert_eq!(results.len(), 1);
        assert!(
            results[0].passed,
            "a healthy candidate must be gated in a scrubbed env (ambient hijack stripped); got: {}",
            results[0].detail
        );
    }

    // RED side of the bidirectional verdict: an unhealthy candidate stays
    // fail-closed regardless of env. Locks that the fix does NOT weaken the gate.
    #[test]
    fn unhealthy_candidate_stays_fail_closed_red() {
        let dir = unique_tmp("unhealthy");
        let bin = write_exe(&dir, "candidate", "#!/bin/sh\nexit 1\n");
        let config = RelaunchConfig::default();

        let results = verify_canary(&bin, &[RelaunchGate::Smoke], &config).unwrap();
        assert_eq!(results.len(), 1);
        assert!(
            !results[0].passed,
            "an unhealthy candidate must stay RED (fail-closed)"
        );
        assert!(!all_gates_passed(&results));
    }

    // The additive `canary_env` knob (#4440): a var stripped by the deny-by-
    // default floor is re-injected when the operator allow-lists its NAME, so a
    // candidate that legitimately REQUIRES that signal goes GREEN — without
    // widening the base floor or inheriting the daemon's whole ambient env.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn canary_env_allowlist_reinjects_a_required_signal() {
        let dir = unique_tmp("allowlist");
        // Candidate is healthy ONLY when it can see the allow-listed signal.
        let bin = write_exe(
            &dir,
            "candidate",
            "#!/bin/sh\n\
             if [ \"$SIMARD_CANARY_ALLOWLISTED\" = \"present\" ]; then exit 0; fi\n\
             echo 'required signal missing' >&2; exit 4\n",
        );

        // SAFETY: serialized by the cognitive_memory serial key (whole-binary);
        // no concurrent test reads this var.
        unsafe { std::env::set_var("SIMARD_CANARY_ALLOWLISTED", "present") };

        // Not allow-listed → stripped by the floor → candidate reddens.
        let denied =
            verify_canary(&bin, &[RelaunchGate::Smoke], &RelaunchConfig::default()).unwrap();
        assert!(
            !denied[0].passed,
            "deny-by-default: an un-listed var must be stripped, reddening the gate"
        );

        // Allow-listed by NAME → re-injected → candidate goes green.
        let config = RelaunchConfig {
            canary_env: vec!["SIMARD_CANARY_ALLOWLISTED".to_string()],
            ..RelaunchConfig::default()
        };
        let allowed = verify_canary(&bin, &[RelaunchGate::Smoke], &config).unwrap();
        unsafe { std::env::remove_var("SIMARD_CANARY_ALLOWLISTED") };

        assert!(
            allowed[0].passed,
            "an allow-listed var must be re-injected so a healthy candidate passes; got: {}",
            allowed[0].detail
        );
    }

    // SEC-D3 (defense-in-depth): a hijack-class NAME placed in `canary_env` must
    // NOT be re-injected — the code-enforced denylist keeps the docstring's
    // "`LD_PRELOAD`-class variables are never allow-listable" guarantee true even
    // when a less-trusted source populates the allow-list. Uses `GIT_SSH_COMMAND`
    // (matches the `GIT_SSH` prefix) because it is inert for `/bin/sh` yet is a
    // real ambient-hijack vector for any git the candidate shells out to.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn hijack_class_name_in_canary_env_is_never_reinjected() {
        let dir = unique_tmp("hijackdeny");
        // Candidate is healthy ONLY when the hijack var is absent from its env.
        let bin = write_exe(
            &dir,
            "candidate",
            "#!/bin/sh\n\
             if [ -n \"$GIT_SSH_COMMAND\" ]; then\n\
             echo 'hijack var leaked into gate' >&2; exit 5; fi\n\
             exit 0\n",
        );

        // SAFETY: serialized by the cognitive_memory serial key (whole-binary);
        // no concurrent test reads this var.
        unsafe { std::env::set_var("GIT_SSH_COMMAND", "malicious --oProxyCommand") };

        // Even though the operator allow-listed the NAME, the denylist refuses it.
        let config = RelaunchConfig {
            canary_env: vec!["GIT_SSH_COMMAND".to_string()],
            ..RelaunchConfig::default()
        };
        let results = verify_canary(&bin, &[RelaunchGate::Smoke], &config).unwrap();
        unsafe { std::env::remove_var("GIT_SSH_COMMAND") };

        assert_eq!(results.len(), 1);
        assert!(
            results[0].passed,
            "a hijack-class name must be refused re-injection (stripped); got: {}",
            results[0].detail
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TDD (Problem: the canary `UnitTest` gate is undiagnosable): FAILING tests,
// written first. Today `run_unit_test_gate` truncates ONLY `stderr` to 200 chars
// and discards `stdout` — but libtest prints the failing-test name and the
// "has been running for over 60 seconds" banner to STDOUT, so every refusal is an
// opaque `tests failed (exit 101): …` tail with no failing name.
//
// These specify the diagnosability seams the fix must add. They are asserted
// against pure helpers (so no `cargo test` is shelled out from within the suite)
// and reference functions that DO NOT yet exist, so they fail to compile until
// the fix lands, then pass.
//
// Contract (all additive, fail-closed preserved, NO timing bounds):
//   * `parse_failing_test_names` — libtest `… FAILED` lines + the `failures:`
//     block → deduped, ordered names; empty on a clean/garbage capture; a single
//     bounded forward scan (ReDoS-safe) on pathological input.
//   * `parse_slow_test_banners` — `has been running for over N seconds` lines →
//     names, kept SEPARATE so a *timeout* red is distinguishable from an
//     *assertion* red.
//   * `summarize_test_failure` — the compact `detail` built from the FULL
//     stdout+stderr; names the failing tests; never empty; never reads as a pass.
//   * `sanitize_gate_capture` — strip ANSI escapes and C0 control bytes
//     (log-injection / terminal-escape-spoofing defense) while preserving
//     printable text and newlines.
//   * `UnitTest` stays in `default_gates()` (the do-not-remove band-aid guard).
#[cfg(test)]
mod diagnosability_tdd {
    use super::*;

    // A representative libtest failure capture: two assertion failures reported
    // both on their `… FAILED` running lines AND in the trailing `failures:`
    // block (so the parser must dedup), plus a passing test and the summary line.
    const LIBTEST_FAILURE_CAPTURE: &str = "\
running 3 tests
test foo::alpha ... ok
test foo::beta ... FAILED
test bar::gamma ... FAILED

failures:

---- foo::beta stdout ----
thread 'foo::beta' panicked at src/foo.rs:10:5:
assertion failed: left == right

failures:
    foo::beta
    bar::gamma

test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
";

    #[test]
    fn parse_failing_test_names_extracts_deduped_failed_tests() {
        let names = parse_failing_test_names(LIBTEST_FAILURE_CAPTURE);
        assert!(
            names.iter().any(|n| n == "foo::beta"),
            "must name the failing test foo::beta: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "bar::gamma"),
            "must name the failing test bar::gamma: {names:?}"
        );
        assert_eq!(
            names.len(),
            2,
            "each failing test named exactly once (dedup `… FAILED` + `failures:` block): {names:?}"
        );
    }

    #[test]
    fn parse_failing_test_names_empty_on_all_green_or_garbage() {
        assert!(parse_failing_test_names("test result: ok. 5 passed; 0 failed").is_empty());
        assert!(parse_failing_test_names("").is_empty());
        assert!(parse_failing_test_names("not libtest output at all\nrandom line").is_empty());
    }

    #[test]
    fn parse_failing_test_names_is_linear_on_pathological_input() {
        // No wall-clock assertion: a huge non-matching capture must be handled by
        // a single bounded forward line-scan (ReDoS-safe) and yield no names — a
        // super-linear parser would hang here instead of returning.
        let huge_single_line = "x".repeat(2_000_000);
        assert!(parse_failing_test_names(&huge_single_line).is_empty());
        let many_lines = "no match on this line\n".repeat(100_000);
        assert!(parse_failing_test_names(&many_lines).is_empty());
    }

    #[test]
    fn parse_slow_test_banners_surfaces_timeouts_separately() {
        let cap = "test squad::slow_op has been running for over 60 seconds\n\
                   test result: FAILED. 0 passed; 0 failed; 0 ignored";
        let slow = parse_slow_test_banners(cap);
        assert!(
            slow.iter().any(|n| n == "squad::slow_op"),
            "a slow-test banner must surface the timing-out test name: {slow:?}"
        );
        // A slow banner is NOT an assertion failure, so it must not leak into the
        // FAILED-name list (timeout-red stays distinguishable from assertion-red).
        assert!(
            parse_failing_test_names(cap).is_empty(),
            "a slow banner alone must not register as an assertion FAILED"
        );
    }

    #[test]
    fn summarize_test_failure_names_the_failing_tests() {
        let detail = summarize_test_failure(LIBTEST_FAILURE_CAPTURE, "");
        assert!(
            detail.contains("foo::beta"),
            "the gate detail must name the failing test(s): {detail}"
        );
        assert!(
            detail.contains("bar::gamma"),
            "the gate detail must name the failing test(s): {detail}"
        );
    }

    #[test]
    fn summarize_test_failure_never_empty_and_never_claims_success() {
        // Even when neither stream is parseable (e.g. a link error before any
        // test ran), the detail must stay meaningful and fail-closed — it must
        // never read as a pass.
        let detail = summarize_test_failure("", "error: linking with `cc` failed");
        assert!(
            !detail.trim().is_empty(),
            "a red must always produce a non-empty diagnostic detail"
        );
        assert!(
            !detail.contains("all tests passed"),
            "the failure summary must never read as a success"
        );
    }

    #[test]
    fn sanitize_gate_capture_strips_ansi_and_control_keeps_text() {
        // ANSI colour wrap + bell + backspace + carriage return around the real
        // libtest line: all control/escape bytes must go, printable text stays.
        let raw = "\u{1b}[31mtest foo::beta ... FAILED\u{1b}[0m\u{7}\u{8}\r\nplain trailing line";
        let clean = sanitize_gate_capture(raw);
        assert!(
            !clean.contains('\u{1b}'),
            "ANSI ESC must be stripped (terminal-escape-spoofing defense): {clean:?}"
        );
        assert!(
            !clean.contains('\u{7}'),
            "the bell control byte must be stripped: {clean:?}"
        );
        assert!(
            !clean.contains('\r'),
            "carriage-return line spoofing must be stripped: {clean:?}"
        );
        assert!(
            clean.contains("test foo::beta ... FAILED"),
            "printable diagnostic text must be preserved: {clean:?}"
        );
        assert!(
            clean.contains("plain trailing line"),
            "newline-separated content must be preserved: {clean:?}"
        );
    }

    #[test]
    fn unit_test_gate_stays_in_default_gates() {
        // The diagnosability fix is strictly additive: removing `UnitTest` from
        // the canary to dodge the red is the explicitly-rejected band-aid.
        assert!(
            super::super::types::default_gates().contains(&RelaunchGate::UnitTest),
            "UnitTest must remain a default canary gate (do-not-remove guard)"
        );
    }
}
