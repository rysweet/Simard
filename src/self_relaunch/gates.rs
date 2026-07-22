use std::path::Path;
use std::process::Command;

use super::types::{GateResult, RelaunchConfig, RelaunchGate};
use crate::error::SimardResult;

/// The closed set of environment-variable NAMES a candidate-binary canary gate
/// is permitted to receive from the live daemon environment. Code-defined and
/// constant: not extensible via config, CLI, or env. Deny-by-default (SEC-I1).
///
/// These are the deploy signals a freshly built candidate binary legitimately
/// needs to resolve its home / state / prompt-assets exactly like the running
/// daemon during gating. Everything else — including all other `SIMARD_*`
/// variables — is dropped. The `unit-test` (`cargo test`) gate receives **none**
/// of these (see [`GateEnvProfile`]).
pub fn canary_gate_env_allowlist() -> Vec<String> {
    vec![
        "SIMARD_HOME".to_string(),
        "SIMARD_PROMPT_ASSETS_DIR".to_string(),
        "SIMARD_STATE_ROOT".to_string(),
    ]
}

/// Fixed, minimal set of env-var NAMES both the Rust toolchain and the candidate
/// binary need to execute at all under `env_clear()`. Carries **no** deploy state
/// and **no** `HOME` (`HOME` is set by the profile layer). `CARGO_HOME` /
/// `RUSTUP_HOME` are pinned here so the `unit-test` gate's neutral `HOME` never
/// forces cargo/rustup to fall back to `$HOME`.
const GATE_ENV_BASE_FLOOR: &[&str] = &["PATH", "USER", "CARGO_HOME", "RUSTUP_HOME"];

/// Which environment profile a canary gate subprocess must run under.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GateEnvProfile {
    /// Runs the freshly built candidate `simard` binary (`smoke`, `gym-baseline`,
    /// `rpc-health`). It must resolve its home / state / prompt-assets exactly
    /// like the running daemon, so it receives the deploy-signal allow-list AND
    /// the live `HOME`.
    CandidateBinary,
    /// Runs `cargo test`. It must NOT see the daemon's live `SIMARD_*` or live
    /// `HOME`, or env-sensitive tests would resolve to live state (via the
    /// `SIMARD_HOME` -> `$HOME/.simard` fallback) and panic (exit 101). Gets a
    /// neutral scratch `HOME` under `canary_target_dir` and no deploy signals.
    UnitTest,
}

/// Configure `cmd` with a hermetic environment for a canary gate subprocess.
///
/// Ordering is mandatory: `env_clear()` FIRST, then the minimal base floor, then
/// the profile-specific layer. Any ambient variable NOT explicitly re-injected is
/// dropped. This prevents the running daemon's live `SIMARD_*` / `HOME` state from
/// leaking into `cargo test` and panicking env-sensitive tests (exit 101) on the
/// deploy host only. See `docs/reference/canary-gate-convergence.md`.
fn scrub_gate_env(cmd: &mut Command, config: &RelaunchConfig, profile: GateEnvProfile) {
    // 1. Deny by default.
    cmd.env_clear();

    // 2. Minimal base floor required for the toolchain/binary to run at all.
    //    Carries NO deploy state and NO `HOME` (`HOME` is profile-specific).
    for key in GATE_ENV_BASE_FLOOR {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }

    // 3. Profile-specific layer (runs last so it always wins any collision).
    match profile {
        // Candidate binary: resolve home/state like the running daemon.
        GateEnvProfile::CandidateBinary => {
            if let Ok(home) = std::env::var("HOME") {
                cmd.env("HOME", home); // live HOME, on purpose
            }
            // Deploy signals re-injected by NAME (values read live, never stored).
            for name in &config.canary_env {
                if let Ok(val) = std::env::var(name) {
                    cmd.env(name, val);
                }
            }
        }
        // Unit tests: neutral scratch HOME, NO SIMARD_* — tests use their own
        // hermetic fixtures.
        GateEnvProfile::UnitTest => {
            let neutral_home = config.canary_target_dir.join("gate-home");
            let _ = std::fs::create_dir_all(&neutral_home);
            cmd.env("HOME", neutral_home);
            // Intentionally NO SIMARD_HOME / SIMARD_STATE_ROOT /
            // SIMARD_PROMPT_ASSETS_DIR here.
        }
    }
}

