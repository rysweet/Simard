use assert_cmd::Command;
use serde_json::Value;

fn simard() -> Command {
    let mut command = Command::cargo_bin("simard").expect("simard binary");
    command.env("SIMARD_TYPED_OODA_FIXTURE", "1");
    command
}

fn output_json(assert: assert_cmd::assert::Assert) -> Value {
    serde_json::from_slice(&assert.get_output().stdout).expect("JSON output")
}

#[test]
fn fixture_completes_action_and_no_action_cycles_from_durable_records() {
    let state = tempfile::tempdir().expect("state");
    let action = output_json(
        simard()
            .args(["ooda", "fixture", "run", "--state-root"])
            .arg(state.path())
            .args([
                "--scenario",
                "spawn-engineer",
                "--request-id",
                "fixture-action-1",
            ])
            .assert()
            .success(),
    );
    assert_eq!(action["outcome"]["kind"], "action");
    assert_eq!(action["effect"]["state"], "succeeded");

    let no_action = output_json(
        simard()
            .args(["ooda", "fixture", "run", "--state-root"])
            .arg(state.path())
            .args([
                "--scenario",
                "no-action",
                "--request-id",
                "fixture-no-action-1",
            ])
            .assert()
            .success(),
    );
    assert_eq!(no_action["outcome"]["kind"], "no_action");
    assert!(no_action["effect"].is_null());

    let listed = output_json(
        simard()
            .args(["ooda", "outcomes", "list", "--state-root"])
            .arg(state.path())
            .args(["--limit", "10"])
            .assert()
            .success(),
    );
    assert_eq!(listed["outcomes"].as_array().expect("outcomes").len(), 2);
}

#[test]
fn fixture_is_rejected_without_explicit_test_gate() {
    let state = tempfile::tempdir().expect("state");
    Command::cargo_bin("simard")
        .expect("simard binary")
        .args(["ooda", "fixture", "run", "--state-root"])
        .arg(state.path())
        .args(["--scenario", "no-action", "--request-id", "fixture-denied"])
        .assert()
        .failure();
}
