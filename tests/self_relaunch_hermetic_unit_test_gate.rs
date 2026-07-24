//! End-to-end regression for the hermetic unit-test canary gate
//! (rysweet/Simard#4558).
//!
//! ## What #4558 was
//!
//! `run_unit_test_gate` ran `cargo test` as a child of the **live daemon**. The
//! in-process lib-test suite bound the daemon's fixed socket / locked the shared
//! WAL under the daemon's `SIMARD_STATE_ROOT` and aborted with exit `101` in
//! under 1.3s — reddening a tree that passed clean standalone (9279/0). Every
//! self-deploy for 6+ hours was refused on the `unit-test` gate, pinning
//! `running_commit` while `DeployDrift` grew. A second failure made it
//! undiagnosable: the gate captured only stderr, truncated to 200 bytes, so the
//! operator saw a spinner fragment (`Drop t…`) instead of the failing test name.
//!
//! ## What these tests pin
//!
//! They drive the REAL public gate (`verify_canary` with `RelaunchGate::UnitTest`)
//! against a tiny standalone fixture crate
//! (`tests/fixtures/unit_test_gate_fixture/`) — never the full `simard` suite,
//! so there is no recursive 30-minute run:
//!
//!   * **green** — with the toggle unset the fixture is a clean green tree; the
//!     gate must go GREEN even when a **simulated live daemon** holds the shared
//!     `SIMARD_STATE_ROOT` (proving the hermetic per-run temp state root wins);
//!   * **toolchain pin** — the green tree must still pass when the daemon env has
//!     `CARGO_HOME` / `RUSTUP_HOME` **unset**, proving the gate resolves the
//!     toolchain from the real `HOME` before the hermetic `HOME` override rather
//!     than stranding `cargo`/`rustup` under an empty temp `$HOME/.cargo` (a
//!     fresh #4558-class self-inflicted red);
//!   * **red / diagnosable** — with the toggle set the named test panics; the
//!     gate must go RED and its `failing_detail` must carry the failing test
//!     **name** and a `FAILED` / `panicked at` / `failures:` marker — asserted
//!     NOT to be a truncated `Drop t…` spinner fragment.
//!
//! ## Why these three are `#[ignore]` by default
//!
//! Driving `RelaunchGate::UnitTest` end-to-end spawns a **nested `cargo test`**
//! (the gate's whole job) from *inside* this `cargo test` run. Nested cargo
//! serializes on the global `~/.cargo/.package-cache` lock, so running these in
//! the default suite makes them slow and lock-contended — the exact reason the
//! in-tree `gates::tests::verify_canary_runs_all_gates_without_short_circuit`
//! deliberately EXCLUDES `UnitTest` (its comment cites a 30-minute recursive
//! run). They are therefore `#[ignore]`d so the default `cargo test` stays green
//! and fast, and are run explicitly in a dedicated lane:
//!
//! ```text
//! cargo test --test self_relaunch_hermetic_unit_test_gate -- --ignored --test-threads=1
//! ```
//!
//! The deterministic, subprocess-free half of the #4558 diagnosability contract
//! (a red tree's failing test NAME survives into `failing_detail`) is proven
//! WITHOUT nested cargo by the `extract_failure_detail` unit tests in
//! `src/self_relaunch/gates.rs` — those are the primary always-run red-phase
//! tests. The `#[ignore]`d tests below are the true end-to-end regression guards.
//!
//! ## TDD status
//!
//! Written before the fix. The **red / diagnosable** assertion FAILS against the
//! current code (the current gate reads only stderr; the failing name is on
//! stdout) and passes once the stdout+stderr capture + `extract_failure_detail`
//! land. The **green** and **toolchain-pin** assertions are regression guards
//! for the hermetic `HOME`/state-root override + toolchain pin. Run them with
//! `-- --ignored` (see above) to exercise the end-to-end gate.
//!
//! Constraints honoured: additive; drives only public API; emits nothing itself
//! (the crate logs via `tracing`/OTel); intent-revealing names only.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use simard::self_relaunch::{RelaunchConfig, RelaunchGate, verify_canary};

/// The runtime toggle the fixture reads to become a red tree. Allow-listed into
/// the gate's `canary_env` so the deny-by-default scrub re-injects it into the
/// child; its value is read live from this process's env at spawn time.
const FIXTURE_FAIL_TOGGLE: &str = "SIMARD_GATE_FIXTURE_FAIL";

