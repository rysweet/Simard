use super::types::{EngineerActionKind, ExecutedEngineerAction};
use super::verification_actions::*;

fn make_action(
    kind: EngineerActionKind,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) -> ExecutedEngineerAction {
    ExecutedEngineerAction {
        selected: super::types::SelectedEngineerAction {
            label: "test-action".to_string(),
            rationale: "testing".to_string(),
            argv: vec!["test".to_string()],
            plan_summary: "test plan".to_string(),
            verification_steps: vec!["step1".to_string()],
            expected_changed_files: vec![],
            kind,
        },
        exit_code,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        changed_files: vec![],
    }
}

#[test]
fn verify_cargo_test_passed() {
    let action = make_action(
        EngineerActionKind::CargoTest,
        0,
        "test result: ok. 10 passed; 0 failed",
        "",
    );
    let mut checks = Vec::new();
    verify_cargo_test(&action, &mut checks).unwrap();
    assert!(checks.iter().any(|c| c.contains("cargo-test-passed=true")));
}

#[test]
fn verify_cargo_test_failed_output() {
    let action = make_action(
        EngineerActionKind::CargoTest,
        1,
        "test result: FAILED. 8 passed; 2 failed",
        "",
    );
    let mut checks = Vec::new();
    verify_cargo_test(&action, &mut checks).unwrap();
    assert!(checks.iter().any(|c| c.contains("cargo-test-passed=false")));
}

#[test]
fn verify_cargo_test_no_output_nonzero_exit() {
    let action = make_action(EngineerActionKind::CargoTest, 101, "", "compile error");
    let mut checks = Vec::new();
    let result = verify_cargo_test(&action, &mut checks);
    assert!(result.is_err());
}

#[test]
fn verify_cargo_check_success() {
    let action = make_action(EngineerActionKind::CargoCheck, 0, "", "");
    let mut checks = Vec::new();
    verify_cargo_check(&action, &mut checks);
    assert!(checks.iter().any(|c| c.contains("cargo-check-passed=true")));
}

#[test]
fn verify_cargo_check_failure_counts_errors() {
    let action = make_action(
        EngineerActionKind::CargoCheck,
        1,
        "",
        "error[E0001]: something\nerror[E0002]: other\nwarning: blah",
    );
    let mut checks = Vec::new();
    verify_cargo_check(&action, &mut checks);
    assert!(checks.iter().any(|c| c.contains("errors=2")));
}

#[test]
fn verify_open_issue_with_url() {
    let action = make_action(
        EngineerActionKind::OpenIssue(super::types::OpenIssueRequest {
            title: "test".to_string(),
            body: "body".to_string(),
            labels: vec![],
        }),
        0,
        "https://github.com/user/repo/issues/1",
        "",
    );
    let mut checks = Vec::new();
    verify_open_issue(&action, &mut checks).unwrap();
    assert!(checks.iter().any(|c| c.contains("issue-url-present=true")));
}

#[test]
fn verify_open_issue_no_url_fails() {
    let action = make_action(
        EngineerActionKind::OpenIssue(super::types::OpenIssueRequest {
            title: "test".to_string(),
            body: "body".to_string(),
            labels: vec![],
        }),
        0,
        "no url here",
        "",
    );
    let mut checks = Vec::new();
    assert!(verify_open_issue(&action, &mut checks).is_err());
}

#[test]
fn build_verification_summary_cargo_test() {
    let action = make_action(EngineerActionKind::CargoTest, 0, "", "");
    let summary = build_verification_summary(&action);
    assert!(summary.contains("passed"));
}

#[test]
fn build_verification_summary_read_only() {
    let action = make_action(EngineerActionKind::ReadOnlyScan, 0, "", "");
    let summary = build_verification_summary(&action);
    assert!(summary.contains("local-only"));
}
