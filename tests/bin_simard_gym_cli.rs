//! Integration tests for the `simard-gym` helper bin.
//!
//! `simard-gym` is a 3-line shim: `main()` delegates to
//! `simard::dispatch_legacy_gym_cli(args)`. These tests exercise the bin's
//! observable CLI surface — usage error on no args, unknown-subcommand error,
//! and missing-required-arg errors — without exercising any external service.
//!
//! Filed against rysweet/Simard#1749 (test-coverage: raise bin from 1% to 70%).

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("simard-gym").expect("simard-gym must build")
}

#[test]
fn no_args_prints_usage_and_fails() {
    let assert = bin().assert().failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("usage: simard-gym"),
        "stderr should contain usage hint, got: {stderr}"
    );
    assert!(
        stderr.contains("list") && stderr.contains("run") && stderr.contains("compare"),
        "usage should advertise core subcommands, got: {stderr}"
    );
}

#[test]
fn unknown_subcommand_fails() {
    let assert = bin()
        .arg("definitely-not-a-real-subcommand")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    // dispatch_legacy_gym_cli uses gym_usage() as the error for unknown
    // subcommands (the inner-match exhaustive-fallthrough pattern), so we
    // assert on the usage substring rather than a specific "unknown" word.
    assert!(
        stderr.contains("usage: simard-gym") || stderr.contains("definitely-not-a-real-subcommand"),
        "stderr should surface usage or echo the bad subcommand, got: {stderr}"
    );
}

#[test]
fn run_without_scenario_id_fails() {
    let assert = bin().arg("run").assert().failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("scenario") || stderr.contains("usage"),
        "missing scenario id should produce a clear error, got: {stderr}"
    );
}

#[test]
fn compare_without_scenario_id_fails() {
    let assert = bin().arg("compare").assert().failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("scenario") || stderr.contains("usage"),
        "missing scenario id should produce a clear error, got: {stderr}"
    );
}

#[test]
fn run_suite_without_suite_id_fails() {
    let assert = bin().arg("run-suite").assert().failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("suite") || stderr.contains("usage"),
        "missing suite id should produce a clear error, got: {stderr}"
    );
}

#[test]
fn extra_args_after_list_fail() {
    // `list` takes no positional arguments; passing one must be rejected by
    // dispatch_legacy_gym_cli's reject_extra_args path.
    let assert = bin().args(["list", "extraneous"]).assert().failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        !stderr.is_empty(),
        "extra arg after list should produce an error message"
    );
}
