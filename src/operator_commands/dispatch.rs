use std::path::{Path, PathBuf};

use super::command_context::CommandContext;
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

/// Dispatch an operator probe using a [`CommandContext`].
///
/// This is the context-based equivalent of [`dispatch_operator_probe`].
/// The positional variant remains for backward compatibility with the CLI
/// arg-parsing layer; new callers should prefer this function.
pub fn dispatch_probe_with_context(
    command: &str,
    ctx: &CommandContext,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        "meeting-run" => {
            let base_type = ctx.require_base_type()?;
            let objective = ctx.require_objective()?;
            run_meeting_probe(
                base_type,
                &ctx.topology,
                objective,
                ctx.state_root_override.clone(),
            )?;
        }
        "meeting-read" => {
            let base_type = ctx.require_base_type()?;
            run_meeting_read_probe(base_type, &ctx.topology, ctx.state_root_override.clone())?;
        }
        "goal-curation-run" => {
            let base_type = ctx.require_base_type()?;
            let objective = ctx.require_objective()?;
            run_goal_curation_probe(
                base_type,
                &ctx.topology,
                objective,
                ctx.state_root_override.clone(),
            )?;
        }
        "terminal-run" => {
            let objective = ctx.require_objective()?;
            run_terminal_probe(&ctx.topology, objective, ctx.state_root_override.clone())?;
        }
        "terminal-run-file" => {
            let objective_path = ctx.require_workspace_root()?;
            run_terminal_probe_from_file(
                &ctx.topology,
                objective_path,
                ctx.state_root_override.clone(),
            )?;
        }
        "terminal-read" => {
            run_terminal_read_probe(&ctx.topology, ctx.state_root_override.clone())?;
        }
        "terminal-recipe-list" => {
            run_terminal_recipe_list_probe()?;
        }
        "terminal-recipe-show" => {
            let objective = ctx.require_objective()?;
            run_terminal_recipe_show_probe(objective)?;
        }
        "terminal-recipe-run" => {
            let recipe_name = ctx.require_objective()?;
            run_terminal_recipe_probe(&ctx.topology, recipe_name, ctx.state_root_override.clone())?;
        }
        "engineer-loop-run" => {
            let workspace_root = ctx.require_workspace_root()?;
            let objective = ctx.require_objective()?;
            run_engineer_loop_probe(
                &ctx.topology,
                workspace_root,
                objective,
                ctx.state_root_override.clone(),
            )?;
        }
        "engineer-read" => {
            run_engineer_read_probe(&ctx.topology, ctx.state_root_override.clone())?;
        }
        "review-run" => {
            let base_type = ctx.require_base_type()?;
            let objective = ctx.require_objective()?;
            run_review_probe(
                base_type,
                &ctx.topology,
                objective,
                ctx.state_root_override.clone(),
            )?;
        }
        "review-read" => {
            let base_type = ctx.require_base_type()?;
            run_review_read_probe(base_type, &ctx.topology, ctx.state_root_override.clone())?;
        }
        "improvement-curation-run" => {
            let base_type = ctx.require_base_type()?;
            let objective = ctx.require_objective()?;
            run_improvement_curation_probe(
                base_type,
                &ctx.topology,
                objective,
                ctx.state_root_override.clone(),
            )?;
        }
        "improvement-curation-read" => {
            let base_type = ctx.require_base_type()?;
            run_improvement_curation_read_probe(
                base_type,
                &ctx.topology,
                ctx.state_root_override.clone(),
            )?;
        }
        "bootstrap-run" => {
            let identity = ctx
                .identity
                .as_deref()
                .ok_or("identity is required for bootstrap-run")?;
            let base_type = ctx.require_base_type()?;
            let objective = ctx.require_objective()?;
            run_bootstrap_probe(
                identity,
                base_type,
                &ctx.topology,
                objective,
                ctx.state_root_override.clone(),
            )?;
        }
        "handoff-roundtrip" => {
            let identity = ctx
                .identity
                .as_deref()
                .ok_or("identity is required for handoff-roundtrip")?;
            let base_type = ctx.require_base_type()?;
            let objective = ctx.require_objective()?;
            run_handoff_probe(identity, base_type, &ctx.topology, objective)?;
        }
        other => return Err(format!("unsupported probe command '{other}'").into()),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
