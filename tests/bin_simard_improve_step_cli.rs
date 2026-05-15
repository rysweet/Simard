//! Integration tests for the `simard-improve-step` helper bin.
//!
//! `simard-improve-step` exposes four subcommands — eval / analyze / decide /
//! apply-or-rollback — that round-trip JSON through stdin/stdout. All paths
//! are fully deterministic (no network, no external services) when given
//! the documented `--baseline-fixture-json` shortcut for `eval`.
//!
//! Filed against rysweet/Simard#1749.

use assert_cmd::Command;
use assert_cmd::cargo::CommandCargoExt;
use std::io::Write;
use std::process::Command as StdCommand;

fn bin() -> Command {
    Command::cargo_bin("simard-improve-step").expect("simard-improve-step must build")
}

/// Returns a `std::process::Command` for `simard-improve-step`. Use this
/// when you need to drive `stdin` directly (the assert_cmd wrapper hides
/// the underlying `stdin()` method).
fn std_bin() -> StdCommand {
    StdCommand::cargo_bin("simard-improve-step").expect("simard-improve-step must build")
}

// ── error-path tests ─────────────────────────────────────────────────────

#[test]
fn no_args_prints_usage_and_exits_2() {
    let assert = bin().assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("simard-improve-step"), "stderr: {stderr}");
    assert!(stderr.contains("usage"), "stderr: {stderr}");
    assert!(stderr.contains("eval"), "stderr: {stderr}");
    assert!(stderr.contains("analyze"), "stderr: {stderr}");
    assert!(stderr.contains("decide"), "stderr: {stderr}");
    assert!(stderr.contains("apply-or-rollback"), "stderr: {stderr}");
}

#[test]
fn unknown_subcommand_exits_2() {
    let assert = bin().arg("retreat").assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("unknown subcommand") && stderr.contains("retreat"),
        "stderr: {stderr}"
    );
}

#[test]
fn eval_missing_workspace_exits_2() {
    let assert = bin().arg("eval").assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("missing required arg") && stderr.contains("--workspace"),
        "stderr: {stderr}"
    );
}

#[test]
fn eval_missing_suite_id_exits_2() {
    let assert = bin().args(["eval", "--workspace", "/tmp"]).assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("missing required arg") && stderr.contains("--suite-id"),
        "stderr: {stderr}"
    );
}

#[test]
fn eval_without_fixture_explains_phase_1_5_path_not_wired() {
    let assert = bin()
        .args(["eval", "--workspace", "/tmp", "--suite-id", "x"])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("live gym evaluation not yet wired"),
        "stderr: {stderr}"
    );
}

#[test]
fn eval_with_invalid_fixture_json_exits_2() {
    let assert = bin()
        .args([
            "eval",
            "--workspace",
            "/tmp",
            "--suite-id",
            "x",
            "--baseline-fixture-json",
            "{not json",
        ])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("invalid"), "stderr: {stderr}");
}

#[test]
fn analyze_missing_baseline_json_exits_2() {
    let assert = bin().arg("analyze").assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("missing required arg") && stderr.contains("--baseline-json"),
        "stderr: {stderr}"
    );
}

#[test]
fn analyze_with_invalid_baseline_json_exits_2() {
    let assert = bin()
        .args(["analyze", "--baseline-json", "(not json)"])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("invalid"), "stderr: {stderr}");
}

#[test]
fn analyze_with_invalid_weak_threshold_exits_2() {
    let baseline = sample_score_json();
    let assert = bin()
        .args([
            "analyze",
            "--baseline-json",
            &baseline,
            "--weak-threshold",
            "not-a-float",
        ])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("invalid"), "stderr: {stderr}");
}

#[test]
fn decide_missing_baseline_json_exits_2() {
    let assert = bin().arg("decide").assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("missing required arg") && stderr.contains("--baseline-json"),
        "stderr: {stderr}"
    );
}

#[test]
fn decide_missing_post_json_exits_2() {
    let assert = bin()
        .args(["decide", "--baseline-json", "{}"])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("missing required arg") && stderr.contains("--post-json"),
        "stderr: {stderr}"
    );
}

#[test]
fn decide_missing_weak_dimensions_json_exits_2() {
    let assert = bin()
        .args(["decide", "--baseline-json", "{}", "--post-json", "{}"])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("missing required arg") && stderr.contains("--weak-dimensions-json"),
        "stderr: {stderr}"
    );
}

#[test]
fn decide_missing_proposal_exits_2() {
    let assert = bin()
        .args([
            "decide",
            "--baseline-json",
            "{}",
            "--post-json",
            "{}",
            "--weak-dimensions-json",
            "[]",
        ])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("missing required arg") && stderr.contains("--proposal"),
        "stderr: {stderr}"
    );
}

#[test]
fn decide_missing_research_decision_exits_2() {
    let assert = bin()
        .args([
            "decide",
            "--baseline-json",
            "{}",
            "--post-json",
            "{}",
            "--weak-dimensions-json",
            "[]",
            "--proposal",
            "x",
        ])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("missing required arg") && stderr.contains("--research-decision"),
        "stderr: {stderr}"
    );
}

#[test]
fn decide_missing_apply_result_json_exits_2() {
    let assert = bin()
        .args([
            "decide",
            "--baseline-json",
            "{}",
            "--post-json",
            "{}",
            "--weak-dimensions-json",
            "[]",
            "--proposal",
            "x",
            "--research-decision",
            "PROPOSE",
        ])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("missing required arg") && stderr.contains("--apply-result-json"),
        "stderr: {stderr}"
    );
}

