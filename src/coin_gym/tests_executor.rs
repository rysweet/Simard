use super::executor::{
    CoinEvaluateConfig, CoinEvaluateExecutor, GradeResult, HarnessExecutor, MockHarnessExecutor,
};
use super::types::{OutcomeCode, Target, TargetFamily};

fn target(id: &str) -> Target {
    Target {
        id: id.to_string(),
        project: "libraw".to_string(),
        commit: "abc".to_string(),
        harness: "libraw_raf_fuzzer".to_string(),
        file: "src/metadata/fuji.cpp".to_string(),
        line: 480,
        family: TargetFamily::Frontier,
    }
}

#[test]
fn grade_result_maps_to_outcome_code() {
    assert_eq!(GradeResult::Reached.to_outcome_code(), OutcomeCode::Reached);
    assert_eq!(
        GradeResult::WrongInput.to_outcome_code(),
        OutcomeCode::WrongInput
    );
    assert_eq!(
        GradeResult::TimedOut.to_outcome_code(),
        OutcomeCode::TimedOut
    );
    assert_eq!(GradeResult::Error.to_outcome_code(), OutcomeCode::Error);
}

#[test]
fn mock_reaches_only_on_matching_input() {
    let exec = MockHarnessExecutor::new().with_reaching_input("t1", "magic-bytes");
    let t = target("t1");
    assert_eq!(exec.grade(&t, "magic-bytes").unwrap(), GradeResult::Reached);
    assert_eq!(exec.grade(&t, "wrong").unwrap(), GradeResult::WrongInput);
}

#[test]
fn mock_unknown_target_is_wrong_input() {
    let exec = MockHarnessExecutor::new();
    assert_eq!(
        exec.grade(&target("unknown"), "anything").unwrap(),
        GradeResult::WrongInput
    );
}

#[test]
fn mock_injected_timeout_and_error_take_precedence() {
    let exec = MockHarnessExecutor::new()
        .with_reaching_input("t1", "ok")
        .with_timeout("t1")
        .with_error("t2");
    // error dominates for t2
    assert_eq!(exec.grade(&target("t2"), "x").unwrap(), GradeResult::Error);
    // timeout dominates even the correct input for t1
    assert_eq!(
        exec.grade(&target("t1"), "ok").unwrap(),
        GradeResult::TimedOut
    );
}

#[test]
fn mock_is_flagged_offline_scaffold() {
    assert!(MockHarnessExecutor::new().is_offline_scaffold());
}

#[test]
fn coin_evaluate_delegate_is_phase3_gated() {
    let exec = CoinEvaluateExecutor::new(CoinEvaluateConfig::new("you/coin@v1"));
    assert!(!exec.is_offline_scaffold());
    let err = exec.grade(&target("t1"), "bytes").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Docker"));
    assert!(msg.contains("Phase 3"));
}

#[test]
fn coin_evaluate_builds_delegation_argv() {
    let exec = CoinEvaluateExecutor::new(CoinEvaluateConfig::new("you/coin@v1"));
    let argv = exec.build_argv(&target("libraw-fuji-480"), "/tmp/input.bin");
    assert_eq!(
        argv,
        vec![
            "coin".to_string(),
            "evaluate".to_string(),
            "--dataset".to_string(),
            "you/coin@v1".to_string(),
            "--target".to_string(),
            "libraw-fuji-480".to_string(),
            "--input".to_string(),
            "/tmp/input.bin".to_string(),
        ]
    );
}

#[test]
fn from_oracle_seeds_reaching_inputs() {
    let mut oracle = std::collections::HashMap::new();
    oracle.insert("t1".to_string(), "seed".to_string());
    let exec = MockHarnessExecutor::from_oracle(oracle);
    assert_eq!(
        exec.grade(&target("t1"), "seed").unwrap(),
        GradeResult::Reached
    );
}
