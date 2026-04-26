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
fn gym_usage_contains_expected_subcommands() {
    let usage = gym_usage();
    assert!(usage.contains("list"), "should mention 'list'");
    assert!(usage.contains("run"), "should mention 'run'");
    assert!(usage.contains("compare"), "should mention 'compare'");
    assert!(usage.contains("run-suite"), "should mention 'run-suite'");
    assert!(usage.contains("simard-gym"), "should mention binary name");
}

// --- next_required / next_optional_path / reject_extra_args ---

#[test]
fn next_required_returns_value_when_present() {
    let mut iter = args(&["hello"]).into_iter();
    assert_eq!(next_required(&mut iter, "greeting").unwrap(), "hello");
}

#[test]
fn next_required_errors_when_empty() {
    let mut iter = std::iter::empty::<String>();
    let err = next_required(&mut iter, "widget").unwrap_err();
    assert!(err.to_string().contains("expected widget"));
}

#[test]
fn next_optional_path_returns_some() {
    let mut iter = args(&["/a/b"]).into_iter();
    assert_eq!(next_optional_path(&mut iter), Some(PathBuf::from("/a/b")));
}

#[test]
fn next_optional_path_returns_none_when_empty() {
    let mut iter = std::iter::empty::<String>();
    assert_eq!(next_optional_path(&mut iter), None);
}

#[test]
fn reject_extra_args_ok_when_empty() {
    reject_extra_args(std::iter::empty::<String>()).unwrap();
}

#[test]
fn reject_extra_args_errors_on_trailing() {
    let err = reject_extra_args(args(&["extra1", "extra2"]).into_iter()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("extra1"));
    assert!(msg.contains("extra2"));
}

// --- dispatch_operator_probe: argument validation ---

#[test]
fn dispatch_operator_probe_no_command() {
    let err = dispatch_operator_probe(std::iter::empty::<String>()).unwrap_err();
    assert!(err.to_string().contains("expected a probe command"));
}

#[test]
fn dispatch_operator_probe_unknown_command() {
    let err = dispatch_operator_probe(args(&["nonexistent"])).unwrap_err();
    assert!(err.to_string().contains("unsupported probe command"));
    assert!(err.to_string().contains("nonexistent"));
}

#[test]
fn dispatch_operator_probe_missing_required_args() {
    let err = dispatch_operator_probe(args(&["bootstrap-run"])).unwrap_err();
    assert!(err.to_string().contains("expected identity"));
}

// --- dispatch_legacy_gym_cli: argument validation ---

#[test]
fn dispatch_legacy_gym_cli_no_command() {
    let err = dispatch_legacy_gym_cli(std::iter::empty::<String>()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("simard-gym"), "should show usage on no args");
}

#[test]
fn dispatch_legacy_gym_cli_unknown_command() {
    let err = dispatch_legacy_gym_cli(args(&["bogus"])).unwrap_err();
    assert!(err.to_string().contains("simard-gym"));
}

#[test]
fn dispatch_legacy_gym_cli_run_missing_scenario_id() {
    let err = dispatch_legacy_gym_cli(args(&["run"])).unwrap_err();
    assert!(err.to_string().contains("expected scenario id"));
}

// --- dispatch_operator_probe: more arg validation ---

#[test]
fn dispatch_operator_probe_bootstrap_missing_topology() {
    let err = dispatch_operator_probe(args(&["bootstrap-run", "id", "type"])).unwrap_err();
    assert!(err.to_string().contains("expected topology"));
}

#[test]
fn dispatch_operator_probe_bootstrap_missing_objective() {
    let err = dispatch_operator_probe(args(&["bootstrap-run", "id", "type", "topo"])).unwrap_err();
    assert!(err.to_string().contains("expected objective"));
}

#[test]
fn dispatch_operator_probe_handoff_missing_args() {
    let err = dispatch_operator_probe(args(&["handoff-roundtrip"])).unwrap_err();
    assert!(err.to_string().contains("expected identity"));
}

#[test]
fn dispatch_operator_probe_meeting_run_missing_args() {
    let err = dispatch_operator_probe(args(&["meeting-run"])).unwrap_err();
    assert!(err.to_string().contains("expected base type"));
}

#[test]
fn dispatch_operator_probe_meeting_read_missing_args() {
    let err = dispatch_operator_probe(args(&["meeting-read"])).unwrap_err();
    assert!(err.to_string().contains("expected base type"));
}

#[test]
fn dispatch_operator_probe_terminal_run_missing_args() {
    let err = dispatch_operator_probe(args(&["terminal-run"])).unwrap_err();
    assert!(err.to_string().contains("expected topology"));
}

#[test]
fn dispatch_operator_probe_terminal_run_file_missing_args() {
    let err = dispatch_operator_probe(args(&["terminal-run-file"])).unwrap_err();
    assert!(err.to_string().contains("expected topology"));
}

#[test]
fn dispatch_operator_probe_terminal_read_missing_args() {
    let err = dispatch_operator_probe(args(&["terminal-read"])).unwrap_err();
    assert!(err.to_string().contains("expected topology"));
}

#[test]
fn dispatch_operator_probe_terminal_recipe_show_missing_args() {
    let err = dispatch_operator_probe(args(&["terminal-recipe-show"])).unwrap_err();
    assert!(err.to_string().contains("expected recipe name"));
}

#[test]
fn dispatch_operator_probe_terminal_recipe_run_missing_args() {
    let err = dispatch_operator_probe(args(&["terminal-recipe-run"])).unwrap_err();
    assert!(err.to_string().contains("expected topology"));
}

#[test]
fn dispatch_operator_probe_engineer_loop_missing_args() {
    let err = dispatch_operator_probe(args(&["engineer-loop-run"])).unwrap_err();
    assert!(err.to_string().contains("expected topology"));
}

#[test]
fn dispatch_operator_probe_engineer_read_missing_args() {
    let err = dispatch_operator_probe(args(&["engineer-read"])).unwrap_err();
    assert!(err.to_string().contains("expected topology"));
}

#[test]
fn dispatch_operator_probe_review_run_missing_args() {
    let err = dispatch_operator_probe(args(&["review-run"])).unwrap_err();
    assert!(err.to_string().contains("expected base type"));
}
