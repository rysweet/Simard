use std::path::{Path, PathBuf};
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

/// Create a fresh, private RAII isolation directory for one `unit-test` gate
/// run. The gate overrides `SIMARD_STATE_ROOT` / `SIMARD_HOME` / `HOME` /
/// `TMPDIR` to this dir so the in-process lib-test suite resolves an **empty,
/// private** state root ([`crate::runtime_config`]: `SIMARD_STATE_ROOT` else
/// `$HOME/.simard`) instead of the live daemon's — it therefore cannot open the
/// daemon's WAL / cognitive-store or bind its socket (the #4558 abort). The dir
/// is randomized (`tempfile`, `O_EXCL`) and canonicalized; if it resolves
/// **outside** the temp root (a symlink escape) the call fails so the caller can
/// fail closed rather than run against a rediscovered production state root.
///
/// Returns `Err` on any setup failure. The caller ([`run_unit_test_gate`]) then
/// returns a **failing** `GateResult` — never a silent non-hermetic fallback to
/// the daemon's live state root (a gate must never write production runtime
/// state).
fn unit_test_isolation_dir() -> std::io::Result<tempfile::TempDir> {
    let dir = tempfile::Builder::new()
        .prefix("simard-unit-test-gate-")
        .tempdir()?;
    // Symlink-escape defense: the isolation path must stay inside the temp root
    // so a test cannot rediscover the production state root through a symlink.
    let canon = dir.path().canonicalize()?;
    let temp_root = std::env::temp_dir().canonicalize()?;
    if !canon.starts_with(&temp_root) {
        return Err(std::io::Error::other(format!(
            "isolation dir {} escaped temp root {}",
            canon.display(),
            temp_root.display()
        )));
    }
    Ok(dir)
}

/// The real, pre-override `HOME` — the value used to derive the toolchain roots
/// **before** the hermetic `HOME` override redirects `HOME` to the empty temp
/// dir. Falls back to `/` (an absolute, non-writable root) only if `HOME` is
/// entirely unset, which keeps the derived toolchain paths absolute.
fn real_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Resolve absolute `CARGO_HOME` / `RUSTUP_HOME` for the gate child, pinned from
/// the **real, pre-override** `HOME` (or the ambient values when set), so the
/// hermetic `HOME` override cannot strand `cargo`/`rustup`.
///
/// Load-bearing invariant (see `docs/reference/hermetic-unit-test-gate.md`):
/// `cargo`/`rustup` resolve their toolchain from `CARGO_HOME` / `RUSTUP_HOME`
/// and, *only when those are unset*, fall back to `$HOME/.cargo` /
/// `$HOME/.rustup`. Under a clean systemd unit those vars are frequently absent
/// (the daemon relies on the `$HOME/.cargo` default). If `HOME` is then
/// overridden to an **empty** temp dir while they are unset, `cargo test` hunts
/// the toolchain under the empty temp `$HOME/.cargo` and aborts — a fresh
/// #4558-class self-inflicted red. Pinning them here from the real `HOME`
/// (preferring ambient values) prevents that.
fn resolve_toolchain_home() -> (PathBuf, PathBuf) {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| real_home().join(".cargo"));
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| real_home().join(".rustup"));
    (cargo_home, rustup_home)
}

/// Upper bound (bytes) on the extracted `unit-test` failure detail. Raised from
/// the old 200-byte stderr-only head so the failing test NAME survives; the
/// downstream deploy `bound_detail` 512-byte cap still governs the final size.
const FAILURE_DETAIL_MAX_BYTES: usize = 4096;