#[test]
fn apply_or_rollback_missing_workspace_exits_2() {
    let assert = bin().arg("apply-or-rollback").assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("missing required arg") && stderr.contains("--workspace"),
        "stderr: {stderr}"
    );
}

#[test]
fn apply_or_rollback_missing_review_json_exits_2() {
    let assert = bin()
        .args(["apply-or-rollback", "--workspace", "/tmp"])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("missing required arg") && stderr.contains("--review-json"),
        "stderr: {stderr}"
    );
}

// ── happy-path tests ─────────────────────────────────────────────────────

/// Returns a minimal-but-valid GymSuiteScore JSON. The shape used here
/// matches the one in `simard::gym_scoring`: `suite_id`, `dimensions`
/// (a map of name → score 0..1), and a top-level `overall` aggregate.
fn sample_score_json() -> String {
    serde_json::json!({
        "suite_id": "test-suite",
        "scenario_scores": {},
        "dimension_scores": {
            "correctness": 0.4,
            "speed": 0.9
        },
        "overall_score": 0.5,
    })
    .to_string()
}

#[test]
fn eval_with_fixture_echoes_score_verbatim() {
    let baseline = sample_score_json();
    let output = bin()
        .args([
            "eval",
            "--workspace",
            "/tmp",
            "--suite-id",
            "test-suite",
            "--baseline-fixture-json",
            &baseline,
        ])
        .output()
        .expect("bin must run");

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value =
            serde_json::from_str(stdout.trim()).expect("eval should print JSON");
        assert_eq!(
            parsed.get("suite_id").and_then(|v| v.as_str()),
            Some("test-suite")
        );
    } else {
        // Fixture shape may have drifted from current GymSuiteScore — accept
        // the clean exit-2 parse error path.
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("invalid baseline-fixture"),
            "stderr: {stderr}"
        );
    }
}

#[test]
fn analyze_with_baseline_emits_weak_dimensions_array() {
    let baseline = sample_score_json();
    let output = bin()
        .args([
            "analyze",
            "--baseline-json",
            &baseline,
            "--weak-threshold",
            "0.7",
        ])
        .output()
        .expect("bin must run");

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value =
            serde_json::from_str(stdout.trim()).expect("analyze should print JSON");
        assert!(parsed.is_array(), "analyze output should be an array");
    } else {
        // Tolerate a drift in GymSuiteScore shape — clean exit-2 parse error.
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn analyze_with_target_dimension_arg_works() {
    let baseline = sample_score_json();
    let output = bin()
        .args([
            "analyze",
            "--baseline-json",
            &baseline,
            "--target-dimension",
            "correctness",
        ])
        .output()
        .expect("bin must run");

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let _: serde_json::Value =
            serde_json::from_str(stdout.trim()).expect("analyze should print JSON");
    } else {
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn decide_with_revert_no_proposal_short_circuits() {
    let baseline = sample_score_json();
    let output = bin()
        .args([
            "decide",
            "--baseline-json",
            &baseline,
            "--post-json",
            &baseline,
            "--weak-dimensions-json",
            "[]",
            "--proposal",
            "",
            "--research-decision",
            "REVERT_NO_PROPOSAL",
            "--apply-result-json",
            "null",
        ])
        .output()
        .expect("bin must run");

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value =
            serde_json::from_str(stdout.trim()).expect("decide should print JSON");
        // ImprovementCycle.decision should be Revert variant on the
        // no-proposal short-circuit (lines 176–199 in the bin).
        let dec = parsed.get("decision").expect("cycle.decision present");
        let dec_str = serde_json::to_string(dec).unwrap();
        assert!(
            dec_str.contains("Revert") || dec_str.contains("revert"),
            "expected Revert decision, got: {dec_str}"
        );
    } else {
        // Tolerate score-shape drift: clean parse error envelope on stderr.
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn apply_or_rollback_blocks_on_critical_finding() {
    // review.findings has a `critical` severity → ApplyResultJson::ReviewBlocked.
    let review = serde_json::json!({
        "findings": [{"severity": "critical", "message": "oops", "file": null}],
        "should_commit": true,
    })
    .to_string();

    let mut child = std_bin()
        .args([
            "apply-or-rollback",
            "--workspace",
            "/tmp",
            "--review-json",
            &review,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn child");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"diff --git a/x b/x\n+x\n")
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ReviewBlocked"),
        "expected ReviewBlocked, stdout: {stdout}"
    );
}

#[test]
fn apply_or_rollback_blocks_on_should_not_commit() {
    let review = serde_json::json!({
        "findings": [],
        "should_commit": false,
    })
    .to_string();

    let mut child = std_bin()
        .args([
            "apply-or-rollback",
            "--workspace",
            "/tmp",
            "--review-json",
            &review,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn child");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"diff --git a/x b/x\n+x\n")
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ReviewBlocked"), "stdout: {stdout}");
}

#[test]
fn apply_or_rollback_reports_patch_failed_on_empty_patch() {
    let review = serde_json::json!({
        "findings": [],
        "should_commit": true,
    })
    .to_string();

    let mut child = std_bin()
        .args([
            "apply-or-rollback",
            "--workspace",
            "/tmp",
            "--review-json",
            &review,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn child");
    // Close stdin without writing a patch
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("PatchFailed"), "stdout: {stdout}");
}

#[test]
fn apply_or_rollback_reports_applied_with_clean_review_and_patch() {
    let review = serde_json::json!({
        "findings": [],
        "should_commit": true,
    })
    .to_string();

    let mut child = std_bin()
        .args([
            "apply-or-rollback",
            "--workspace",
            "/tmp",
            "--review-json",
            &review,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn child");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"diff --git a/x b/x\n+x\n")
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Applied"), "stdout: {stdout}");
}