/// Redact URL-embedded credentials THEN char-boundary-safe truncate so the
/// final string (including any ellipsis) is at most 512 bytes.
///
/// Order is mandatory: redact BEFORE bound, so truncation can never split a
/// `user:pass@host` credential in a way that leaks the tail or defeats the
/// redactor (SEC-D2). The 512-byte cap counts the trailing ellipsis and snaps
/// **down** to a UTF-8 char boundary, so multi-byte stderr can never panic the
/// overseer tick or exceed the bound.
fn bound_gate_detail(raw: &str) -> String {
    const MAX: usize = 512;
    const ELLIPSIS: &str = "...";

    let redacted = crate::self_deploy::source_prep::redact_credentials(raw);
    if redacted.len() <= MAX {
        return redacted;
    }

    // Reserve room for the ellipsis, then snap DOWN to a char boundary so the
    // total length (content + ellipsis) can never exceed MAX.
    let mut end = MAX - ELLIPSIS.len();
    while end > 0 && !redacted.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ELLIPSIS}", redacted[..end].trim_end())
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
        RelaunchGate::Smoke => run_smoke_gate(binary, config),
        RelaunchGate::UnitTest => run_unit_test_gate(config),
        RelaunchGate::GymBaseline => run_gym_baseline_gate(binary, config),
        RelaunchGate::RpcHealth => run_rpc_health_gate(binary, config),
    }
}

fn run_smoke_gate(binary: &Path, config: &RelaunchConfig) -> GateResult {
    let mut cmd = Command::new(binary);
    scrub_gate_env(&mut cmd, config, GateEnvProfile::CandidateBinary);
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
            detail: bound_gate_detail(&format!(
                "binary exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        },
        Err(e) => GateResult {
            gate: RelaunchGate::Smoke,
            passed: false,
            detail: bound_gate_detail(&format!("failed to execute binary: {e}")),
        },
    }
}

fn run_unit_test_gate(config: &RelaunchConfig) -> GateResult {
    let mut cmd = Command::new("cargo");
    scrub_gate_env(&mut cmd, config, GateEnvProfile::UnitTest);
    match cmd
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
            GateResult {
                gate: RelaunchGate::UnitTest,
                passed: false,
                detail: bound_gate_detail(&format!(
                    "tests failed (exit {}): {}",
                    output.status, stderr
                )),
            }
        }
        Err(e) => GateResult {
            gate: RelaunchGate::UnitTest,
            passed: false,
            detail: bound_gate_detail(&format!("cargo test failed to run: {e}")),
        },
    }
}

