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
        // Temp dir for the `unit-test` gate's `cargo test` toolchain: rustc/cargo
        // write intermediate artifacts under $TMPDIR, so dropping it can push
        // compile temporaries onto an unintended default and falsely redden a
        // healthy candidate. Part of the universal floor by design intent.
        "TMPDIR",
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

/// The `unit-test` canary gate: shells out to `cargo test --lib` for the
/// candidate crate under the scrubbed gate env ([`scrub_gate_env`]).
///
/// Scope (#4522 root cause): the gate is scoped to `--lib` — the **exact**
/// proven-green unit scope (9262 passed, 0 failed). The prior all-targets
/// `cargo test` dragged in `tests/` integration binaries that require signals
/// the deny-by-default scrub strips, so the harness aborted (exit 101) **before**
/// emitting any `test result:` summary — reddening a genuinely-healthy candidate
/// every overseer tick and stalling self-deploy. `--lib` matches the healthy
/// baseline 1:1 and is the blocking unit-test scope the canary invariant mandates.
///
/// Diagnosability (#4522 second defect): on failure this captures **both** stdout
/// and stderr and routes them through [`parse_unit_test_failure`], which surfaces
/// the failing test name(s) and the `test result: FAILED` summary (cargo prints
/// these on **stdout**, which the prior `truncate_output(&stderr, 200)` discarded)
/// plus a bounded tail for the no-summary abort case. The detail is emitted
/// structurally via `tracing` (never `print!`/`println!`); a spawn error is a
/// louder `tracing::error!`. Fail-closed is preserved: a genuine failure still
/// reddens, now with a diagnosable detail instead of an opaque `exit 101`.
fn run_unit_test_gate(config: &RelaunchConfig) -> GateResult {
    let mut cmd = scrubbed_command("cargo", config);
    cmd.arg("test")
        .arg("--lib")
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
            let parsed = parse_unit_test_failure(&stdout, &stderr);
            // Structured, bounded, credential-redacted emission (no raw print!):
            // the failing test identity / abort signal that the crash-loop hid.
            tracing::warn!(
                target: "self_relaunch::gate",
                gate = %RelaunchGate::UnitTest,
                exit = %output.status,
                detail = %bound_gate_detail(&parsed),
                "unit-test canary gate failed"
            );
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("tests failed (exit {}): {}", output.status, parsed),
            }
        }
        Err(e) => {
            tracing::error!(
                target: "self_relaunch::gate",
                gate = %RelaunchGate::UnitTest,
                error = %e,
                "unit-test canary gate could not spawn `cargo test`"
            );
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("cargo test failed to run: {e}"),
            }
        }
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
        return s.trim().to_string();
    }
    // Char-boundary-safe truncation (never splits a multi-byte codepoint); see
    // [`super::types::next_char_boundary`] for the shared O(1) back-off.
    let end = super::types::next_char_boundary(s, max_len);
    format!("{}...", s[..end].trim())
}

/// Tail-preserving counterpart to [`truncate_output`]: keeps the **last**
/// `max_len` bytes (backed off to a UTF-8 boundary) with a leading `...` marker
/// when elided. `cargo test` prints the load-bearing `test result:` summary and
/// the `failures:` roster **last**, so a head-preserving truncation would throw
/// away exactly the diagnostic that makes a red canary legible. Char-boundary
/// safe (never panics on multi-byte input); a string already within the cap is
/// returned trimmed and unmarked.
fn truncate_output_tail(s: &str, max_len: usize) -> String {
    let trimmed = s.trim();
    if trimmed.len() <= max_len {
        return trimmed.to_string();
    }
    // Take the last `max_len` bytes, then move forward to the next char boundary
    // (shared [`super::types::next_char_boundary`]) so the retained tail is
    // always valid whole UTF-8.
    let start = super::types::next_char_boundary(trimmed, trimmed.len() - max_len);
    format!("...{}", &trimmed[start..])
}

