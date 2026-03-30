use std::path::PathBuf;

use crate::operator_commands::{
    run_bootstrap_probe, run_engineer_loop_probe, run_goal_curation_probe,
    run_goal_curation_read_probe, run_gym_compare, run_gym_list, run_gym_scenario, run_gym_suite,
    run_improvement_curation_probe, run_improvement_curation_read_probe, run_meeting_probe,
    run_meeting_read_probe, run_review_probe, run_review_read_probe, run_terminal_probe,
};

const OPERATOR_CLI_HELP: &str = "\
Simard unified operator CLI

Product modes:
  engineer run <topology> <workspace-root> <objective> [state-root]
  engineer terminal <topology> <objective> [state-root]
  meeting run <base-type> <topology> <objective> [state-root]
  meeting read <base-type> <topology> [state-root]
  goal-curation run <base-type> <topology> <objective> [state-root]
  goal-curation read <base-type> <topology> [state-root]
  improvement-curation run <base-type> <topology> <objective> [state-root]
  improvement-curation read <base-type> <topology> [state-root]
  gym list
  gym run <scenario-id>
  gym compare <scenario-id>
  gym run-suite <suite-id>

Operator utilities:
  review run <base-type> <topology> <objective> [state-root]
  review read <base-type> <topology> [state-root]
  bootstrap run <identity> <base-type> <topology> <objective> [state-root]

Compatibility binaries remain available: simard_operator_probe, simard-gym
";

pub fn dispatch_operator_cli<I>(args: I) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        print!("{}", operator_cli_help());
        return Ok(());
    };

    if matches!(command.as_str(), "--help" | "-h" | "help") {
        print!("{}", operator_cli_help());
        return Ok(());
    }

    match command.as_str() {
        "engineer" => dispatch_engineer_command(args),
        "meeting" => dispatch_meeting_command(args),
        "goal-curation" => dispatch_goal_curation_command(args),
        "improvement-curation" => dispatch_improvement_curation_command(args),
        "review" => dispatch_review_command(args),
        "gym" => dispatch_gym_command(args),
        "bootstrap" => dispatch_bootstrap_command(args),
        other => Err(format!("unsupported command '{other}'").into()),
    }
}

pub fn operator_cli_usage() -> &'static str {
    "usage: simard <engineer|meeting|goal-curation|improvement-curation|gym|review|bootstrap> ..."
}

pub fn operator_cli_help() -> &'static str {
    OPERATOR_CLI_HELP
}

fn dispatch_engineer_command(
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
        other => Err(format!("unsupported command 'engineer {other}'").into()),
    }
}

fn dispatch_meeting_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "meeting command")?;
    match subcommand.as_str() {
        "run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_meeting_probe(&base_type, &topology, &objective, state_root)
        }
        "read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_meeting_read_probe(&base_type, &topology, state_root)
        }
        other => Err(format!("unsupported command 'meeting {other}'").into()),
    }
}

fn dispatch_goal_curation_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "goal-curation command")?;
    match subcommand.as_str() {
        "run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_goal_curation_probe(&base_type, &topology, &objective, state_root)
        }
        "read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_goal_curation_read_probe(&base_type, &topology, state_root)
        }
        other => Err(format!("unsupported command 'goal-curation {other}'").into()),
    }
}

fn dispatch_improvement_curation_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "improvement-curation command")?;
    match subcommand.as_str() {
        "run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_improvement_curation_probe(&base_type, &topology, &objective, state_root)
        }
        "read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_improvement_curation_read_probe(&base_type, &topology, state_root)
        }
        other => Err(format!("unsupported command 'improvement-curation {other}'").into()),
    }
}

fn dispatch_review_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "review command")?;
    match subcommand.as_str() {
        "run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_review_probe(&base_type, &topology, &objective, state_root)
        }
        "read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_review_read_probe(&base_type, &topology, state_root)
        }
        other => Err(format!("unsupported command 'review {other}'").into()),
    }
}

fn dispatch_gym_command(
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

fn dispatch_bootstrap_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "bootstrap command")?;
    match subcommand.as_str() {
        "run" => {
            let identity = next_required(&mut args, "identity")?;
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_bootstrap_probe(&identity, &base_type, &topology, &objective, state_root)
        }
        other => Err(format!("unsupported command 'bootstrap {other}'").into()),
    }
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
