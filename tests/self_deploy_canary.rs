//! Hermetic self-deploy canary regression suite (rysweet/Simard#4622).
//!
//! # Why this file exists
//!
//! The self-deploy `UnitTest` canary gate historically shelled out to an
//! *unfiltered* `cargo test` over the whole `simard` lib suite inside the
//! isolated self-deploy-target env. That suite is a moving target: non-hermetic
//! tests (shared `HOME`/temp dirs, serial-resource contention, `Drop`-order
//! cleanup guards) panic under the canary's isolated env even while they are
//! green in CI. Each merge de-flaked one test and a new one surfaced —
//! whack-a-mole — which wedged self-deploy in a red-canary crash-loop
//! (`running_commit` frozen at `e3a4327834db`; deterministic `exit status 101`).
//!
//! This target is the **curated, hermetic-by-construction** replacement scope
//! for that gate. It is compiled ONLY under the opt-in `canary-tests` feature
//! (a TEST-SELECTION flag, mirroring `slow-tests`; see `Cargo.toml`), so the
//! self-deploy canary can invoke exactly this bounded set via
//! `cargo test --test self_deploy_canary --features canary-tests` instead of the
//! whole shifting lib suite. A newly-added non-hermetic lib test therefore can
//! never re-wedge the gate.
//!
//! # Contract these tests pin (TDD — written before the scoping change lands)
//!
//! Every test here consumes ONLY the crate's public API
//! (`verify_canary`, `default_gates`, `all_gates_passed`, `RelaunchConfig`,
//! `RelaunchGate`, `GateResult`, `canary_gate_env_allowlist`) and is hermetic:
//! it owns a `tempfile::TempDir` and, when it mutates process-global env, points
//! `HOME`/`SIMARD_STATE_ROOT`/`SIMARD_HOME` at that owned dir, runs
//! `#[serial]`, and restores prior env deterministically on `Drop`.
//!
//! Load-bearing invariants:
//!   1. FAIL-CLOSED end-to-end: `verify_canary` against a binary whose
//!      `--version` exits non-zero yields a RED `GateResult` — never mapped to
//!      pass. This is the deploy-authorization guarantee: a genuine regression
//!      still refuses relaunch.
//!   2. MEANINGFUL (positive control): the same machinery greens a healthy
//!      stub binary, so the gate is not trivially always-red / cosmetic.
//!   3. NO SHORT-CIRCUIT: `verify_canary` evaluates and reports every requested
//!      gate; a red gate does not silently drop later gates or the aggregate.
//!   4. GATE ORDER: `default_gates()` stays Smoke → UnitTest → GymBaseline →
//!      RpcHealth (the `UnitTest` gate is scoped, not removed).
//!   5. AGGREGATION: `all_gates_passed` is true iff every gate passed.
//!   6. DENY-BY-DEFAULT ENV FLOOR (SEC): `canary_gate_env_allowlist()` is a
//!      minimal Simard deploy-shape allow-list with no hijack-class names.

#![cfg(feature = "canary-tests")]

use std::fs;
use std::path::{Path, PathBuf};

use serial_test::serial;
use tempfile::TempDir;

use simard::self_relaunch::canary_gate_env_allowlist;
use simard::{
    GateResult, RelaunchConfig, RelaunchGate, all_gates_passed, default_gates, verify_canary,
};

/// RAII guard that makes a test hermetic w.r.t. the process-global deploy-shape
/// env: it points `HOME`/`SIMARD_STATE_ROOT`/`SIMARD_HOME` at an owned
/// `TempDir` and removes `SIMARD_MEMORY_SOCKET` (so nothing can dial a live
/// daemon), then restores the prior values on `Drop` — deterministically, even
/// on an uncaught panic mid-test. Tests using this MUST be `#[serial]` so no two
/// mutate these globals concurrently (SAFETY basis for the `unsafe` env calls).
struct HermeticEnv {
    _dir: TempDir,
    restore: Vec<(&'static str, Option<String>)>,
}

impl HermeticEnv {
    const VARS: [&'static str; 4] = [
        "HOME",
        "SIMARD_STATE_ROOT",
        "SIMARD_HOME",
        "SIMARD_MEMORY_SOCKET",
    ];

