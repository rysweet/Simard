use super::dispatch::*;

use std::path::PathBuf;

fn s(value: &str) -> String {
    value.to_string()
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| s(v)).collect()
}

// --- gym_usage ---

#[test]
fn dispatch_operator_probe_review_read_missing_args() {
    let err = dispatch_operator_probe(args(&["review-read"])).unwrap_err();
    assert!(err.to_string().contains("expected base type"));
}

#[test]
fn dispatch_operator_probe_improvement_curation_run_missing_args() {
    let err = dispatch_operator_probe(args(&["improvement-curation-run"])).unwrap_err();
    assert!(err.to_string().contains("expected base type"));
}

#[test]
fn dispatch_operator_probe_improvement_curation_read_missing_args() {
    let err = dispatch_operator_probe(args(&["improvement-curation-read"])).unwrap_err();
    assert!(err.to_string().contains("expected base type"));
}

#[test]
fn dispatch_operator_probe_goal_curation_missing_args() {
    let err = dispatch_operator_probe(args(&["goal-curation-run"])).unwrap_err();
    assert!(err.to_string().contains("expected base type"));
}

// --- dispatch_legacy_gym_cli: more arg validation ---

#[test]
fn dispatch_legacy_gym_cli_compare_missing_scenario_id() {
    let err = dispatch_legacy_gym_cli(args(&["compare"])).unwrap_err();
    assert!(err.to_string().contains("expected scenario id"));
}

#[test]
fn dispatch_legacy_gym_cli_run_suite_missing_suite_id() {
    let err = dispatch_legacy_gym_cli(args(&["run-suite"])).unwrap_err();
    assert!(err.to_string().contains("expected suite id"));
}

#[test]
fn dispatch_legacy_gym_cli_list_rejects_extra_args() {
    let err = dispatch_legacy_gym_cli(args(&["list", "extra"])).unwrap_err();
    assert!(err.to_string().contains("trailing arguments"));
}

#[test]
fn dispatch_legacy_gym_cli_run_rejects_extra_args() {
    let err = dispatch_legacy_gym_cli(args(&["run", "scenario-1", "extra"])).unwrap_err();
    assert!(err.to_string().contains("trailing arguments"));
}

// --- dispatch_operator_probe: trailing argument rejection ---

#[test]
fn dispatch_operator_probe_bootstrap_rejects_extra_args() {
    let err = dispatch_operator_probe(args(&[
        "bootstrap-run",
        "id",
        "local-harness",
        "single-process",
        "objective",
        "/some/path",
        "extra-trailing",
    ]))
    .unwrap_err();
    assert!(err.to_string().contains("trailing arguments"));
}

#[test]
fn dispatch_operator_probe_handoff_rejects_extra_args() {
    let err = dispatch_operator_probe(args(&[
        "handoff-roundtrip",
        "id",
        "local-harness",
        "single-process",
        "objective",
        "extra",
    ]))
    .unwrap_err();
    assert!(err.to_string().contains("trailing arguments"));
}

#[test]
fn dispatch_operator_probe_meeting_run_missing_objective() {
    let err = dispatch_operator_probe(args(&["meeting-run", "local-harness", "single-process"]))
        .unwrap_err();
    assert!(err.to_string().contains("expected objective"));
}

#[test]
fn dispatch_operator_probe_meeting_read_missing_topology() {
    let err = dispatch_operator_probe(args(&["meeting-read", "local-harness"])).unwrap_err();
    assert!(err.to_string().contains("expected topology"));
}

#[test]
fn dispatch_operator_probe_terminal_run_missing_objective() {
    let err = dispatch_operator_probe(args(&["terminal-run", "single-process"])).unwrap_err();
    assert!(err.to_string().contains("expected objective"));
}

#[test]
fn dispatch_operator_probe_terminal_run_file_missing_objective_file() {
    let err = dispatch_operator_probe(args(&["terminal-run-file", "single-process"])).unwrap_err();
    assert!(err.to_string().contains("expected objective file"));
}

#[test]
fn dispatch_operator_probe_terminal_recipe_run_missing_recipe_name() {
    let err =
        dispatch_operator_probe(args(&["terminal-recipe-run", "single-process"])).unwrap_err();
    assert!(err.to_string().contains("expected recipe name"));
}

#[test]
fn dispatch_operator_probe_engineer_loop_missing_workspace() {
    let err = dispatch_operator_probe(args(&["engineer-loop-run", "single-process"])).unwrap_err();
    assert!(err.to_string().contains("expected workspace root"));
}

