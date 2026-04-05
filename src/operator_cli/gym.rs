use crate::operator_commands::{run_gym_compare, run_gym_list, run_gym_scenario, run_gym_suite};

use super::args::{next_required, reject_extra_args};

pub(super) fn dispatch_gym_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "gym command")?;
    match subcommand.as_str() {
        "list" => {
            reject_extra_args(args)?;
            run_gym_list()
        }
        "run" => {
            let scenario_id = next_required(&mut args, "scenario id")?;
            reject_extra_args(args)?;
            run_gym_scenario(&scenario_id)
        }
        "compare" => {
            let scenario_id = next_required(&mut args, "scenario id")?;
            reject_extra_args(args)?;
            run_gym_compare(&scenario_id)
        }
        "run-suite" => {
            let suite_id = next_required(&mut args, "suite id")?;
            reject_extra_args(args)?;
            run_gym_suite(&suite_id)
        }
        other => Err(format!("unsupported command 'gym {other}'").into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::operator_cli::dispatch_operator_cli;

    #[test]
    fn test_gym_missing_subcommand() {
        let result = dispatch_operator_cli(vec!["gym".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected gym command")
        );
    }

    #[test]
    fn test_gym_unknown_subcommand() {
        let result = dispatch_operator_cli(vec!["gym".to_string(), "nope".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command 'gym nope'")
        );
    }

    #[test]
    fn test_gym_run_missing_scenario_id() {
        let result = dispatch_operator_cli(vec!["gym".to_string(), "run".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected scenario id")
        );
    }

    #[test]
    fn test_gym_compare_missing_scenario_id() {
        let result = dispatch_operator_cli(vec!["gym".to_string(), "compare".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected scenario id")
        );
    }

    #[test]
    fn test_gym_run_suite_missing_suite_id() {
        let result = dispatch_operator_cli(vec!["gym".to_string(), "run-suite".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected suite id")
        );
    }

    #[test]
    fn test_gym_run_rejects_extra_args() {
        let result = dispatch_operator_cli(vec![
            "gym".to_string(),
            "run".to_string(),
            "scenario1".to_string(),
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
    fn test_gym_compare_rejects_extra_args() {
        let result = dispatch_operator_cli(vec![
            "gym".to_string(),
            "compare".to_string(),
            "scenario1".to_string(),
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
    fn test_gym_run_suite_rejects_extra_args() {
        let result = dispatch_operator_cli(vec![
            "gym".to_string(),
            "run-suite".to_string(),
            "suite1".to_string(),
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
    fn test_gym_list_rejects_extra_args() {
        let result = dispatch_operator_cli(vec![
            "gym".to_string(),
            "list".to_string(),
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
}
