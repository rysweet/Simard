use std::path::PathBuf;

use crate::operator_commands::{
    run_copilot_submit_probe, run_engineer_loop_probe, run_engineer_read_probe, run_terminal_probe,
    run_terminal_probe_from_file, run_terminal_read_probe, run_terminal_recipe_list_probe,
    run_terminal_recipe_probe, run_terminal_recipe_show_probe,
};

use super::args::{
    next_optional_path, next_required, parse_state_root_and_json, reject_extra_args,
};

pub(super) fn dispatch_engineer_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "engineer command")?;
    match subcommand.as_str() {
        "run" => {
            let topology = next_required(&mut args, "topology")?;
            let workspace_root = next_required(&mut args, "workspace root")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_engineer_loop_probe(
                &topology,
                &PathBuf::from(workspace_root),
                &objective,
                state_root,
            )
        }
        "terminal" => {
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_terminal_probe(&topology, &objective, state_root)
        }
        "terminal-file" => {
            let topology = next_required(&mut args, "topology")?;
            let objective_path = next_required(&mut args, "objective file")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_terminal_probe_from_file(&topology, &PathBuf::from(objective_path), state_root)
        }
        "terminal-read" => {
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_terminal_read_probe(&topology, state_root)
        }
        "terminal-recipe-list" => {
            reject_extra_args(args)?;
            run_terminal_recipe_list_probe()
        }
        "terminal-recipe-show" => {
            let recipe_name = next_required(&mut args, "recipe name")?;
            reject_extra_args(args)?;
            run_terminal_recipe_show_probe(&recipe_name)
        }
        "terminal-recipe" => {
            let topology = next_required(&mut args, "topology")?;
            let recipe_name = next_required(&mut args, "recipe name")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_terminal_recipe_probe(&topology, &recipe_name, state_root)
        }
        "copilot-submit" => {
            let topology = next_required(&mut args, "topology")?;
            let trailing = args.collect::<Vec<_>>();
            let (state_root, json_output) = parse_state_root_and_json(trailing)?;
            run_copilot_submit_probe(&topology, state_root, json_output)
        }
        "read" => {
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_engineer_read_probe(&topology, state_root)
        }
        other => Err(format!("unsupported command 'engineer {other}'").into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::operator_cli::dispatch_operator_cli;

    #[test]
    fn test_engineer_missing_subcommand() {
        let result = dispatch_operator_cli(vec!["engineer".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected engineer command")
        );
    }

    #[test]
    fn test_engineer_unknown_subcommand() {
        let result = dispatch_operator_cli(vec!["engineer".to_string(), "nope".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command 'engineer nope'")
        );
    }

    #[test]
    fn test_engineer_run_missing_topology() {
        let result = dispatch_operator_cli(vec!["engineer".to_string(), "run".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected topology")
        );
    }

    #[test]
    fn test_engineer_run_missing_workspace_root() {
        let result = dispatch_operator_cli(vec![
            "engineer".to_string(),
            "run".to_string(),
            "single-process".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected workspace root")
        );
    }

    #[test]
    fn test_engineer_run_missing_objective() {
        let result = dispatch_operator_cli(vec![
            "engineer".to_string(),
            "run".to_string(),
            "single-process".to_string(),
            "/workspace".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected objective")
        );
    }

    #[test]
    fn test_engineer_terminal_missing_args() {
        let result = dispatch_operator_cli(vec!["engineer".to_string(), "terminal".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected topology")
        );
    }

    #[test]
    fn test_engineer_terminal_file_missing_args() {
        let result =
            dispatch_operator_cli(vec!["engineer".to_string(), "terminal-file".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_engineer_terminal_read_missing_topology() {
        let result =
            dispatch_operator_cli(vec!["engineer".to_string(), "terminal-read".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected topology")
        );
    }

    #[test]
    fn test_engineer_read_missing_topology() {
        let result = dispatch_operator_cli(vec!["engineer".to_string(), "read".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected topology")
        );
    }

    #[test]
    fn test_engineer_terminal_recipe_show_missing_name() {
        let result = dispatch_operator_cli(vec![
            "engineer".to_string(),
            "terminal-recipe-show".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected recipe name")
        );
    }

    #[test]
    fn test_engineer_terminal_recipe_missing_args() {
        let result =
            dispatch_operator_cli(vec!["engineer".to_string(), "terminal-recipe".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_engineer_copilot_submit_missing_topology() {
        let result =
            dispatch_operator_cli(vec!["engineer".to_string(), "copilot-submit".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected topology")
        );
    }

    #[test]
    fn test_engineer_terminal_recipe_list_rejects_extra() {
        let result = dispatch_operator_cli(vec![
            "engineer".to_string(),
            "terminal-recipe-list".to_string(),
            "extra".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected trailing")
        );
    }

    #[test]
    fn test_engineer_terminal_recipe_show_rejects_extra() {
        let result = dispatch_operator_cli(vec![
            "engineer".to_string(),
            "terminal-recipe-show".to_string(),
            "name".to_string(),
            "extra".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected trailing")
        );
    }

    #[test]
    fn test_engineer_run_rejects_trailing_after_state_root() {
        let result = dispatch_operator_cli(vec![
            "engineer".to_string(),
            "run".to_string(),
            "topology".to_string(),
            "/workspace".to_string(),
            "objective".to_string(),
            "/state".to_string(),
            "extra".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected trailing")
        );
    }

    #[test]
    fn test_engineer_terminal_missing_objective() {
        let result = dispatch_operator_cli(vec![
            "engineer".to_string(),
            "terminal".to_string(),
            "topology".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected objective")
        );
    }

    #[test]
    fn test_engineer_terminal_file_missing_objective_file() {
        let result = dispatch_operator_cli(vec![
            "engineer".to_string(),
            "terminal-file".to_string(),
            "topology".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected objective file")
        );
    }

    #[test]
    fn test_engineer_terminal_recipe_missing_recipe_name() {
        let result = dispatch_operator_cli(vec![
            "engineer".to_string(),
            "terminal-recipe".to_string(),
            "topology".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected recipe name")
        );
    }
}