#[test]
fn dispatch_operator_probe_engineer_loop_missing_objective() {
    let err = dispatch_operator_probe(args(&[
        "engineer-loop-run",
        "single-process",
        "/tmp/workspace",
    ]))
    .unwrap_err();
    assert!(err.to_string().contains("expected objective"));
}

#[test]
fn dispatch_operator_probe_review_run_missing_topology() {
    let err = dispatch_operator_probe(args(&["review-run", "local-harness"])).unwrap_err();
    assert!(err.to_string().contains("expected topology"));
}

#[test]
fn dispatch_operator_probe_review_run_missing_objective() {
    let err = dispatch_operator_probe(args(&["review-run", "local-harness", "single-process"]))
        .unwrap_err();
    assert!(err.to_string().contains("expected objective"));
}

#[test]
fn dispatch_operator_probe_improvement_curation_run_missing_topology() {
    let err =
        dispatch_operator_probe(args(&["improvement-curation-run", "local-harness"])).unwrap_err();
    assert!(err.to_string().contains("expected topology"));
}

#[test]
fn dispatch_operator_probe_improvement_curation_read_missing_topology() {
    let err =
        dispatch_operator_probe(args(&["improvement-curation-read", "local-harness"])).unwrap_err();
    assert!(err.to_string().contains("expected topology"));
}

#[test]
fn dispatch_operator_probe_goal_curation_missing_topology() {
    let err = dispatch_operator_probe(args(&["goal-curation-run", "local-harness"])).unwrap_err();
    assert!(err.to_string().contains("expected topology"));
}

#[test]
fn dispatch_operator_probe_goal_curation_missing_objective() {
    let err = dispatch_operator_probe(args(&[
        "goal-curation-run",
        "local-harness",
        "single-process",
    ]))
    .unwrap_err();
    assert!(err.to_string().contains("expected objective"));
}

#[test]
fn dispatch_legacy_gym_cli_compare_rejects_extra_args() {
    let err = dispatch_legacy_gym_cli(args(&["compare", "scenario-1", "extra"])).unwrap_err();
    assert!(err.to_string().contains("trailing arguments"));
}

#[test]
fn dispatch_legacy_gym_cli_run_suite_rejects_extra_args() {
    let err = dispatch_legacy_gym_cli(args(&["run-suite", "suite-1", "extra"])).unwrap_err();
    assert!(err.to_string().contains("trailing arguments"));
}

#[test]
fn next_required_consumes_only_first_item() {
    let mut iter = args(&["first", "second"]).into_iter();
    assert_eq!(next_required(&mut iter, "a").unwrap(), "first");
    assert_eq!(next_required(&mut iter, "b").unwrap(), "second");
}

#[test]
fn reject_extra_args_single_trailing() {
    let err = reject_extra_args(args(&["only_one"]).into_iter()).unwrap_err();
    assert!(err.to_string().contains("only_one"));
}

// Tests previously inlined in dispatch.rs (#1266 burndown)
mod dispatch_inline {
    use crate::operator_commands::dispatch::*;
    use std::path::PathBuf;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn next_required_returns_value() {
        let mut it = args(&["hello", "world"]).into_iter();
        assert_eq!(next_required(&mut it, "first").unwrap(), "hello");
        assert_eq!(next_required(&mut it, "second").unwrap(), "world");
    }

    #[test]
    fn next_required_error_on_empty() {
        let mut it = std::iter::empty::<String>();
        assert!(next_required(&mut it, "missing").is_err());
    }

    #[test]
    fn next_optional_path_some_and_none() {
        let mut it = args(&["/tmp/test"]).into_iter();
        let p = next_optional_path(&mut it);
        assert_eq!(p, Some(PathBuf::from("/tmp/test")));

        let mut it = std::iter::empty::<String>();
        assert_eq!(next_optional_path(&mut it), None);
    }

    #[test]
    fn reject_extra_args_ok_when_empty() {
        assert!(reject_extra_args(std::iter::empty::<String>()).is_ok());
    }

    #[test]
    fn reject_extra_args_err_with_extra() {
        let result = reject_extra_args(args(&["extra1", "extra2"]).into_iter());
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("extra1"));
        assert!(msg.contains("extra2"));
    }

    #[test]
    fn gym_usage_returns_static_str() {
        let usage = gym_usage();
        assert!(usage.contains("simard-gym"));
        assert!(usage.contains("list"));
        assert!(usage.contains("run-suite"));
    }

    #[test]
    fn dispatch_operator_probe_unknown_command() {
        let result = dispatch_operator_probe(vec!["nonexistent-command".to_string()]);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("unsupported"));
    }

    #[test]
    fn dispatch_legacy_gym_cli_no_args() {
        let result = dispatch_legacy_gym_cli(std::iter::empty::<String>());
        assert!(result.is_err());
    }
}
