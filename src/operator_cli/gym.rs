use crate::BenchmarkScenarioSet;
use crate::operator_commands::{run_gym_compare, run_gym_list, run_gym_scenario, run_gym_suite};

use super::args::{next_required, reject_extra_args};

pub(super) const GYM_HELP: &str = "\
Simard gym subcommand

Usage: simard gym <command> [args]

Commands:
  list [extended]             List gym scenarios. Defaults to the core V1
                              high-signal set; pass 'extended' for all classes.
  run <scenario-id>           Run a specific gym scenario.
  compare <scenario-id>       Compare results for a scenario.
  run-suite <suite-id>        Run a scenario suite. 'starter' runs the core V1
                              set (default); 'extended' runs all scenarios.
  help, -h, --help            Show this help message and exit.
";

pub(super) fn dispatch_gym_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "gym command")?;
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{GYM_HELP}");
            Ok(())
        }
        "list" => {
            let set = parse_list_scenario_set(args)?;
            run_gym_list(set)
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

/// Parses the optional selector for `gym list`. No argument resolves to the
/// core V1 set; `extended`/`--extended` opts into the full registry. Any other
/// trailing arguments are rejected to preserve the CLI's strict-arg contract.
fn parse_list_scenario_set(
    args: impl Iterator<Item = String>,
) -> Result<BenchmarkScenarioSet, Box<dyn std::error::Error>> {
    let rest: Vec<String> = args.collect();
    match rest.as_slice() {
        [] => Ok(BenchmarkScenarioSet::Core),
        [selector] if selector == "extended" || selector == "--extended" => {
            Ok(BenchmarkScenarioSet::Extended)
        }
        _ => Err(format!("unexpected trailing arguments: {}", rest.join(" ")).into()),
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
    fn test_gym_help_exits_ok() {
        let result = dispatch_operator_cli(vec!["gym".to_string(), "--help".to_string()]);
        assert!(result.is_ok(), "gym --help must exit Ok, got: {result:?}");
    }

    #[test]
    fn test_gym_short_help_exits_ok() {
        let result = dispatch_operator_cli(vec!["gym".to_string(), "-h".to_string()]);
        assert!(result.is_ok(), "gym -h must exit Ok, got: {result:?}");
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

    #[test]
    fn test_gym_list_extended_selector_ok() {
        let result = dispatch_operator_cli(vec![
            "gym".to_string(),
            "list".to_string(),
            "extended".to_string(),
        ]);
        assert!(
            result.is_ok(),
            "gym list extended must succeed, got: {result:?}"
        );
    }

    #[test]
    fn test_gym_list_extended_flag_ok() {
        let result = dispatch_operator_cli(vec![
            "gym".to_string(),
            "list".to_string(),
            "--extended".to_string(),
        ]);
        assert!(
            result.is_ok(),
            "gym list --extended must succeed, got: {result:?}"
        );
    }

    #[test]
    fn test_gym_list_default_ok() {
        let result = dispatch_operator_cli(vec!["gym".to_string(), "list".to_string()]);
        assert!(result.is_ok(), "gym list must succeed, got: {result:?}");
    }

    #[test]
    fn test_gym_list_unknown_selector_rejected() {
        let result = dispatch_operator_cli(vec![
            "gym".to_string(),
            "list".to_string(),
            "core".to_string(),
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
