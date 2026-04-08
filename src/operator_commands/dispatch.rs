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

pub(super) fn next_required(
    args: &mut impl Iterator<Item = String>,
    label: &'static str,
) -> Result<String, Box<dyn std::error::Error>> {
    args.next()
        .ok_or_else(|| format!("expected {label}").into())
}

pub(super) fn next_optional_path(args: &mut impl Iterator<Item = String>) -> Option<PathBuf> {
    args.next().map(PathBuf::from)
}

pub(super) fn reject_extra_args(
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

    // ---- gym_usage ----

    #[test]
    fn gym_usage_is_non_empty() {
        let usage = gym_usage();
        assert!(!usage.is_empty());
        assert!(usage.contains("simard-gym"));
    }

    // ---- next_required ----

    #[test]
    fn next_required_returns_value() {
        let mut args = vec!["hello".to_string()].into_iter();
        let val = next_required(&mut args, "word").unwrap();
        assert_eq!(val, "hello");
    }

    #[test]
    fn next_required_empty_iterator_errors() {
        let mut args = Vec::<String>::new().into_iter();
        let result = next_required(&mut args, "something");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("expected something"));
    }

    // ---- next_optional_path ----

    #[test]
    fn next_optional_path_with_value() {
        let mut args = vec!["/some/path".to_string()].into_iter();
        let path = next_optional_path(&mut args);
        assert_eq!(path, Some(PathBuf::from("/some/path")));
    }

    #[test]
    fn next_optional_path_empty() {
        let mut args = Vec::<String>::new().into_iter();
        let path = next_optional_path(&mut args);
        assert!(path.is_none());
    }

    // ---- reject_extra_args ----

    #[test]
    fn reject_extra_args_no_extra() {
        let args = Vec::<String>::new().into_iter();
        reject_extra_args(args).unwrap();
    }

    #[test]
    fn reject_extra_args_with_extras() {
        let args = vec!["extra1".to_string(), "extra2".to_string()].into_iter();
        let result = reject_extra_args(args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("extra1"));
        assert!(err.contains("extra2"));
    }

    // ---- dispatch_operator_probe ----

    #[test]
    fn dispatch_operator_probe_no_args_errors() {
        let result = dispatch_operator_probe(Vec::<String>::new());
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_operator_probe_unknown_mode_errors() {
        let result = dispatch_operator_probe(vec!["unknown-mode".to_string()]);
        assert!(result.is_err());
    }

    // ---- dispatch_legacy_gym_cli ----

    #[test]
    fn dispatch_legacy_gym_cli_no_args_errors() {
        let result = dispatch_legacy_gym_cli(Vec::<String>::new());
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_legacy_gym_cli_unknown_command_errors() {
        let result = dispatch_legacy_gym_cli(vec!["bogus".to_string()]);
        assert!(result.is_err());
    }
}