    fn new() -> Self {
        let dir = TempDir::new().expect("mint isolated canary tempdir");
        let root = dir.path();
        let restore = Self::VARS
            .iter()
            .map(|&k| (k, std::env::var(k).ok()))
            .collect();

        // SAFETY: every test constructing a `HermeticEnv` is `#[serial]`, so no
        // two of them mutate these process-global vars concurrently.
        unsafe {
            std::env::set_var("HOME", root);
            std::env::set_var("SIMARD_STATE_ROOT", root);
            std::env::set_var("SIMARD_HOME", root);
            std::env::remove_var("SIMARD_MEMORY_SOCKET");
        }

        Self { _dir: dir, restore }
    }

    fn path(&self) -> &Path {
        self._dir.path()
    }
}

impl Drop for HermeticEnv {
    fn drop(&mut self) {
        // SAFETY: see `HermeticEnv::new` — the owning test is `#[serial]`.
        unsafe {
            for (k, prior) in self.restore.drain(..) {
                match prior {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
}

/// Write an executable stub binary at `dir/name` that ignores its argv and
/// exits with `exit_code`, echoing `stdout_line` on stdout. `verify_canary`'s
/// Smoke gate runs `<binary> --version`; a healthy stub exits 0, a regressed
/// stub exits non-zero (we use 101, the real deterministic panic code observed
/// in the crash-loop). No `cargo`, no network — hermetic by construction.
fn write_stub_binary(dir: &Path, name: &str, exit_code: i32, stdout_line: &str) -> PathBuf {
    let path = dir.join(name);
    let script = format!("#!/bin/sh\necho \"{stdout_line}\"\nexit {exit_code}\n");
    fs::write(&path, script).expect("write stub binary");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path).expect("stat stub").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod +x stub");
    }

    path
}

/// A `RelaunchConfig` whose target-dir and manifest-dir live under the owned
/// hermetic root, so even the fields the Smoke gate does not consume point at
/// throwaway paths (never the live worktree / state root).
fn hermetic_config(root: &Path) -> RelaunchConfig {
    RelaunchConfig {
        canary_target_dir: root.join("canary-target"),
        manifest_dir: root.join("manifest"),
        canary_env: Vec::new(),
        ..RelaunchConfig::default()
    }
}

// ---------------------------------------------------------------------------
// Invariant 1 (LOAD-BEARING): fail-closed end-to-end.
// ---------------------------------------------------------------------------

#[test]
#[serial(canary_deploy_shape_env)]
fn verify_canary_reddens_when_binary_regresses() {
    let env = HermeticEnv::new();
    let config = hermetic_config(env.path());

    // A regressed candidate: `--version` exits 101 (the observed panic code).
    let bad = write_stub_binary(env.path(), "regressed-simard", 101, "boom");

    let results = verify_canary(&bad, &[RelaunchGate::Smoke], &config)
        .expect("verify_canary drives the gate and returns a verdict");

    assert_eq!(results.len(), 1, "one gate requested, one result expected");
    let smoke = &results[0];
    assert_eq!(smoke.gate, RelaunchGate::Smoke);
    assert!(
        !smoke.passed,
        "a non-zero `--version` MUST redden the gate (fail-closed); got: {smoke}"
    );
    assert!(
        !all_gates_passed(&results),
        "a red gate MUST make the aggregate fail-closed — relaunch refused"
    );
}

// ---------------------------------------------------------------------------
// Invariant 2 (MEANINGFUL / positive control): the gate is not always-red.
// ---------------------------------------------------------------------------

#[test]
#[serial(canary_deploy_shape_env)]
fn verify_canary_greens_a_healthy_binary() {
    let env = HermeticEnv::new();
    let config = hermetic_config(env.path());

    // A healthy candidate: `--version` prints and exits 0.
    let good = write_stub_binary(env.path(), "healthy-simard", 0, "simard 9.9.9-canary");

    let results = verify_canary(&good, &[RelaunchGate::Smoke], &config)
        .expect("verify_canary drives the gate and returns a verdict");

    assert_eq!(results.len(), 1);
    assert!(
        results[0].passed,
        "a healthy binary MUST pass Smoke — otherwise the gate is cosmetic / \
         always-red and self-deploy could never converge; got: {}",
        results[0]
    );
    assert!(all_gates_passed(&results));
}

// ---------------------------------------------------------------------------
// Invariant 3 (NO SHORT-CIRCUIT): every requested gate is evaluated & reported.
// ---------------------------------------------------------------------------

#[test]
#[serial(canary_deploy_shape_env)]
fn verify_canary_does_not_short_circuit_on_first_red() {
    let env = HermeticEnv::new();
    let config = hermetic_config(env.path());

    let bad = write_stub_binary(env.path(), "regressed-simard", 101, "boom");

    // Two gates requested; a red first gate must NOT drop the second.
    let results = verify_canary(&bad, &[RelaunchGate::Smoke, RelaunchGate::Smoke], &config)
        .expect("verify_canary returns a verdict per requested gate");

    assert_eq!(
        results.len(),
        2,
        "every requested gate must be evaluated and reported (no short-circuit)"
    );
    assert!(
        results.iter().all(|r| !r.passed),
        "both gates run the same regressed binary and must both be RED"
    );
    assert!(!all_gates_passed(&results));
}

// ---------------------------------------------------------------------------
// Invariant 4 (GATE ORDER): UnitTest is scoped, NOT removed.
// ---------------------------------------------------------------------------

#[test]
fn default_gates_order_is_stable_and_includes_unit_test() {
    let gates = default_gates();
    assert_eq!(
        gates,
        vec![
            RelaunchGate::Smoke,
            RelaunchGate::UnitTest,
            RelaunchGate::GymBaseline,
            RelaunchGate::RpcHealth,
        ],
        "the scoping change narrows the UnitTest gate's invocation, it must NOT \
         drop the gate from the default deploy-authorization set (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// Invariant 5 (AGGREGATION): all_gates_passed is true iff every gate passed.
// ---------------------------------------------------------------------------

#[test]
fn all_gates_passed_is_true_iff_every_gate_passed() {
    let pass = |gate| GateResult {
        gate,
        passed: true,
        detail: "ok".to_string(),
    };
    let fail = |gate| GateResult {
        gate,
        passed: false,
        detail: "regression".to_string(),
    };

    // Vacuous truth: an empty verdict list is not "failed".
    assert!(all_gates_passed(&[]));

    let all_green = vec![pass(RelaunchGate::Smoke), pass(RelaunchGate::UnitTest)];
    assert!(all_gates_passed(&all_green));

    let one_red = vec![pass(RelaunchGate::Smoke), fail(RelaunchGate::UnitTest)];
    assert!(
        !all_gates_passed(&one_red),
        "a single red gate must fail the aggregate — the gate authorizes deploy"
    );
}

// ---------------------------------------------------------------------------
// Invariant 6 (SEC): the canary env allow-list is a minimal, hijack-free floor.
// ---------------------------------------------------------------------------

#[test]
fn canary_gate_env_allowlist_is_deny_by_default_and_hijack_free() {
    let allow = canary_gate_env_allowlist();

    assert!(
        !allow.is_empty(),
        "the allow-list supplies the deploy-shape signals a healthy candidate needs"
    );

    // Every allow-listed name is a Simard deploy-shape signal — the allow-list
    // is NOT a general ambient-env passthrough (deny-by-default).
    for name in &allow {
        assert!(
            name.starts_with("SIMARD_"),
            "allow-list must contain only Simard deploy-shape names, found: {name}"
        );
    }

    // No execution-hijack class name may ever be allow-listed (SEC-D3): a
    // dynamic-loader / shell / git-transport steer must never reach a gate.
    let hijack_prefixes = ["LD_", "DYLD_", "GIT_SSH"];
    let hijack_exact = ["BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS", "IFS"];
    for name in &allow {
        let upper = name.to_ascii_uppercase();
        assert!(
            !hijack_prefixes.iter().any(|p| upper.starts_with(p)),
            "hijack-class prefix name must never be allow-listed: {name}"
        );
        assert!(
            !hijack_exact.iter().any(|e| upper == *e),
            "hijack-class exact name must never be allow-listed: {name}"
        );
    }

    // Deny-by-default: general ambient names are NOT part of the allow-list;
    // the base floor (PATH/HOME/…) is injected by `scrub_gate_env`, not here.
    for denied in ["PATH", "HOME", "LD_PRELOAD", "GIT_SSH_COMMAND"] {
        assert!(
            !allow.iter().any(|n| n == denied),
            "{denied} must not be in the canary deploy-shape allow-list"
        );
    }
}