/// Parse a failed `cargo test` invocation's combined output into a bounded,
/// high-signal, credential-safe detail (#4522 diagnosability fix).
///
/// The prior gate kept only `truncate_output(&stderr, 200)` and **discarded
/// stdout**, where cargo prints the failing test names and the `test result:
/// FAILED` summary — so a red canary was an opaque `exit 101` with no test
/// identity, making the crash-loop undiagnosable. This scans **both** streams
/// for the high-signal lines that name the failure:
///   * `... FAILED` per-test lines (the failing test identity),
///   * the `test result:` summary line,
///   * abort/compile signals (`error[…]`, `error:`, `could not compile`,
///     `panicked`) for the exit-101-with-no-summary shape.
///
/// Marker lines are de-duplicated and capped (log-flood / DoS guard), then a
/// bounded [`truncate_output_tail`] of the raw combined output is appended so
/// cargo's *last words* survive even when they were not matched as a marker.
/// The whole detail is finally tail-bounded to `MAX_DETAIL_BYTES`. It surfaces
/// only cargo's own output — never any environment contents.
fn parse_unit_test_failure(stdout: &str, stderr: &str) -> String {
    const MAX_MARKER_LINES: usize = 20;
    const TAIL_BYTES: usize = 8 * 1024;
    const MAX_DETAIL_BYTES: usize = 16 * 1024;

    fn is_marker(line: &str) -> bool {
        line.contains("... FAILED")
            || line.starts_with("test result:")
            || line.starts_with("error[")
            || line.starts_with("error:")
            || line.contains("could not compile")
            || line.contains("panicked")
    }

    let mut markers: Vec<&str> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in stdout.lines().chain(stderr.lines()) {
        let trimmed = line.trim();
        if trimmed.is_empty() || !is_marker(trimmed) {
            continue;
        }
        if seen.insert(trimmed) {
            markers.push(trimmed);
            if markers.len() >= MAX_MARKER_LINES {
                break;
            }
        }
    }

    // Raw combined output whose *tail* preserves the trailing summary/roster.
    let mut combined = String::with_capacity(stdout.len() + stderr.len() + 1);
    combined.push_str(stdout);
    if !stdout.is_empty() && !stderr.is_empty() {
        combined.push('\n');
    }
    combined.push_str(stderr);
    let tail = truncate_output_tail(&combined, TAIL_BYTES);

    let mut detail = markers.join("\n");
    if !tail.is_empty() {
        if !detail.is_empty() {
            detail.push_str("\n---\n");
        }
        detail.push_str(&tail);
    }

    // Final DoS guard, tail-preserving so the summary is never the part elided.
    truncate_output_tail(&detail, MAX_DETAIL_BYTES)
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
// TDD (Problem 2 — #4522 canary red-canary crash-loop): FAILING tests, first.
//
// The self-deploy canary's unit-test gate aborts with exit status 101 on EVERY
// overseer tick even though `cargo test --lib` is fully green (9262 passed) in a
// normal environment. Two coupled defects, both proven here through the gate's
// OBSERVABLE contract:
//
//   (1) SCOPE MISMATCH — the gate runs `cargo test` over ALL targets, so under
//       the `env_clear()`-scrubbed canary env it drags in `tests/` integration
//       binaries that require signals the deny-by-default floor strips, aborting
//       (exit 101) before any `test result:` summary is emitted. The proven-green
//       baseline is the `--lib` unit scope; the gate must match it.
//   (2) UNDIAGNOSABLE FAILURE — the failure detail is `truncate_output(&stderr,
//       200)` and DISCARDS stdout, where cargo prints the failing test identity
//       and the `test result: FAILED` summary. The loop is therefore invisible.
//
// These tests specify the additive fix and MUST fail against the current code
// (the pure helpers `parse_unit_test_failure` / `truncate_output_tail` do not yet
// exist; the gate still runs all targets and discards stdout) and pass once the
// fix lands. Constraints honoured: additive, fail-closed preserved, `tracing`/
// OTel only (no `print!`/`println!`), deny-by-default scrub never weakened.
#[cfg(test)]
mod diagnosability_tests {
    use super::*;

    // --- truncate_output_tail: cargo prints `test result:` LAST, so a tail-
    // preserving truncation (not the head-preserving `truncate_output`) is what
    // keeps the load-bearing summary visible in a bounded detail. ---

    #[test]
    fn truncate_output_tail_short_string_unchanged() {
        // Below the cap: returned verbatim (trimmed), no ellipsis marker.
        let out = truncate_output_tail("test result: FAILED", 200);
        assert_eq!(out, "test result: FAILED");
    }

    #[test]
    fn truncate_output_tail_keeps_trailing_summary() {
        // A long head of noise followed by the summary cargo emits LAST. The
        // tail truncation must retain the summary and mark the elision up front.
        let mut s = "compiling noise line that is pure prefix chatter\n".repeat(200);
        s.push_str("test result: FAILED. 9260 passed; 2 failed; 0 ignored");
        let out = truncate_output_tail(&s, 80);
        assert!(
            out.contains("test result: FAILED. 9260 passed; 2 failed"),
            "tail truncation must retain the trailing summary cargo prints last: {out}"
        );
        assert!(
            out.starts_with("..."),
            "an elided tail must be marked with a leading ellipsis: {out}"
        );
        assert!(
            out.len() <= 84,
            "tail must be bounded to ~max_len (+ellipsis), got {} bytes",
            out.len()
        );
    }

    #[test]
    fn truncate_output_tail_is_char_boundary_safe() {
        // A cut that lands mid-UTF-8 must back off to a boundary, never panic.
        let s = "é".repeat(50); // 100 bytes of 2-byte chars
        let out = truncate_output_tail(&s, 5); // 5 bytes lands mid-char
        assert!(out.starts_with("..."), "elided tail must be marked: {out}");
        assert!(
            out.trim_start_matches('.').chars().all(|c| c == 'é'),
            "retained tail must be valid whole UTF-8 chars: {out}"
        );
    }

    // --- parse_unit_test_failure: the diagnosability core. Captures BOTH
    // streams and surfaces the failing-test identity + summary, so a genuine
    // red canary is diagnosable instead of an opaque `exit 101`. ---

    #[test]
    fn unit_test_failure_surfaces_failing_test_name_and_summary() {
        // cargo prints the failing test name and the FAILED summary on STDOUT —
        // exactly what the old `truncate_output(&stderr, 200)` threw away.
        let stdout = "\
running 3 tests
test foo::works ... ok
test foo::my_failing_case ... FAILED
test bar::also_ok ... ok

failures:

---- foo::my_failing_case stdout ----
thread 'foo::my_failing_case' panicked at src/foo.rs:42:9:
assertion `left == right` failed

failures:
    foo::my_failing_case

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured";
        let detail = parse_unit_test_failure(stdout, "");
        assert!(
            detail.contains("my_failing_case"),
            "must surface the failing test name (stdout was previously discarded): {detail}"
        );
        assert!(
            detail.contains("test result: FAILED"),
            "must surface the FAILED summary line: {detail}"
        );
    }

    #[test]
    fn unit_test_abort_without_summary_surfaces_tail() {
        // The observed #4522 shape: cargo aborts (exit 101) with NO `test
        // result:` summary at all — a compile/link/harness abort. The detail
        // must STILL be non-empty and carry the high-signal abort line, so the
        // loop is diagnosable rather than an opaque exit code.
        let stderr = "\
   Compiling simard v0.1.0 (/canary)
error[E0433]: failed to resolve: use of undeclared crate or module `nope`
  --> tests/needs_env.rs:1:5
error: could not compile `simard` (test \"needs_env\") due to 1 previous error";
        let detail = parse_unit_test_failure("", stderr);
        assert!(
            !detail.trim().is_empty(),
            "an exit-101 abort must never yield an empty detail (the whole bug)"
        );
        assert!(
            detail.contains("could not compile") || detail.contains("error[E0433]"),
            "must surface the abort/compile signal when there is no summary line: {detail}"
        );
    }

    #[test]
    fn unit_test_failure_detail_is_bounded() {
        // DoS / log-flood guard: a gate's combined output can be arbitrarily
        // large, but the parsed detail must stay bounded before it is emitted.
        let huge = "error: spurious repeated noise line to flood the buffer\n".repeat(50_000);
        let detail = parse_unit_test_failure(&huge, "");
        assert!(
            detail.len() <= 16 * 1024,
            "parsed detail must be bounded (DoS guard), got {} bytes",
            detail.len()
        );
    }

    #[test]
    fn unit_test_failure_never_dumps_environment() {
        // Redaction posture: the parser surfaces cargo's own output only; it is
        // given no environment and must not fabricate/echo any. A benign token
        // in the OUTPUT is fine to pass through (bound_gate_detail redacts at
        // emission); the invariant here is simply "no env keys leak in".
        let detail = parse_unit_test_failure("test result: FAILED. 1 failed", "");
        assert!(
            !detail.contains("SIMARD_HOME") && !detail.contains("PATH="),
            "parser must not surface environment contents: {detail}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TDD (#4522 SCOPE fix): a healthy candidate must PASS the unit-test gate.
//
// Hermetic proof of the root-cause scope alignment. A tiny fixture crate holds:
//   * a passing LIBRARY unit test (`src/lib.rs`), and
//   * an integration target (`tests/`) that reddens the scrubbed gate env
//     (models the real `tests/` binaries that need SIMARD_* / heavier signals).
//
// Current code runs `cargo test` over ALL targets → the integration target runs
// → the gate reddens (RED) → this test FAILS. After the fix aligns the gate to
// the proven-green `--lib` unit scope, only the library test runs → GREEN → this
// test passes. It is deliberately real (spawns cargo) and fails LOUDLY if the
// toolchain is absent (no silent skip that would mask a broken gate).
#[cfg(all(test, unix))]
mod scope_tests {
    use super::*;
    use std::fs;

    #[test]
    fn unit_test_gate_passes_for_healthy_candidate_under_lib_scope() {
        let tmp = tempfile::tempdir().expect("create fixture tempdir");
        let root = tmp.path();

        // Minimal, dependency-free fixture crate (offline-buildable).
        fs::write(
            root.join("Cargo.toml"),
            "[package]\n\
             name = \"canary_fixture\"\n\
             version = \"0.0.0\"\n\
             edition = \"2021\"\n\
             \n\
             [lib]\n\
             path = \"src/lib.rs\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        // Healthy library unit test — passes under any scrubbed env.
        fs::write(
            root.join("src/lib.rs"),
            "#[cfg(test)]\n\
             mod tests {\n\
             #[test]\n\
             fn healthy_unit() { assert_eq!(2 + 2, 4); }\n\
             }\n",
        )
        .unwrap();
        // Integration target that reddens the ALL-TARGETS scope: it requires a
        // signal the deny-by-default gate env strips, so it fails there. `--lib`
        // must NOT build or run it (that is the whole fix).
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(
            root.join("tests/needs_env.rs"),
            "#[test]\n\
             fn requires_scrubbed_away_signal() {\n\
             assert!(\n\
             std::env::var(\"CANARY_FIXTURE_INTEGRATION_SIGNAL\").is_ok(),\n\
             \"integration target needs a signal the scrubbed gate env strips\"\n\
             );\n\
             }\n",
        )
        .unwrap();

        let config = RelaunchConfig {
            manifest_dir: root.to_path_buf(),
            canary_target_dir: root.join("target"),
            ..RelaunchConfig::default()
        };

        // Fail loudly, not silently, if the toolchain is genuinely unavailable —
        // a canary gate that cannot run cargo is itself a defect, not a skip.
        let toolchain_ok = scrubbed_command("cargo", &config)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(
            toolchain_ok,
            "cargo toolchain must be reachable under the scrubbed gate env; \
             a gate that cannot invoke cargo is a defect (no silent skip)"
        );

        let result = run_unit_test_gate(&config);
        assert!(
            result.passed,
            "a healthy candidate must PASS the unit-test gate once it is scoped \
             to `--lib` (the proven-green 9262/0 baseline); detail: {}",
            result.detail
        );
    }
}
