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
