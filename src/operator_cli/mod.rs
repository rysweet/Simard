mod args;
mod curation;
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

const OPERATOR_CLI_HELP: &str = "\
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
  ooda run [--cycles=N] [state-root]
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
mod tests {
    use super::*;

    #[test]
    fn test_help_text_contains_update_command() {
        let help = operator_cli_help();
        assert!(
            help.contains("update"),
            "help should mention 'update' command"
        );
    }

    #[test]
    fn test_help_text_contains_install_command() {
        let help = operator_cli_help();
        assert!(
            help.contains("install"),
            "help should mention 'install' command"
        );
    }

    #[test]
    fn test_usage_mentions_update_and_install() {
        let usage = operator_cli_usage();
        assert!(usage.contains("update"));
        assert!(usage.contains("install"));
    }

    #[test]
    fn test_unknown_command_returns_error() {
        let result = dispatch_operator_cli(vec!["nonexistent-cmd".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command")
        );
    }

    #[test]
    fn test_update_rejects_extra_args() {
        let result = dispatch_operator_cli(vec!["update".to_string(), "extra".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected trailing arguments")
        );
    }

    #[test]
    fn test_install_rejects_extra_args() {
        let result = dispatch_operator_cli(vec!["install".to_string(), "extra".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected trailing arguments")
        );
    }

    #[test]
    fn test_help_flag_does_not_error() {
        let result = dispatch_operator_cli(vec!["--help".to_string()]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_no_args_shows_help() {
        let result = dispatch_operator_cli(std::iter::empty::<String>());
        assert!(result.is_ok());
    }

    #[test]
    fn test_help_text_contains_all_top_level_commands() {
        let help = operator_cli_help();
        for cmd in &[
            "engineer",
            "meeting",
            "goal-curation",
            "improvement-curation",
            "gym",
            "ooda",
            "spawn",
            "handover",
            "update",
            "self-test",
            "act-on-decisions",
            "install",
            "review",
            "bootstrap",
        ] {
            assert!(help.contains(cmd), "help should mention '{cmd}' command");
        }
    }

    #[test]
    fn test_help_flag_variants() {
        for flag in &["-h", "--help", "help"] {
            let result = dispatch_operator_cli(vec![flag.to_string()]);
            assert!(result.is_ok(), "flag '{flag}' should not error");
        }
    }

    // ── spawn dispatch ──

    #[test]
    fn test_spawn_missing_agent_name() {
        let result = dispatch_operator_cli(vec!["spawn".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected agent name")
        );
    }

    #[test]
    fn test_spawn_missing_goal() {
        let result = dispatch_operator_cli(vec!["spawn".to_string(), "agent1".to_string()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expected goal"));
    }

    #[test]
    fn test_spawn_missing_worktree_path() {
        let result = dispatch_operator_cli(vec![
            "spawn".to_string(),
            "agent1".to_string(),
            "do stuff".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected worktree path")
        );
    }

    #[test]
    fn test_spawn_invalid_depth() {
        let result = dispatch_operator_cli(vec![
            "spawn".to_string(),
            "agent1".to_string(),
            "goal".to_string(),
            "/worktree".to_string(),
            "--depth=abc".to_string(),
        ]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid --depth"));
    }

    #[test]
    fn test_spawn_unexpected_flag() {
        let result = dispatch_operator_cli(vec![
            "spawn".to_string(),
            "agent1".to_string(),
            "goal".to_string(),
            "/worktree".to_string(),
            "--unknown=x".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected argument")
        );
    }

    // ── bootstrap dispatch ──

    #[test]
    fn test_bootstrap_missing_subcommand() {
        let result = dispatch_operator_cli(vec!["bootstrap".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected bootstrap command")
        );
    }

    #[test]
    fn test_bootstrap_unknown_subcommand() {
        let result = dispatch_operator_cli(vec!["bootstrap".to_string(), "unknown".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command 'bootstrap unknown'")
        );
    }

    #[test]
    fn test_bootstrap_run_missing_identity() {
        let result = dispatch_operator_cli(vec!["bootstrap".to_string(), "run".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected identity")
        );
    }

    #[test]
    fn test_bootstrap_run_missing_base_type() {
        let result = dispatch_operator_cli(vec![
            "bootstrap".to_string(),
            "run".to_string(),
            "identity".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected base type")
        );
    }

    #[test]
    fn test_bootstrap_run_missing_topology() {
        let result = dispatch_operator_cli(vec![
            "bootstrap".to_string(),
            "run".to_string(),
            "identity".to_string(),
            "base-type".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected topology")
        );
    }

    #[test]
    fn test_bootstrap_run_missing_objective() {
        let result = dispatch_operator_cli(vec![
            "bootstrap".to_string(),
            "run".to_string(),
            "identity".to_string(),
            "base-type".to_string(),
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

    // ── handover dispatch ──

    #[test]
    fn test_handover_rejects_unexpected_arg() {
        let result =
            dispatch_operator_cli(vec!["handover".to_string(), "--bad-flag=x".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected argument")
        );
    }

    // ── self-test rejects extra args ──

    #[test]
    fn test_self_test_rejects_extra_args() {
        let result = dispatch_operator_cli(vec!["self-test".to_string(), "extra".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected trailing arguments")
        );
    }

    // ── OPERATOR_CLI_HELP constant ──

    #[test]
    fn test_operator_cli_help_starts_with_simard() {
        assert!(OPERATOR_CLI_HELP.starts_with("Simard"));
    }

    #[test]
    fn test_operator_cli_usage_is_not_empty() {
        assert!(!operator_cli_usage().is_empty());
    }

    #[test]
    fn test_help_text_contains_newlines() {
        let help = operator_cli_help();
        assert!(help.contains('\n'));
    }

    #[test]
    fn test_usage_starts_with_usage() {
        let usage = operator_cli_usage();
        assert!(usage.starts_with("usage:"));
    }

    #[test]
    fn test_help_mentions_product_modes() {
        let help = operator_cli_help();
        assert!(help.contains("Product modes:"));
    }

    #[test]
    fn test_help_mentions_operator_utilities() {
        let help = operator_cli_help();
        assert!(help.contains("Operator utilities:"));
    }

    #[test]
    fn test_help_mentions_compatibility() {
        let help = operator_cli_help();
        assert!(help.contains("Compatibility"));
    }
}