/// The fully-qualified name of the fixture's panicking test — the name that must
/// survive into `failing_detail` on a red tree.
const RED_TEST_NAME: &str = "fixture_panics_when_toggled";

/// Monotonic suffix so each test's isolated dirs never collide within this
/// process, even though the env-mutating tests are serialized.
static SEQ: AtomicU32 = AtomicU32::new(0);

fn fixture_manifest_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unit_test_gate_fixture")
}

fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-hermetic-gate-{}-{}-{}",
        tag,
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create isolated test dir");
    dir
}

/// A gate config pointed at the fixture crate, with the toggle and
/// `SIMARD_STATE_ROOT` allow-listed so a simulated-live-daemon state root and
/// the red toggle both reach (or are overridden in) the child as the scenario
/// requires. A fresh `canary_target_dir` per call keeps the fixture build
/// isolated.
fn fixture_gate_config(tag: &str) -> RelaunchConfig {
    RelaunchConfig {
        manifest_dir: fixture_manifest_dir(),
        canary_target_dir: unique_dir(&format!("target-{tag}")),
        canary_env: vec![
            "SIMARD_STATE_ROOT".to_string(),
            FIXTURE_FAIL_TOGGLE.to_string(),
        ],
        ..RelaunchConfig::default()
    }
}

/// GREEN: a clean green fixture must pass the `unit-test` gate even while a
/// simulated live daemon holds the shared `SIMARD_STATE_ROOT` — proving the gate
/// isolates into its own per-run temp state root instead of colliding with the
/// daemon's WAL/socket (the #4558 abort).
///
/// Serialized: mutates process-global env (`SIMARD_STATE_ROOT`, and clears the
/// toggle) which a concurrent test's env read could tear.
#[test]
#[ignore = "spawns a nested `cargo test` (fixture build); run in a dedicated lane via `-- --ignored` to avoid ~/.cargo/.package-cache lock contention in the default suite"]
#[serial_test::serial(hermetic_gate_env)]
fn green_fixture_passes_gate_under_simulated_live_daemon() {
    let config = fixture_gate_config("green");
    // Simulate the live daemon: a shared state root the daemon "owns".
    let daemon_state_root = unique_dir("daemon-state-root");

    // SAFETY: serialized under the `hermetic_gate_env` key; no concurrent test
    // in this binary reads these vars while this test runs.
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", &daemon_state_root);
        std::env::remove_var(FIXTURE_FAIL_TOGGLE);
    }

    let results = verify_canary(
        Path::new("/unused-by-unit-test-gate"),
        &[RelaunchGate::UnitTest],
        &config,
    )
    .expect("verify_canary should not error");

    // SAFETY: see above.
    unsafe {
        std::env::remove_var("SIMARD_STATE_ROOT");
    }

    assert_eq!(results.len(), 1);
    assert!(
        results[0].passed,
        "a green fixture must pass the hermetic unit-test gate even under a \
         simulated live daemon holding the shared SIMARD_STATE_ROOT; got: {}",
        results[0].detail
    );
}

/// TOOLCHAIN PIN: the green tree must still pass when the daemon env has
/// `CARGO_HOME` / `RUSTUP_HOME` **unset**. Under the hermetic `HOME` override
/// this only holds if the gate pins the toolchain from the real pre-override
/// `HOME`; without the pin, `cargo`/`rustup` hunt an empty temp `$HOME/.cargo`
/// and abort — a fresh #4558-class self-inflicted red.
///
/// Serialized: mutates process-global env (`CARGO_HOME`/`RUSTUP_HOME`).
#[test]
#[ignore = "spawns a nested `cargo test` (fixture build); run in a dedicated lane via `-- --ignored` to avoid ~/.cargo/.package-cache lock contention in the default suite"]
#[serial_test::serial(hermetic_gate_env)]
fn green_fixture_passes_when_daemon_env_has_no_cargo_or_rustup_home() {
    let config = fixture_gate_config("toolchain");

    // Save and clear CARGO_HOME/RUSTUP_HOME to model a clean systemd unit where
    // the daemon relies on the `$HOME/.cargo` default. Restored after the run.
    let saved_cargo = std::env::var_os("CARGO_HOME");
    let saved_rustup = std::env::var_os("RUSTUP_HOME");

    // SAFETY: serialized under the `hermetic_gate_env` key.
    unsafe {
        std::env::remove_var("CARGO_HOME");
        std::env::remove_var("RUSTUP_HOME");
        std::env::remove_var(FIXTURE_FAIL_TOGGLE);
    }

    let results = verify_canary(
        Path::new("/unused-by-unit-test-gate"),
        &[RelaunchGate::UnitTest],
        &config,
    );

    // SAFETY: restore the original toolchain env before asserting so a panic
    // cannot leak the cleared state into other tests.
    unsafe {
        match saved_cargo {
            Some(v) => std::env::set_var("CARGO_HOME", v),
            None => std::env::remove_var("CARGO_HOME"),
        }
        match saved_rustup {
            Some(v) => std::env::set_var("RUSTUP_HOME", v),
            None => std::env::remove_var("RUSTUP_HOME"),
        }
    }

    let results = results.expect("verify_canary should not error");
    assert_eq!(results.len(), 1);
    assert!(
        results[0].passed,
        "the green fixture must still pass with CARGO_HOME/RUSTUP_HOME unset in \
         the daemon env — the gate must resolve the toolchain from the real HOME \
         before the hermetic HOME override; got: {}",
        results[0].detail
    );
}

