use std::path::Path;
use std::process::Command;

use super::types::{GateResult, RelaunchConfig, RelaunchGate};
use crate::error::{SimardError, SimardResult};

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

/// Build the scrubbed `cargo test` [`Command`] for the `unit-test` canary gate,
/// isolated to a **fresh ephemeral state root** (#4628).
///
/// Root cause of the #4628 crash-loop: the canary ran `cargo test` against a
/// target that SHARED the live daemon's state root, so unit tests that open the
/// cognitive store ([`crate::cognitive_memory::open_guard`]'s `acquire_open_lock`)
/// and the typed-OODA sqlite outcome store collided with the running daemon's
/// single-writer locks — `cognitive store is held open by another process` and
/// `database is locked` — reddening every self-deploy tick (exit status 101).
///
/// The fix overrides the canonical Simard state-root env vars onto a throwaway
/// [`tempfile::TempDir`] so the canary opens its OWN stores and never contends:
///   * `SIMARD_STATE_ROOT` — canonical; takes precedence for store location.
///   * `SIMARD_HOME` — overridden for consistency so nothing resolves back to
///     the live root.
///   * `SIMARD_MEMORY_SOCKET` — **removed** (`env_remove`) so the canary never
///     dials the live daemon's in-memory socket.
///
/// The three overrides are applied **after** [`scrubbed_command`] (which runs
/// `env_clear()` + the [`scrub_gate_env`] allow-list re-injection), so they win
/// last-write-wins over any inherited or allow-listed live value — mirroring the
/// `CARGO_BUILD_JOBS` override ordering. SEC-D3: these post-scrub `.env()`
/// overrides are confined to the unit-test gate and to these three known
/// `SIMARD_*` names; the pattern is deliberately NOT copied into the other gates
/// (`rpc-health` in particular must keep dialing the live daemon).
///
/// The returned [`tempfile::TempDir`] guard OWNS the isolated root and MUST be
/// held until after the subprocess exits (`cmd.output()`), or the root would be
/// deleted mid-run; cleanup happens on its drop. Fails closed (REQ-SEC-5): on a
/// `TempDir` creation error this returns `Err` and the caller reddens the gate —
/// it never falls back to the live daemon's state root.
fn build_unit_test_command(config: &RelaunchConfig) -> SimardResult<(Command, tempfile::TempDir)> {
    // REQ-SEC-4: randomized, 0700 directory under the system temp dir (mkdtemp)
    // — no predictable path, closing TOCTOU/symlink races on a shared host.
    let state_root = tempfile::TempDir::new().map_err(|e| SimardError::PersistentStoreIo {
        store: "canary-unit-test-state-root".to_string(),
        action: "create-isolated-state-root".to_string(),
        path: std::env::temp_dir(),
        reason: e.to_string(),
    })?;
    // REQ-SEC-2: an absolute path (no CWD-relative escape/collision against the
    // subprocess working dir). A `TempDir` under the system temp dir is always
    // absolute; assert to keep the invariant loud if that ever changes.
    debug_assert!(
        state_root.path().is_absolute(),
        "ephemeral canary state root must be absolute"
    );

    let mut cmd = scrubbed_command("cargo", config);
    cmd.arg("test")
        .arg("--manifest-path")
        .arg(config.manifest_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&config.canary_target_dir)
        .env("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs());

    // #4628 state-root isolation — applied AFTER the scrub (last-write-wins).
    cmd.env("SIMARD_STATE_ROOT", state_root.path());
    cmd.env("SIMARD_HOME", state_root.path());
    cmd.env_remove("SIMARD_MEMORY_SOCKET");

    Ok((cmd, state_root))
}

/// Fail-closed RED result for the `unit-test` gate (REQ-SEC-5): returned when an
/// isolated ephemeral state root could not be created. The canary MUST NOT fall
/// back to the live daemon's state root — doing so would reintroduce the #4628
/// single-writer lock contention and leak canary writes into production state.
fn unit_test_gate_failed_closed(reason: &str) -> GateResult {
    GateResult {
        gate: RelaunchGate::UnitTest,
        passed: false,
        detail: format!(
            "could not create an isolated state root for the canary unit-test gate: {reason}"
        ),
    }
}

fn run_unit_test_gate(config: &RelaunchConfig) -> GateResult {
    // Fail closed (REQ-SEC-5) if the isolated state root cannot be created — the
    // canary must never contend with, or write into, the live daemon's state.
    let (mut cmd, _state_root) = match build_unit_test_command(config) {
        Ok(built) => built,
        Err(e) => {
            let reason = e.to_string();
            tracing::error!(
                target: "self_relaunch::gate",
                error = %bound_gate_detail(&reason),
                "canary unit-test gate failed closed: could not isolate state root"
            );
            return unit_test_gate_failed_closed(&reason);
        }
    };
    // `_state_root` (the ephemeral TempDir) is bound here so it OUTLIVES
    // `cmd.output()`: dropping it earlier would delete the isolated state root
    // mid-run. Its cleanup happens on drop after the subprocess exits below.
    match cmd.output() {
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

    // ─────────────────────────────────────────────────────────────────────
    // TDD (#4628 — canary unit-test gate state-root isolation): FAILING tests,
    // written first. They MUST fail to compile / fail against the current code
    // (which builds+runs the `cargo test` Command inline in `run_unit_test_gate`
    // with NO state isolation) and pass once the fix extracts two seams:
    //
    //   * `build_unit_test_command(config) -> SimardResult<(Command, TempDir)>`
    //     — the scrubbed `cargo test` Command with `SIMARD_STATE_ROOT` +
    //       `SIMARD_HOME` overridden (AFTER the scrub, last-write-wins) to a
    //       fresh absolute ephemeral `TempDir`, and `SIMARD_MEMORY_SOCKET`
    //       removed, so the canary never contends with the live daemon's
    //       exclusive cognitive/typed-OODA locks. The returned `TempDir` guard
    //       MUST outlive `cmd.output()`.
    //   * `unit_test_gate_failed_closed(reason) -> GateResult` — the fail-closed
    //       RED result returned when the isolated state root cannot be created
    //       (REQ-SEC-5: never fall back to the live root).
    //
    // Constraints honoured: additive, fail-closed preserved, intent-revealing
    // names only, `tracing`/OTel only (no `print!`/`println!`). Only the
    // unit-test gate is isolated — smoke/gym/rpc-health are untouched.

    // Test (a) — env-target assertion (design decision #1/#2, REQ-SEC-2/6).
    // The gate must point `cargo test` at an ISOLATED, absolute, per-run temp
    // state root that OVERRIDES any inherited/allow-listed live value (proving
    // the overrides are applied AFTER `scrub_gate_env` — last-write-wins), and
    // must NOT hand the canary the live daemon's memory socket.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn unit_test_gate_overrides_state_root_to_isolated_temp() {
        use std::collections::HashMap;
        use std::ffi::{OsStr, OsString};
        use std::path::PathBuf;

        // A "live daemon" shape in the ambient env — the exact values that, if
        // inherited, would collide with the running daemon's exclusive locks.
        let live = std::env::temp_dir().join(format!("simard-live-daemon-{}", std::process::id()));

        // SAFETY: serialized by the cognitive_memory serial key (whole-binary);
        // no concurrent test reads these vars.
        unsafe {
            std::env::set_var("SIMARD_STATE_ROOT", &live);
            std::env::set_var("SIMARD_HOME", &live);
            std::env::set_var("SIMARD_MEMORY_SOCKET", "/run/simard-live-daemon.sock");
        }

        // Allow-list the deploy-shape names so `scrub_gate_env` RE-INJECTS the
        // live values; the gate's isolation override must still win — this is
        // the ordering guarantee (overrides applied after the scrub).
        let config = RelaunchConfig {
            canary_env: vec![
                "SIMARD_STATE_ROOT".to_string(),
                "SIMARD_HOME".to_string(),
                "SIMARD_MEMORY_SOCKET".to_string(),
            ],
            ..RelaunchConfig::default()
        };

        let (cmd, tmp) = build_unit_test_command(&config)
            .expect("gate must build an isolated cargo-test command");

        let envs: HashMap<OsString, Option<OsString>> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(OsStr::to_os_string)))
            .collect();

        // SAFETY: same serial key; restore ambient env before asserting.
        unsafe {
            std::env::remove_var("SIMARD_STATE_ROOT");
            std::env::remove_var("SIMARD_HOME");
            std::env::remove_var("SIMARD_MEMORY_SOCKET");
        }

        let state_root = PathBuf::from(
            envs.get(OsStr::new("SIMARD_STATE_ROOT"))
                .and_then(|v| v.clone())
                .expect("gate must set SIMARD_STATE_ROOT on the cargo-test command"),
        );
        let home = PathBuf::from(
            envs.get(OsStr::new("SIMARD_HOME"))
                .and_then(|v| v.clone())
                .expect("gate must set SIMARD_HOME on the cargo-test command"),
        );

        // REQ-SEC-2: absolute path (no CWD-relative escape/collision).
        assert!(
            state_root.is_absolute(),
            "isolated state root must be absolute, got: {}",
            state_root.display()
        );
        // tempfile::TempDir lives under the system temp dir.
        assert!(
            state_root.starts_with(std::env::temp_dir()),
            "isolated state root must live under temp_dir(), got: {}",
            state_root.display()
        );
        // It is exactly the ephemeral TempDir the gate created and returned.
        assert_eq!(
            state_root,
            tmp.path(),
            "SIMARD_STATE_ROOT must be the gate's ephemeral TempDir"
        );
        // It MUST override (not equal) the inherited/allow-listed live root — the
        // whole point of the #4628 fix (last-write-wins after the scrub).
        assert_ne!(
            state_root, live,
            "gate must NOT share the live daemon's state root (#4628 contention)"
        );
        assert_eq!(
            home,
            tmp.path(),
            "SIMARD_HOME must point at the same isolated root"
        );
        assert_ne!(home, live, "gate must override the live SIMARD_HOME too");

        // The canary must never be handed a memory socket pointing at the live
        // daemon: after the scrub's `env_clear()` the socket is absent, and the
        // gate additionally `env_remove`s it — either way it is never a live
        // value on the command.
        match envs.get(OsStr::new("SIMARD_MEMORY_SOCKET")) {
            None | Some(None) => {}
            Some(Some(v)) => {
                panic!("gate must not point the canary at a live memory socket, got: {v:?}")
            }
        }

        // Each gate run gets its OWN isolated root — no cross-run collision.
        let (_cmd2, tmp2) = build_unit_test_command(&RelaunchConfig::default())
            .expect("second gate build must also isolate");
        assert_ne!(
            tmp.path(),
            tmp2.path(),
            "each unit-test gate run must get a unique ephemeral state root"
        );
    }

    // Test (fail-closed) — REQ-SEC-5. When the isolated state root cannot be
    // created the gate must return a RED unit-test result, NEVER fall back to
    // the live daemon's state root (which would reintroduce #4628).
    #[test]
    fn unit_test_gate_fails_closed_when_state_isolation_unavailable() {
        let result = unit_test_gate_failed_closed("mkdir /proc/nonexistent: permission denied");
        assert_eq!(result.gate, RelaunchGate::UnitTest);
        assert!(
            !result.passed,
            "must fail closed on isolation failure, never fall back to the live state root"
        );
        assert!(
            result.detail.contains("isolated state root"),
            "detail must explain the state-isolation failure, got: {}",
            result.detail
        );
        assert!(
            result.detail.contains("permission denied"),
            "detail must carry the underlying cause, got: {}",
            result.detail
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

    // Test (b) — reproduce-then-confirm (#4628). Faithfully reproduces the
    // exit-101 crash-loop mechanism at the OS level: the live daemon holds an
    // exclusive advisory `flock` on a store lock file under its state root
    // (exactly what the cognitive store's `acquire_open_lock` and the typed-OODA
    // sqlite outcome store take — a single-writer lock). A SECOND opener at the
    // SAME state root cannot lock (the contention that reddened every canary
    // tick). The fix points the unit-test gate at an ISOLATED state root, so a
    // store lock under THAT root is acquired cleanly while the daemon still
    // holds its own — proving the isolated run goes green instead of exit-101.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn unit_test_gate_isolated_root_avoids_live_daemon_lock_contention() {
        use std::os::unix::io::AsRawFd;
        use std::path::PathBuf;

        // Live daemon acquires its store lock under its state root.
        let live = unique_tmp("live-daemon");
        let live_lock = live.join("store.open.lock");
        let held = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&live_lock)
            .expect("open live daemon store lock file");
        assert_eq!(
            unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0,
            "the live daemon must acquire its exclusive store lock"
        );

        // Reproduce #4628: a second opener sharing the live root CANNOT lock —
        // this is the `database is locked` / `held open by another process`
        // contention that failed the canary ~1.7s into `cargo test`.
        let contender = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&live_lock)
            .expect("open contender handle on the shared live lock file");
        assert_ne!(
            unsafe { libc::flock(contender.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0,
            "a second open on the SHARED live state root MUST contend (the crash-loop)"
        );

        // Confirm the fix: the gate isolates the canary's state root, so the
        // same store lock under the isolated root is acquired without contending
        // with the live daemon — the gate would go green, not exit-101.
        let (cmd, iso) = build_unit_test_command(&RelaunchConfig::default())
            .expect("gate must build an isolated cargo-test command");
        let iso_root: PathBuf = cmd
            .get_envs()
            .find(|(k, _)| k.to_str() == Some("SIMARD_STATE_ROOT"))
            .and_then(|(_, v)| v)
            .map(PathBuf::from)
            .expect("gate must set an isolated SIMARD_STATE_ROOT");
        assert_ne!(
            iso_root, live,
            "the isolated canary root must differ from the live daemon root"
        );
        assert_eq!(
            iso_root,
            iso.path(),
            "the isolated root must be the gate's ephemeral TempDir"
        );

        let iso_lock = iso_root.join("store.open.lock");
        let iso_file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(&iso_lock)
            .expect("open store lock file under the isolated root");
        assert_eq!(
            unsafe { libc::flock(iso_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0,
            "isolated-root store open MUST succeed while the live daemon holds its own lock"
        );

        // Release locks and clean up the simulated live root (the isolated root
        // is cleaned when `iso` drops).
        unsafe {
            libc::flock(iso_file.as_raw_fd(), libc::LOCK_UN);
            libc::flock(held.as_raw_fd(), libc::LOCK_UN);
        }
        let _ = fs::remove_dir_all(&live);
    }
}
