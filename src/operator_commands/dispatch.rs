use std::path::{Path, PathBuf};

use super::command_context::CommandContext;
use super::{
    run_bootstrap_probe, run_coin_gym_verify_probe, run_engineer_loop_probe,
    run_engineer_read_probe, run_goal_curation_probe, run_gym_compare, run_gym_list,
    run_gym_scenario, run_gym_suite, run_handoff_probe, run_improvement_curation_probe,
    run_improvement_curation_read_probe, run_meeting_probe, run_meeting_read_probe,
    run_review_probe, run_review_read_probe, run_signal_notify_probe, run_terminal_probe,
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
        "coin-gym-verify" => {
            reject_extra_args(args)?;
            run_coin_gym_verify_probe()?;
        }
        "signal-notify" => {
            let message = next_required(&mut args, "message")?;
            reject_extra_args(args)?;
            run_signal_notify_probe(&message)?;
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
        "coin-gym-verify" => {
            run_coin_gym_verify_probe()?;
        }
        other => return Err(format!("unsupported probe command '{other}'").into()),
    }

    Ok(())
}

#[cfg(test)]
mod doc_parity_tests {
    //! Guards against operator-surface documentation drift.
    //!
    //! Every `simard_operator_probe` subcommand wired in
    //! [`dispatch_operator_probe`] is a shipped compatibility surface, so the
    //! runtime-contracts reference must document it. This test caught (and now
    //! prevents regressing) the gap where `handoff-roundtrip` was fully wired
    //! but absent from the doc.

    const DISPATCH_SRC: &str = include_str!("dispatch.rs");
    const RUNTIME_CONTRACTS_DOC: &str = include_str!("../../docs/reference/runtime-contracts.md");

    /// Extract the command literals handled by the positional
    /// `dispatch_operator_probe` match arms. Scoped to that function so the gym
    /// dispatcher's `list`/`run`/`compare`/`run-suite` arms (a separate surface,
    /// documented under the gym section) are not mistaken for probe commands.
    fn operator_probe_commands() -> Vec<String> {
        let start = DISPATCH_SRC
            .find("pub fn dispatch_operator_probe")
            .expect("dispatch_operator_probe present");
        let end = DISPATCH_SRC[start..]
            .find("pub fn dispatch_legacy_gym_cli")
            .map(|offset| start + offset)
            .expect("legacy gym dispatcher present");
        let body = &DISPATCH_SRC[start..end];

        let mut commands = Vec::new();
        for line in body.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix('"')
                && let Some(close) = rest.find('"')
                && rest[close + 1..].trim_start().starts_with("=>")
            {
                commands.push(rest[..close].to_string());
            }
        }
        commands.sort();
        commands.dedup();
        commands
    }

    #[test]
    fn every_operator_probe_command_is_documented() {
        let commands = operator_probe_commands();
        assert!(
            !commands.is_empty(),
            "expected to parse at least one probe command from dispatch_operator_probe"
        );
        for command in commands {
            let needle = format!("simard_operator_probe {command}");
            assert!(
                RUNTIME_CONTRACTS_DOC.contains(&needle),
                "operator-probe command `{command}` is wired in dispatch.rs but is not documented \
                 as `{needle}` in docs/reference/runtime-contracts.md"
            );
        }
    }
}