/// Extract the operator-actionable failure block from a `cargo test` run.
///
/// Scans the COMBINED stdout+stderr stream (stdout carries `test … FAILED` /
/// `failures:`; stderr carries `panicked at …`) and returns the first matching
/// marker block with the failing test NAME preserved, in precedence order:
/// `failures:` > `panicked at …` > `test … FAILED`. The block is captured from
/// the **start of the marker's line** so a name that precedes `panicked at`
/// (`thread '…' panicked at …`) or heads the `failures:` dump survives. With no
/// marker at all (e.g. a linker OOM or an abort before any test line) it falls
/// back to the tail of the combined stream so the detail is never empty. The
/// result is UTF-8-safely clamped to [`FAILURE_DETAIL_MAX_BYTES`] — never a raw
/// dump — so an enlarged detail cannot leak unrelated output into logs.
fn extract_failure_detail(stdout: &str, stderr: &str) -> String {
    let combined = if stdout.is_empty() {
        stderr.to_string()
    } else if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    };

    // First match wins, in precedence order. `failures:` heads the richest
    // libtest block (test name AND panic message); `panicked at` carries the
    // panic site (name precedes it on the same line); `FAILED` is the bare
    // per-test result line (`test <name> … FAILED`).
    for marker in ["failures:", "panicked at", "FAILED"] {
        if let Some(pos) = combined.find(marker) {
            let line_start = combined[..pos].rfind('\n').map_or(0, |nl| nl + 1);
            let block = combined[line_start..].trim();
            return clamp_utf8(block, FAILURE_DETAIL_MAX_BYTES);
        }
    }

    // No structured marker: bounded, non-empty tail so the operator sees the
    // real output (e.g. a linker/OOM abort) rather than an empty detail.
    let trimmed = combined.trim();
    clamp_utf8_tail(trimmed, FAILURE_DETAIL_MAX_BYTES)
}

