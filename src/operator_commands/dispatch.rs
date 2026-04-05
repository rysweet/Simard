use std::path::{Path, PathBuf};

use super::{
    run_bootstrap_probe, run_engineer_loop_probe, run_engineer_read_probe, run_goal_curation_probe,
    run_gym_compare, run_gym_list, run_gym_scenario, run_gym_suite, run_handoff_probe,
    run_improvement_curation_probe, run_improvement_curation_read_probe, run_meeting_probe,
    run_meeting_read_probe, run_review_probe, run_review_read_probe, run_terminal_probe,
    run_terminal_probe_from_file, run_terminal_read_probe, run_terminal_recipe_list_probe,
    run_terminal_recipe_probe, run_terminal_recipe_show_probe,
};

pub fn dispatch_operator_probe<I>(args: I) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let command = args.next().ok_or("expected a probe command")?;

    match command.as_str() {
        "bootstrap-run" => {
            let identity = next_required(&mut args, "identity")?;
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_bootstrap_probe(&identity, &base_type, &topology, &objective, state_root)?;
        }
        "handoff-roundtrip" => {
            let identity = next_required(&mut args, "identity")?;
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            reject_extra_args(args)?;
            run_handoff_probe(&identity, &base_type, &topology, &objective)?;
        }
        "meeting-run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_meeting_probe(&base_type, &topology, &objective, state_root)?;
        }
        "meeting-read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_meeting_read_probe(&base_type, &topology, state_root)?;
        }
        "goal-curation-run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_goal_curation_probe(&base_type, &topology, &objective, state_root)?;
        }
        "terminal-run" => {
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_terminal_probe(&topology, &objective, state_root)?;
        }
        "terminal-run-file" => {
            let topology = next_required(&mut args, "topology")?;
            let objective_path = next_required(&mut args, "objective file")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_terminal_probe_from_file(&topology, Path::new(&objective_path), state_root)?;
        }
        "terminal-read" => {
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_terminal_read_probe(&topology, state_root)?;
        }
        "terminal-recipe-list" => {
            reject_extra_args(args)?;
            run_terminal_recipe_list_probe()?;
        }
        "terminal-recipe-show" => {
            let recipe_name = next_required(&mut args, "recipe name")?;
            reject_extra_args(args)?;
            run_terminal_recipe_show_probe(&recipe_name)?;
        }
        "terminal-recipe-run" => {
            let topology = next_required(&mut args, "topology")?;
            let recipe_name = next_required(&mut args, "recipe name")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_terminal_recipe_probe(&topology, &recipe_name, state_root)?;
        }
        "engineer-loop-run" => {
            let topology = next_required(&mut args, "topology")?;
            let workspace_root = next_required(&mut args, "workspace root")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_engineer_loop_probe(
                &topology,
                Path::new(&workspace_root),
                &objective,
                state_root,
            )?;
        }
        "engineer-read" => {
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_engineer_read_probe(&topology, state_root)?;
        }
        "review-run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_review_probe(&base_type, &topology, &objective, state_root)?;
        }
        "review-read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_review_read_probe(&base_type, &topology, state_root)?;
        }
        "improvement-curation-run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_improvement_curation_probe(&base_type, &topology, &objective, state_root)?;
        }
        "improvement-curation-read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_improvement_curation_read_probe(&base_type, &topology, state_root)?;
        }
        other => return Err(format!("unsupported probe command '{other}'").into()),
    }

    Ok(())
}

pub fn dispatch_legacy_gym_cli<I>(args: I) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let command = args.next().ok_or(gym_usage())?;

    match command.as_str() {
        "list" => {
            reject_extra_args(args)?;
            run_gym_list()?;
        }
        "run" => {
            let scenario_id = next_required(&mut args, "scenario id")?;
            reject_extra_args(args)?;
            run_gym_scenario(&scenario_id)?;
        }
        "compare" => {
            let scenario_id = next_required(&mut args, "scenario id")?;
            reject_extra_args(args)?;
            run_gym_compare(&scenario_id)?;
        }
        "run-suite" => {
            let suite_id = next_required(&mut args, "suite id")?;
            reject_extra_args(args)?;
            run_gym_suite(&suite_id)?;
        }
        _ => return Err(gym_usage().into()),
    }

    Ok(())
}

pub fn gym_usage() -> &'static str {
    "usage: simard-gym <list|run <scenario-id>|compare <scenario-id>|run-suite <suite-id>>"
}

fn next_required(
    args: &mut impl Iterator<Item = String>,
    label: &'static str,
) -> Result<String, Box<dyn std::error::Error>> {
    args.next()
        .ok_or_else(|| format!("expected {label}").into())
}

fn next_optional_path(args: &mut impl Iterator<Item = String>) -> Option<PathBuf> {
    args.next().map(PathBuf::from)
}

fn reject_extra_args(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(extra) = args.next() {
        let mut extras = vec![extra];
        extras.extend(args);
        return Err(format!("unexpected trailing arguments: {}", extras.join(" ")).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let err =
            dispatch_operator_probe(args(&["bootstrap-run", "id", "type", "topo"])).unwrap_err();
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
        let err =
            dispatch_operator_probe(args(&["meeting-run", "local-harness", "single-process"]))
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
        let err =
            dispatch_operator_probe(args(&["terminal-run-file", "single-process"])).unwrap_err();
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
        let err =
            dispatch_operator_probe(args(&["engineer-loop-run", "single-process"])).unwrap_err();
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
        let err = dispatch_operator_probe(args(&["improvement-curation-run", "local-harness"]))
            .unwrap_err();
        assert!(err.to_string().contains("expected topology"));
    }

    #[test]
    fn dispatch_operator_probe_improvement_curation_read_missing_topology() {
        let err = dispatch_operator_probe(args(&["improvement-curation-read", "local-harness"]))
            .unwrap_err();
        assert!(err.to_string().contains("expected topology"));
    }

    #[test]
    fn dispatch_operator_probe_goal_curation_missing_topology() {
        let err =
            dispatch_operator_probe(args(&["goal-curation-run", "local-harness"])).unwrap_err();
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
}
