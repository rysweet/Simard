mod args;
mod curation;
mod dashboard;
mod decisions;
mod engineer;
mod gym;
mod meeting;
mod ooda;
mod review;

use std::path::PathBuf;

use crate::agent_roles::AgentRole;
use crate::agent_supervisor::{SubordinateConfig, spawn_subordinate};
use crate::cmd_install::handle_install;
use crate::cmd_self_update::{handle_self_test, handle_self_update};
use crate::operator_commands::run_bootstrap_probe;
use crate::self_relaunch::{
    RelaunchConfig, all_gates_passed, build_canary, default_gates, handover, verify_canary,
};

use args::{next_optional_path, next_required, reject_extra_args};

pub(super) const OPERATOR_CLI_HELP: &str = "\
Simard unified operator CLI

Product modes:
  engineer run <topology> <workspace-root> <objective> [state-root]
  engineer terminal <topology> <objective> [state-root]
  engineer terminal-file <topology> <objective-file> [state-root]
  engineer terminal-recipe-list
  engineer terminal-recipe-show <recipe-name>
  engineer terminal-recipe <topology> <recipe-name> [state-root]
  engineer copilot-submit <topology> [state-root] [--json]
  engineer terminal-read <topology> [state-root]
  engineer read <topology> [state-root]
  meeting run <base-type> <topology> <objective> [state-root]
  meeting read <base-type> <topology> [state-root]
  meeting repl [topic]
  goal-curation run <base-type> <topology> <objective> [state-root]
  goal-curation read <base-type> <topology> [state-root]
  improvement-curation run <base-type> <topology> <objective> [state-root]
  improvement-curation read <base-type> <topology> [state-root]
  gym list
  gym run <scenario-id>
  gym compare <scenario-id>
  gym run-suite <suite-id>
  ooda run [--cycles=N] [--no-auto-reload] [state-root]
  dashboard serve [--port=8080]
  spawn <agent-name> <goal> <worktree-path> [--depth=N]
  handover [--canary-dir=PATH] [--manifest-dir=PATH]
  update
  self-test
  act-on-decisions
  install

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
        "engineer" => engineer::dispatch_engineer_command(args),
        "meeting" => meeting::dispatch_meeting_command(args),
        "goal-curation" => curation::dispatch_goal_curation_command(args),
        "improvement-curation" => curation::dispatch_improvement_curation_command(args),
        "review" => review::dispatch_review_command(args),
        "gym" => gym::dispatch_gym_command(args),
        "ooda" => ooda::dispatch_ooda_command(args),
        "dashboard" => dashboard::dispatch_dashboard_command(args),
        "spawn" => dispatch_spawn_command(args),
        "handover" => dispatch_handover_command(args),
        "bootstrap" => dispatch_bootstrap_command(args),
        "act-on-decisions" => {
            reject_extra_args(args)?;
            decisions::dispatch_act_on_decisions()
        }
        "update" => {
            reject_extra_args(args)?;
            handle_self_update()
        }
        "self-test" => {
            reject_extra_args(args)?;
            handle_self_test()
        }
        "install" => {
            reject_extra_args(args)?;
            handle_install()
        }
        other => Err(format!("unsupported command '{other}'").into()),
    }
}

pub fn operator_cli_usage() -> &'static str {
    "usage: simard <engineer|meeting|goal-curation|improvement-curation|gym|ooda|spawn|handover|update|install|review|bootstrap> ..."
}

pub fn operator_cli_help() -> &'static str {
    OPERATOR_CLI_HELP
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

fn dispatch_spawn_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let agent_name = next_required(&mut args, "agent name")?;
    let goal = next_required(&mut args, "goal")?;
    let worktree_path = next_required(&mut args, "worktree path")?;

    let mut depth: u32 = 0;
    for arg in args {
        if let Some(n) = arg.strip_prefix("--depth=") {
            depth = n
                .parse()
                .map_err(|_| format!("invalid --depth value: {n}"))?;
        } else {
            return Err(format!("unexpected argument: {arg}").into());
        }
    }

    let config = SubordinateConfig {
        agent_name: agent_name.clone(),
        goal: goal.clone(),
        role: AgentRole::Engineer,
        worktree_path: PathBuf::from(&worktree_path),
        current_depth: depth,
    };

    let handle = spawn_subordinate(&config)?;
    println!(
        "spawned subordinate '{}' with pid {} in {}",
        handle.agent_name, handle.pid, worktree_path
    );
    Ok(())
}

fn dispatch_handover_command(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut canary_dir: Option<PathBuf> = None;
    let mut manifest_dir: Option<PathBuf> = None;

    for arg in args {
        if let Some(v) = arg.strip_prefix("--canary-dir=") {
            canary_dir = Some(PathBuf::from(v));
        } else if let Some(v) = arg.strip_prefix("--manifest-dir=") {
            manifest_dir = Some(PathBuf::from(v));
        } else {
            return Err(format!("unexpected argument: {arg}").into());
        }
    }

    let mut config = RelaunchConfig::default();
    if let Some(dir) = canary_dir {
        config.canary_target_dir = dir;
    }
    if let Some(dir) = manifest_dir {
        config.manifest_dir = dir;
    }

    eprintln!("building canary binary...");
    let canary = build_canary(&config)?;
    eprintln!("canary built at {}", canary.display());

    eprintln!("running gate checks...");
    let gates = default_gates();
    let results = verify_canary(&canary, &gates, &config)?;
    for r in &results {
        eprintln!("  {r}");
    }

    if !all_gates_passed(&results) {
        return Err("canary verification failed — aborting handover".into());
    }

    eprintln!("all gates passed — handing over to canary");
    let pid = std::process::id();
    handover(pid, &canary)?;
    // handover does not return on success (exec replaces process)
    Ok(())
}

#[cfg(test)]
mod tests_mod;