fn run_gym_baseline_gate(binary: &Path, config: &RelaunchConfig) -> GateResult {
    let mut cmd = Command::new(binary);
    scrub_gate_env(&mut cmd, config, GateEnvProfile::CandidateBinary);
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
            detail: bound_gate_detail(&format!(
                "gym probe failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        },
        Err(e) => GateResult {
            gate: RelaunchGate::GymBaseline,
            passed: false,
            detail: bound_gate_detail(&format!("gym probe failed to run: {e}")),
        },
    }
}

fn run_rpc_health_gate(binary: &Path, config: &RelaunchConfig) -> GateResult {
    let timeout_secs = config.health_timeout.as_secs().to_string();
    let mut cmd = Command::new(binary);
    scrub_gate_env(&mut cmd, config, GateEnvProfile::CandidateBinary);
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
            detail: bound_gate_detail(&format!(
                "rpc health failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        },
        Err(e) => GateResult {
            gate: RelaunchGate::RpcHealth,
            passed: false,
            detail: bound_gate_detail(&format!("rpc health probe failed to run: {e}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_gate_handles_missing_binary() {
        let config = RelaunchConfig::default();
        let result = run_smoke_gate(Path::new("/tmp/no-such-binary-48291"), &config);
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

    // =====================================================================
    // Step 7 (TDD): canary gate env-isolation contract.
    //
    // These tests specify the split-profile env-isolation fix for the
    // persistently-RED self-deploy canary (root cause: the `unit-test` gate
    // inherited the live daemon's `SIMARD_*` / `HOME`, panicking env-sensitive
    // lib tests with exit 101 on the deploy host only). See
    // docs/reference/canary-gate-convergence.md for the full contract.
    //
    // They reference symbols that do NOT yet exist (`GateEnvProfile`,
    // `scrub_gate_env`, `bound_gate_detail`, `canary_gate_env_allowlist`,
    // `RelaunchConfig.canary_env`), so this module fails to compile until the
    // implementation lands — the intended TDD "red" state. `cargo build`
    // (which excludes `#[cfg(test)]`) stays green.
    // =====================================================================

    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    /// Serialize every test that mutates process-global env. Env mutation is
    /// not thread-safe and cargo runs tests in parallel. Paired with
    /// `#[serial_test::serial(cognitive_memory)]` so it also serializes against
    /// the rest of the crate's env-mutating tests.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Snapshot + restore a set of ambient env vars around a test body so env
    /// mutation never leaks between serialized tests.
    struct EnvSnapshot {
        saved: Vec<(String, Option<String>)>,
    }

    impl EnvSnapshot {
        fn capture(keys: &[&str]) -> Self {
            let saved = keys
                .iter()
                .map(|k| ((*k).to_string(), std::env::var(k).ok()))
                .collect();
            Self { saved }
        }
    }

    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                // SAFETY: callers hold `env_lock()` for the whole test body,
                // so no other thread races these mutations.
                unsafe {
                    match v {
                        Some(val) => std::env::set_var(k, val),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }

    /// Absolute path to the `env` coreutil, used to observe the *actual* child
    /// environment produced by `scrub_gate_env` without spawning `cargo test`.
    fn env_binary() -> Option<PathBuf> {
        for candidate in ["/usr/bin/env", "/bin/env"] {
            if Path::new(candidate).exists() {
                return Some(PathBuf::from(candidate));
            }
        }
        None
    }

    fn parse_child_env(stdout: &str) -> HashMap<String, String> {
        stdout
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Apply `scrub_gate_env` to an `env` subprocess and return the child's
    /// resulting environment. Caller must hold `env_lock()`.
    fn scrubbed_child_env(
        config: &RelaunchConfig,
        profile: GateEnvProfile,
    ) -> HashMap<String, String> {
        let env_bin = env_binary().expect("`env` coreutil must exist on the test host");
        let mut cmd = Command::new(env_bin);
        scrub_gate_env(&mut cmd, config, profile);
        let output = cmd.output().expect("spawning `env` must succeed");
        assert!(
            output.status.success(),
            "`env` exited non-zero: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        parse_child_env(&String::from_utf8_lossy(&output.stdout))
    }

    fn unique_config() -> RelaunchConfig {
        RelaunchConfig {
            canary_target_dir: std::env::temp_dir().join(format!(
                "simard-gate-env-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            )),
            ..Default::default()
        }
    }

    // --- canary_gate_env_allowlist(): closed, code-defined deploy-signal set ---

    #[test]
    fn allowlist_is_exact_closed_set() {
        let mut got = crate::self_relaunch::canary_gate_env_allowlist();
        got.sort();
        let mut want = vec![
            "SIMARD_HOME".to_string(),
            "SIMARD_PROMPT_ASSETS_DIR".to_string(),
            "SIMARD_STATE_ROOT".to_string(),
        ];
        want.sort();
        assert_eq!(
            got, want,
            "allow-list must be exactly the three deploy signals"
        );
    }

    #[test]
    fn allowlist_is_names_only_not_values() {
        for entry in crate::self_relaunch::canary_gate_env_allowlist() {
            assert!(
                !entry.contains('='),
                "allow-list entry must be a NAME, not a NAME=VALUE pair: {entry:?}"
            );
            assert!(
                entry.starts_with("SIMARD_"),
                "allow-list entries are deploy signals: {entry:?}"
            );
            assert_eq!(
                entry,
                entry.to_uppercase(),
                "env var names are upper-case: {entry:?}"
            );
        }
    }

    #[test]
    fn allowlist_denies_other_vars() {
        let list = crate::self_relaunch::canary_gate_env_allowlist();
        for denied in [
            "HOME",
            "PATH",
            "SIMARD_GIT_HASH",
            "SIMARD_TOKEN",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
        ] {
            assert!(
                !list.contains(&denied.to_string()),
                "{denied} must NOT be allow-listed (deny-by-default)"
            );
        }
    }

    // --- RelaunchConfig.canary_env: names-only carrier ---

    #[test]
    fn config_default_populates_canary_env_from_allowlist() {
        let config = RelaunchConfig::default();
        assert_eq!(
            config.canary_env,
            crate::self_relaunch::canary_gate_env_allowlist(),
            "default config must carry the allow-list names"
        );
    }

    #[test]
    fn config_canary_env_is_names_only() {
        for name in RelaunchConfig::default().canary_env {
            assert!(
                !name.contains('='),
                "canary_env stores NAMES only, never NAME=VALUE: {name:?}"
            );
        }
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn config_debug_does_not_leak_env_values() {
        let _g = env_lock().lock().unwrap();
        let _snap = EnvSnapshot::capture(&["SIMARD_HOME"]);
        // SAFETY: serialized via env_lock + restored by EnvSnapshot::drop.
        unsafe {
            std::env::set_var("SIMARD_HOME", "super-secret-live-home-value");
        }
        let config = RelaunchConfig {
            canary_env: vec!["SIMARD_HOME".to_string()],
            ..Default::default()
        };
        let debug = format!("{config:?}");
        assert!(
            debug.contains("SIMARD_HOME"),
            "Debug should surface the NAME: {debug}"
        );
        assert!(
            !debug.contains("super-secret-live-home-value"),
            "Debug must NEVER surface the resolved VALUE (SEC-D1): {debug}"
        );
    }

    // --- scrub_gate_env: UnitTest profile (the root-cause fix) ---

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn scrub_unit_test_profile_drops_all_simard_vars() {
        let _g = env_lock().lock().unwrap();
        if env_binary().is_none() {
            return;
        }
        let _snap = EnvSnapshot::capture(&[
            "SIMARD_HOME",
            "SIMARD_STATE_ROOT",
            "SIMARD_PROMPT_ASSETS_DIR",
            "SIMARD_LEAK",
        ]);
        // SAFETY: serialized via env_lock + restored by EnvSnapshot::drop.
        unsafe {
            std::env::set_var("SIMARD_HOME", "/live/daemon/home");
            std::env::set_var("SIMARD_STATE_ROOT", "/live/daemon/state");
            std::env::set_var("SIMARD_PROMPT_ASSETS_DIR", "/live/daemon/prompts");
            std::env::set_var("SIMARD_LEAK", "should-not-reach-cargo-test");
        }
        let config = unique_config();
        let child = scrubbed_child_env(&config, GateEnvProfile::UnitTest);

        // The unit-test gate must see NO SIMARD_* — this is the exit-101 fix.
        for key in child.keys() {
            assert!(
                !key.starts_with("SIMARD_"),
                "unit-test gate leaked a SIMARD_* var into `cargo test`: {key}"
            );
        }
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn scrub_unit_test_profile_uses_neutral_scratch_home() {
        let _g = env_lock().lock().unwrap();
        if env_binary().is_none() {
            return;
        }
        let _snap = EnvSnapshot::capture(&["HOME"]);
        // SAFETY: serialized via env_lock + restored by EnvSnapshot::drop.
        unsafe {
            std::env::set_var("HOME", "/live/daemon/home");
        }
        let config = unique_config();
        let child = scrubbed_child_env(&config, GateEnvProfile::UnitTest);

        let expected_home = config.canary_target_dir.join("gate-home");
        let child_home = child.get("HOME").expect("unit-test gate must set HOME");
        assert_eq!(
            PathBuf::from(child_home),
            expected_home,
            "unit-test gate HOME must be a neutral scratch dir, not the live HOME"
        );
        assert_ne!(
            child_home, "/live/daemon/home",
            "unit-test gate must NOT inherit the live daemon HOME"
        );
        assert!(
            expected_home.exists(),
            "scrub_gate_env must create the neutral scratch HOME dir"
        );
        let _ = std::fs::remove_dir_all(&config.canary_target_dir);
    }

    // --- scrub_gate_env: CandidateBinary profile ---

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn scrub_candidate_binary_profile_injects_allowlist_and_live_home() {
        let _g = env_lock().lock().unwrap();
        if env_binary().is_none() {
            return;
        }
        let _snap = EnvSnapshot::capture(&[
            "HOME",
            "SIMARD_HOME",
            "SIMARD_STATE_ROOT",
            "SIMARD_PROMPT_ASSETS_DIR",
            "SIMARD_LEAK",
        ]);
        // SAFETY: serialized via env_lock + restored by EnvSnapshot::drop.
        unsafe {
            std::env::set_var("HOME", "/live/daemon/home");
            std::env::set_var("SIMARD_HOME", "/live/simard/home");
            std::env::set_var("SIMARD_STATE_ROOT", "/live/simard/state");
            std::env::set_var("SIMARD_PROMPT_ASSETS_DIR", "/live/simard/prompts");
            std::env::set_var("SIMARD_LEAK", "not-a-deploy-signal");
        }
        let config = RelaunchConfig {
            canary_env: crate::self_relaunch::canary_gate_env_allowlist(),
            ..unique_config()
        };
        let child = scrubbed_child_env(&config, GateEnvProfile::CandidateBinary);

        // Candidate binary must resolve home/state like the daemon.
        assert_eq!(
            child.get("HOME").map(String::as_str),
            Some("/live/daemon/home"),
            "candidate-binary gate must carry the live HOME"
        );
        assert_eq!(
            child.get("SIMARD_HOME").map(String::as_str),
            Some("/live/simard/home"),
            "allow-listed SIMARD_HOME must be re-injected with its live value"
        );
        assert_eq!(
            child.get("SIMARD_STATE_ROOT").map(String::as_str),
            Some("/live/simard/state")
        );
        assert_eq!(
            child.get("SIMARD_PROMPT_ASSETS_DIR").map(String::as_str),
            Some("/live/simard/prompts")
        );
        // Non-allow-listed SIMARD_* is still dropped (deny-by-default).
        assert!(
            !child.contains_key("SIMARD_LEAK"),
            "non-allow-listed SIMARD_LEAK must be dropped even for candidate gates"
        );
    }

    // --- scrub_gate_env: base floor preserved under both profiles ---

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn scrub_preserves_base_floor_both_profiles() {
        let _g = env_lock().lock().unwrap();
        if env_binary().is_none() {
            return;
        }
        let _snap = EnvSnapshot::capture(&["PATH", "CARGO_HOME"]);
        // SAFETY: serialized via env_lock + restored by EnvSnapshot::drop.
        unsafe {
            // Keep a real PATH so `/usr/bin/env` still resolves its own deps,
            // but pin a sentinel component we can assert survived.
            let path = std::env::var("PATH").unwrap_or_default();
            std::env::set_var("PATH", format!("/sentinel/bin:{path}"));
            std::env::set_var("CARGO_HOME", "/sentinel/cargo-home");
        }
        let config = unique_config();

        for profile in [GateEnvProfile::CandidateBinary, GateEnvProfile::UnitTest] {
            let child = scrubbed_child_env(&config, profile);
            assert!(
                child
                    .get("PATH")
                    .is_some_and(|p| p.contains("/sentinel/bin")),
                "base floor must carry PATH through (needed to locate the toolchain)"
            );
            assert_eq!(
                child.get("CARGO_HOME").map(String::as_str),
                Some("/sentinel/cargo-home"),
                "base floor must carry CARGO_HOME so a neutral HOME never breaks cargo"
            );
        }
        let _ = std::fs::remove_dir_all(&config.canary_target_dir);
    }

    // --- bound_gate_detail: redact-then-bound credential guard ---

    #[test]
    fn bound_gate_detail_redacts_url_credentials() {
        let raw = "clone failed: https://alice:supersecret@github.com/rysweet/Simard.git";
        let out = bound_gate_detail(raw);
        assert!(
            !out.contains("supersecret") && !out.contains("alice:"),
            "URL-embedded credentials must be redacted: {out}"
        );
        assert!(
            out.contains("github.com/rysweet/Simard.git"),
            "redaction must preserve the non-secret remainder: {out}"
        );
    }

    #[test]
    fn bound_gate_detail_bounds_to_512_bytes() {
        let raw = "x".repeat(5_000);
        let out = bound_gate_detail(&raw);
        assert!(
            out.len() <= 512,
            "gate detail must be bounded to <=512 bytes, got {}",
            out.len()
        );
    }

    #[test]
    fn bound_gate_detail_utf8_safe_at_boundary() {
        // Multi-byte chars straddling the 512-byte budget must not panic and
        // must yield valid UTF-8 (guaranteed by returning String).
        let raw = "é".repeat(1_000); // 2 bytes each => 2000 bytes
        let out = bound_gate_detail(&raw);
        assert!(out.len() <= 512, "must be bounded, got {}", out.len());
        // Round-trips as UTF-8 by construction; assert no partial code point.
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn bound_gate_detail_redacts_before_bounding() {
        // Place a credential so that, if truncation happened BEFORE redaction,
        // the 512-byte cut would land mid-token and leak "alice:supersec…".
        // Redact-before-bound must remove the whole userinfo first.
        let prefix = "a".repeat(490);
        let raw = format!("{prefix} https://alice:supersecret@github.com/x");
        assert!(
            raw.len() > 512,
            "precondition: input exceeds the 512B bound"
        );
        let out = bound_gate_detail(&raw);
        assert!(out.len() <= 512, "must still be bounded, got {}", out.len());
        assert!(
            !out.contains("alice") && !out.contains("supersec"),
            "redact-before-bound must strip the credential even at the truncation \
             boundary (SEC-D2): {out}"
        );
    }

    #[test]
    fn bound_gate_detail_short_tokenless_passes_through() {
        let raw = "tests failed (exit 101): assertion failed";
        assert_eq!(
            bound_gate_detail(raw),
            raw,
            "short, credential-free detail must be preserved verbatim"
        );
    }

    // --- fail-closed: env isolation must never mask a real RED (SEC-A1) ---

    #[test]
    fn fail_closed_missing_binary_still_red_with_allowlist_wired() {
        let config = RelaunchConfig::default();
        assert!(
            !config.canary_env.is_empty(),
            "precondition: default config wires the deploy-signal allow-list"
        );
        let results = verify_canary(
            Path::new("/no-such-candidate-binary-77321"),
            &[
                RelaunchGate::Smoke,
                RelaunchGate::GymBaseline,
                RelaunchGate::RpcHealth,
            ],
            &config,
        )
        .unwrap();
        assert!(
            results.iter().all(|r| !r.passed),
            "env isolation must never flip a genuinely broken candidate to GREEN"
        );
    }
}