/// RED / DIAGNOSABLE: a genuinely failing test must redden the gate AND the
/// `failing_detail` must name the failing test with a structured marker. This is
/// the #4558 diagnosability regression — it FAILS against the current
/// stderr-only, 200-byte gate (the failing name is on stdout) and passes once
/// the stdout+stderr capture + `extract_failure_detail` land.
///
/// Serialized: mutates process-global env (the fail toggle).
#[test]
#[ignore = "spawns a nested `cargo test` (fixture build); run in a dedicated lane via `-- --ignored` to avoid ~/.cargo/.package-cache lock contention in the default suite"]
#[serial_test::serial(hermetic_gate_env)]
fn red_fixture_failing_detail_names_the_failing_test() {
    let config = fixture_gate_config("red");

    // SAFETY: serialized under the `hermetic_gate_env` key.
    unsafe {
        std::env::set_var(FIXTURE_FAIL_TOGGLE, "1");
    }

    let results = verify_canary(
        Path::new("/unused-by-unit-test-gate"),
        &[RelaunchGate::UnitTest],
        &config,
    );

    // SAFETY: clear the toggle before asserting so a panic cannot leak it.
    unsafe {
        std::env::remove_var(FIXTURE_FAIL_TOGGLE);
    }

    let results = results.expect("verify_canary should not error");
    assert_eq!(results.len(), 1);
    let detail = &results[0].detail;

    assert!(
        !results[0].passed,
        "a genuinely failing fixture test must redden the gate (fail-closed); \
         got PASS with detail: {detail}"
    );
    assert!(
        detail.contains(RED_TEST_NAME),
        "failing_detail must NAME the failing test (`{RED_TEST_NAME}`) — the \
         #4558 diagnosability fix; got: {detail}"
    );
    assert!(
        detail.contains("FAILED") || detail.contains("panicked at") || detail.contains("failures:"),
        "failing_detail must carry a structured failure marker \
         (FAILED / panicked at / failures:); got: {detail}"
    );
    assert!(
        !detail.contains("Drop t"),
        "failing_detail must not be a truncated progress-spinner fragment \
         (the #4558 `Drop t…` symptom); got: {detail}"
    );
}

/// Fast, always-run wiring guard (no subprocess): the fixture crate the
/// `#[ignore]`d end-to-end tests point the gate at must exist and be a
/// well-formed standalone package. Keeps the fixture path honest so the
/// dedicated `--ignored` lane never silently no-ops on a moved/renamed fixture.
#[test]
fn fixture_crate_is_present_and_standalone() {
    let dir = fixture_manifest_dir();
    let cargo_toml = dir.join("Cargo.toml");
    assert!(
        cargo_toml.is_file(),
        "fixture Cargo.toml must exist at {}",
        cargo_toml.display()
    );
    let manifest = std::fs::read_to_string(&cargo_toml).expect("read fixture Cargo.toml");
    // The empty `[workspace]` table pins the fixture as its own workspace root so
    // the gate can build it in isolation via `--manifest-path`.
    assert!(
        manifest.contains("[workspace]"),
        "fixture must declare an empty [workspace] so it builds standalone; got:\n{manifest}"
    );
    assert!(
        dir.join("lib.rs").is_file(),
        "fixture lib.rs (the green/red tree) must exist at {}",
        dir.join("lib.rs").display()
    );
}