/// UTF-8-safe head clamp to at most `max` bytes (never splits a codepoint,
/// never appends an ellipsis that could exceed `max`).
fn clamp_utf8(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// UTF-8-safe tail clamp to at most `max` bytes (keeps the END of `s`, which is
/// where an abort's real error typically lands).
fn clamp_utf8_tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

/// Run the `unit-test` canary gate: `cargo test` against `config.manifest_dir`,
/// **hermetically** isolated so a running daemon cannot red-canary a passing
/// suite (#4558), and **diagnosably** — a real failure names the failing test.
///
/// Isolation: after [`scrub_gate_env`], four keys (`SIMARD_STATE_ROOT`,
/// `SIMARD_HOME`, `HOME`, `TMPDIR`) are overridden **per child** to a fresh
/// [`unit_test_isolation_dir`] and `current_dir` is set to the manifest dir, so
/// the in-process test suite resolves an empty private state root and cannot
/// open the live daemon's WAL / cognitive-store or bind its socket. The
/// toolchain (`CARGO_HOME` / `RUSTUP_HOME`) is pinned from the real
/// pre-override `HOME` **before** the `HOME` override so the redirect cannot
/// strand `cargo`/`rustup` (see [`resolve_toolchain_home`]). Fail-closed: a
/// temp-dir setup failure returns a failing `GateResult`, never a non-hermetic
/// run against the live state root.
fn run_unit_test_gate(config: &RelaunchConfig) -> GateResult {
    // Fail closed: no isolation dir -> failing GateResult, never a live-root fallback.
    let isolation = match unit_test_isolation_dir() {
        Ok(dir) => dir,
        Err(e) => {
            return GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("unit-test gate could not create an isolated state root: {e}"),
            };
        }
    };
    let iso = isolation.path();

    // Pin the toolchain from the REAL (pre-override) HOME before HOME is
    // redirected to the empty temp dir (otherwise cargo/rustup hunt the
    // toolchain under the empty temp $HOME/.cargo and abort — a #4558-class red).
    let (cargo_home, rustup_home) = resolve_toolchain_home();

    let mut cmd = scrubbed_command("cargo", config);
    cmd.arg("test")
        .arg("--manifest-path")
        .arg(config.manifest_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&config.canary_target_dir)
        .env("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs())
        // Toolchain pin — absolute, resolved from the real HOME, set explicitly
        // so the HOME override below cannot strand cargo/rustup.
        .env("CARGO_HOME", &cargo_home)
        .env("RUSTUP_HOME", &rustup_home)
        // Hermetic override — applied AFTER scrub_gate_env so the isolated temp
        // path wins over the allow-listed SIMARD_HOME / SIMARD_STATE_ROOT.
        .env("SIMARD_STATE_ROOT", iso)
        .env("SIMARD_HOME", iso)
        .env("HOME", iso)
        .env("TMPDIR", iso)
        .current_dir(&config.manifest_dir);

    match cmd.output() {
        Ok(output) if output.status.success() => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: true,
            detail: "all tests passed".to_string(),
        },
        Ok(output) => {
            // Capture BOTH streams (#4558): the failing test name lives on
            // stdout (`failures:` / `test … FAILED`); a panic site lives on
            // stderr (`panicked at …`). The old stderr-only, 200-byte head
            // landed on a progress-spinner fragment (`Drop t…`) and hid it.
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = extract_failure_detail(&stdout, &stderr);
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: format!("tests failed (exit {}): {}", output.status, detail),
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

    // ─────────────────────────────────────────────────────────────────────────
    // TDD (Problem 2 — #4558 diagnosable unit-test gate): FAILING tests, written
    // first. These specify `extract_failure_detail(stdout, stderr)`, the pure
    // helper the hermetic gate uses so a red tree's failing test NAME survives
    // into `failing_detail` instead of being lost to the old stderr-only,
    // 200-byte spinner-fragment head (`Drop t…`).
    //
    // Contract (see docs/reference/hermetic-unit-test-gate.md):
    //   * Scans the COMBINED stdout+stderr stream (libtest writes `failures:` /
    //     `test … FAILED` to stdout; a panic writes `panicked at …` to stderr).
    //   * Extracts the FIRST structured marker block, test-name first, in
    //     precedence order: `failures:` > `panicked at …` > `test … FAILED`.
    //   * UTF-8-safely clamps the result to 4096 bytes (raised from 200).
    //   * No-marker input falls back to a bounded, non-empty tail — never a
    //     raw dump, never empty for non-empty input.
    //
    // They MUST fail to compile/run against the current code (the function does
    // not exist yet) and pass once the fix lands. Pure — no subprocess, no env
    // mutation, so no `serial(cognitive_memory)` key is required. `tracing`/OTel
    // discipline is a runtime concern; these assert only the extractor's return.
    // ─────────────────────────────────────────────────────────────────────────

    /// A realistic libtest `failures:` block on **stdout** — the richest marker
    /// (test name AND panic message) and the one the old stderr-only capture
    /// missed entirely.
    const FAILURES_BLOCK_STDOUT: &str = "\
running 2 tests
test tests::fixture_passes_cleanly ... ok
test tests::fixture_panics_when_toggled ... FAILED

failures:

---- tests::fixture_panics_when_toggled stdout ----
thread 'tests::fixture_panics_when_toggled' panicked at tests/fixtures/unit_test_gate_fixture/lib.rs:44:13:
intentional fixture failure for red-canary detail extraction
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    tests::fixture_panics_when_toggled

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";

    /// What cargo itself prints to **stderr** for a failed test run — carries no
    /// test name, so a stderr-only capture cannot name the failure.
    const CARGO_STDERR: &str = "error: test failed, to rerun pass `--lib`\n";

    #[test]
    fn extract_failure_detail_names_failing_test_from_failures_block() {
        let detail = extract_failure_detail(FAILURES_BLOCK_STDOUT, CARGO_STDERR);
        assert!(
            detail.contains("fixture_panics_when_toggled"),
            "failing test NAME must survive into the detail; got: {detail}"
        );
        assert!(
            detail.contains("failures:"),
            "the `failures:` marker block must be selected; got: {detail}"
        );
        // The #4558 regression: the detail must NOT be a truncated progress
        // spinner fragment that hides which test failed.
        assert!(
            !detail.trim().starts_with("Drop t"),
            "detail must not be a spinner fragment; got: {detail}"
        );
    }

    #[test]
    fn extract_failure_detail_names_failing_test_from_panicked_at_on_stderr() {
        // No `failures:` anywhere; the panic site is on stderr (uncaptured
        // panic / abort path). The extractor must still surface it.
        let stderr = "\
thread 'tests::open_wal_lock' panicked at src/cognitive_memory/open_guard.rs:88:9:
state root already locked by the live daemon
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
";
        let detail = extract_failure_detail("", stderr);
        assert!(
            detail.contains("panicked at"),
            "the `panicked at` marker must be selected; got: {detail}"
        );
        assert!(
            detail.contains("open_wal_lock"),
            "the panicking test/context name must survive; got: {detail}"
        );
    }

    #[test]
    fn extract_failure_detail_names_failing_test_from_failed_line() {
        // Lowest-precedence marker: only a per-test `... FAILED` result line,
        // no `failures:` block and no `panicked at` (e.g. a `#[should_panic]`
        // that did not panic, or a non-panicking assertion harness).
        let stdout = "\
running 1 test
test tests::rpc_dials_live_socket ... FAILED
";
        let detail = extract_failure_detail(stdout, "");
        assert!(
            detail.contains("rpc_dials_live_socket"),
            "the FAILED test name must survive; got: {detail}"
        );
        assert!(
            detail.contains("FAILED"),
            "the `FAILED` marker must be present; got: {detail}"
        );
    }

    #[test]
    fn extract_failure_detail_prefers_failures_block_over_panicked_at() {
        // Both markers present with DIFFERENT names: `failures:` wins, so the
        // richer name+panic block is returned rather than a bare panic site.
        let stdout = "\
test tests::the_failures_block_test ... FAILED

failures:

---- tests::the_failures_block_test stdout ----
assertion `left == right` failed

failures:
    tests::the_failures_block_test
";
        let stderr = "thread 'tests::a_different_panic_test' panicked at src/x.rs:1:1:\nboom\n";
        let detail = extract_failure_detail(stdout, stderr);
        assert!(
            detail.contains("the_failures_block_test"),
            "the `failures:` block must take precedence; got: {detail}"
        );
    }

    #[test]
    fn extract_failure_detail_clamps_to_4096_bytes() {
        // A marker followed by a very long body must be bounded to 4096 bytes
        // (raised from the old 200) — the failing name still fits well inside.
        let mut stdout = String::from("failures:\n    tests::huge_output_test\n");
        stdout.push_str(&"x".repeat(20_000));
        let detail = extract_failure_detail(&stdout, "");
        assert!(
            detail.len() <= 4096,
            "detail must be clamped to <= 4096 bytes; got {} bytes",
            detail.len()
        );
        assert!(
            detail.contains("huge_output_test"),
            "the failing name must survive the clamp; got: {detail}"
        );
    }

    #[test]
    fn extract_failure_detail_is_utf8_boundary_safe() {
        // Multi-byte characters straddling the 4096-byte clamp must never panic
        // and must yield valid UTF-8 (guaranteed by the `String` return, but
        // the clamp must not truncate mid-codepoint and lose data or abort).
        let mut stdout = String::from("failures:\n    tests::utf8_boundary_test\n");
        stdout.push_str(&"é".repeat(4000)); // 2 bytes each → ~8000 bytes body
        let detail = extract_failure_detail(&stdout, "");
        assert!(
            detail.len() <= 4096,
            "clamped detail must be <= 4096 bytes; got {} bytes",
            detail.len()
        );
        // Round-trips as valid UTF-8 without panicking (implicit in `String`).
        assert!(
            detail.contains("utf8_boundary_test"),
            "the failing name must survive the UTF-8-safe clamp; got: {detail}"
        );
    }

    #[test]
    fn extract_failure_detail_no_marker_falls_back_to_bounded_nonempty_tail() {
        // No structured marker at all (e.g. a linker OOM or an abort before any
        // test line). The detail must be non-empty (so the operator sees
        // *something*) and still bounded.
        let stdout = "";
        let stderr = "collect2: fatal error: ld terminated with signal 9 [Killed]\n";
        let detail = extract_failure_detail(stdout, stderr);
        assert!(
            !detail.trim().is_empty(),
            "no-marker input must still yield a non-empty detail"
        );
        assert!(
            detail.len() <= 4096,
            "fallback detail must be bounded; got {} bytes",
            detail.len()
        );
        assert!(
            detail.contains("ld terminated") || detail.contains("fatal error"),
            "fallback must carry a tail of the real output; got: {detail}"
        );
    }

    #[test]
    fn extract_failure_detail_scans_stdout_when_stderr_empty() {
        // Proves stdout is scanned even when stderr is empty — the exact #4558
        // gap (the failing name lives on stdout; the old code read only stderr).
        let detail = extract_failure_detail(FAILURES_BLOCK_STDOUT, "");
        assert!(
            detail.contains("fixture_panics_when_toggled"),
            "stdout must be scanned for the failing name; got: {detail}"
        );
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
