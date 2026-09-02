//! Integration tests for the `simard_operator_probe` helper bin.
//!
//! `simard_operator_probe` is a 3-line shim: `main()` delegates to
//! `simard::dispatch_operator_probe(args)`. These tests exercise the
//! observable CLI surface — usage error on no args and unknown-command
//! errors — without exercising any external service.
//!
//! Filed against rysweet/Simard#1749.

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("simard_operator_probe").expect("simard_operator_probe must build")
}

#[test]
fn no_args_fails_with_clear_message() {
    let assert = bin().assert().failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let msg = format!("{stderr}{stdout}");
    assert!(
        msg.contains("expected a probe command") || msg.contains("Error"),
        "expected probe-command hint, got: {msg}"
    );
}

#[test]
fn unknown_subcommand_fails() {
    let assert = bin().arg("definitely-not-a-real-probe").assert().failure();
    // dispatch_operator_probe returns an error for unknown commands; the
    // exact wording is library-internal so we just verify clean failure.
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        !stderr.is_empty() || !stdout.is_empty(),
        "expected an error message"
    );
}

#[test]
fn bootstrap_run_missing_args_fails() {
    let assert = bin().arg("bootstrap-run").assert().failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let msg = format!("{stderr}{stdout}");
    assert!(
        msg.contains("expected") || msg.contains("identity") || msg.contains("Error"),
        "missing-args message expected, got: {msg}"
    );
}

#[test]
fn coin_gym_verify_passes_and_reports_local_done_gate() {
    // The LOCAL COIN Gym harness done-gate is hermetic and offline, so the
    // operator-probe surface must exit 0 on the built-in sample and print the
    // per-criterion PASS matrix plus the aggregate result. This proves the
    // repo-grounded harness is reachable from the operator-probe binary, not
    // only from the standalone `coin-gym` binary.
    let assert = bin().arg("coin-gym-verify").assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        stdout.contains("Probe mode: coin-gym-verify"),
        "expected probe header, got: {stdout}"
    );
    assert!(
        stdout.contains("7/7 LOCAL acceptance criteria passed"),
        "expected all done-gate criteria to pass, got: {stdout}"
    );
    // LOCAL-only framing must be present so the surface never implies live VM
    // grading or external result posting.
    assert!(
        stdout.contains("LOCAL offline harness only"),
        "expected LOCAL-only scope note, got: {stdout}"
    );
}

#[test]
fn coin_gym_verify_rejects_trailing_args() {
    // The done-gate takes no arguments; trailing tokens are an operator error
    // and must fail cleanly rather than silently ignoring them.
    let assert = bin()
        .args(["coin-gym-verify", "unexpected"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let msg = format!("{stderr}{stdout}");
    assert!(
        msg.contains("unexpected trailing arguments"),
        "expected trailing-argument rejection, got: {msg}"
    );
}
