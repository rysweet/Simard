use std::path::PathBuf;

use chrono::Local;

use crate::agent_roles::AgentRole;
use crate::agent_supervisor::{SubordinateConfig, spawn_subordinate};
use crate::cmd_install::handle_install;
use crate::cmd_self_update::handle_self_update;
use crate::operator_commands::{
    run_bootstrap_probe, run_copilot_submit_probe, run_engineer_loop_probe,
    run_engineer_read_probe, run_goal_curation_probe, run_goal_curation_read_probe,
    run_gym_compare, run_gym_list, run_gym_scenario, run_gym_suite, run_improvement_curation_probe,
    run_improvement_curation_read_probe, run_meeting_probe, run_meeting_read_probe,
    run_review_probe, run_review_read_probe, run_terminal_probe, run_terminal_probe_from_file,
    run_terminal_read_probe, run_terminal_recipe_list_probe, run_terminal_recipe_probe,
    run_terminal_recipe_show_probe,
};
use crate::operator_commands_meeting::run_meeting_repl_command;
use crate::operator_commands_ooda::run_ooda_daemon;
use crate::self_relaunch::{
    RelaunchConfig, all_gates_passed, build_canary, default_gates, handover, verify_canary,
};

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
        "engineer" => dispatch_engineer_command(args),
        "meeting" => dispatch_meeting_command(args),
        "goal-curation" => dispatch_goal_curation_command(args),
        "improvement-curation" => dispatch_improvement_curation_command(args),
        "review" => dispatch_review_command(args),
        "gym" => dispatch_gym_command(args),
        "ooda" => dispatch_ooda_command(args),
        "spawn" => dispatch_spawn_command(args),
        "handover" => dispatch_handover_command(args),
        "bootstrap" => dispatch_bootstrap_command(args),
        "act-on-decisions" => {
            reject_extra_args(args)?;
            dispatch_act_on_decisions()
        }
        "update" => {
            reject_extra_args(args)?;
            handle_self_update()
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

fn dispatch_meeting_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = args.next().unwrap_or_else(|| "repl".to_string());
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
        "repl" | "begin" | "start" => {
            let topic = args
                .next()
                .unwrap_or_else(|| Local::now().format("%Y-%m-%d:%H:%M").to_string());
            reject_extra_args(args)?;
            run_meeting_repl_command(&topic)
        }
        // Any other word is treated as a topic for a meeting repl
        topic => {
            let rest: Vec<String> = args.collect();
            let full_topic = if rest.is_empty() {
                topic.to_string()
            } else {
                format!("{topic} {}", rest.join(" "))
            };
            run_meeting_repl_command(&full_topic)
        }
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

fn dispatch_ooda_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "ooda command")?;
    match subcommand.as_str() {
        "run" => {
            let mut max_cycles: u32 = 0; // 0 = infinite
            let mut state_root: Option<PathBuf> = None;

            for arg in args {
                if let Some(n) = arg.strip_prefix("--cycles=") {
                    max_cycles = n
                        .parse()
                        .map_err(|_| format!("invalid --cycles value: {n}"))?;
                } else if state_root.is_none() {
                    state_root = Some(PathBuf::from(arg));
                } else {
                    return Err(format!("unexpected argument: {arg}").into());
                }
            }

            run_ooda_daemon(max_cycles, state_root)
        }
        other => Err(format!("unsupported command 'ooda {other}'").into()),
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

fn parse_state_root_and_json(
    trailing: Vec<String>,
) -> Result<(Option<PathBuf>, bool), Box<dyn std::error::Error>> {
    match trailing.as_slice() {
        [] => Ok((None, false)),
        [flag] if flag == "--json" => Ok((None, true)),
        [state_root] => Ok((Some(PathBuf::from(state_root)), false)),
        [state_root, flag] if flag == "--json" => Ok((Some(PathBuf::from(state_root)), true)),
        _ => Err(format!("unexpected trailing arguments: {}", trailing.join(" ")).into()),
    }
}

/// Read the latest meeting handoff and create GitHub issues for each
/// decision and action item via `gh issue create`.
fn dispatch_act_on_decisions() -> Result<(), Box<dyn std::error::Error>> {
    use crate::meeting_facilitator::{
        default_handoff_dir, load_meeting_handoff, mark_meeting_handoff_processed,
    };

    let dir = default_handoff_dir();
    let handoff = load_meeting_handoff(&dir)?;

    let Some(handoff) = handoff else {
        println!("No meeting handoff found in {}", dir.display());
        return Ok(());
    };

    if handoff.processed {
        println!(
            "Meeting handoff already processed (topic: {})",
            handoff.topic
        );
        return Ok(());
    }

    println!(
        "Processing meeting handoff: {} (closed {})",
        handoff.topic, handoff.closed_at
    );

    let mut created = 0u32;

    for decision in &handoff.decisions {
        let title = format!("Decision: {}", decision.description);
        let body = format!(
            "**Rationale:** {}\n**Participants:** {}\n\n_From meeting: {}_",
            decision.rationale,
            if decision.participants.is_empty() {
                "(none)".to_string()
            } else {
                decision.participants.join(", ")
            },
            handoff.topic,
        );
        match std::process::Command::new("gh")
            .args(["issue", "create", "--title", &title, "--body", &body])
            .output()
        {
            Ok(output) if output.status.success() => {
                let url = String::from_utf8_lossy(&output.stdout);
                println!(
                    "  Created issue for decision: {} → {}",
                    decision.description,
                    url.trim()
                );
                created += 1;
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!(
                    "  [warn] gh issue create failed for '{}': {}",
                    decision.description,
                    stderr.trim()
                );
            }
            Err(e) => {
                eprintln!("  [warn] Failed to run gh: {e}");
            }
        }
    }

    for item in &handoff.action_items {
        let title = format!("Action: {}", item.description);
        let due = item.due_description.as_deref().unwrap_or("(unspecified)");
        let body = format!(
            "**Owner:** {}\n**Priority:** {}\n**Due:** {}\n\n_From meeting: {}_",
            item.owner, item.priority, due, handoff.topic,
        );
        match std::process::Command::new("gh")
            .args(["issue", "create", "--title", &title, "--body", &body])
            .output()
        {
            Ok(output) if output.status.success() => {
                let url = String::from_utf8_lossy(&output.stdout);
                println!(
                    "  Created issue for action: {} → {}",
                    item.description,
                    url.trim()
                );
                created += 1;
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!(
                    "  [warn] gh issue create failed for '{}': {}",
                    item.description,
                    stderr.trim()
                );
            }
            Err(e) => {
                eprintln!("  [warn] Failed to run gh: {e}");
            }
        }
    }

    if !handoff.open_questions.is_empty() {
        println!("\nOpen questions (not filed as issues):");
        for q in &handoff.open_questions {
            println!("  - {q}");
        }
    }

    mark_meeting_handoff_processed(&dir)?;
    println!("\nDone. Created {created} issue(s). Handoff marked as processed.");
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
    fn test_parse_state_root_and_json_empty() {
        let (root, json) = parse_state_root_and_json(vec![]).unwrap();
        assert!(root.is_none());
        assert!(!json);
    }

    #[test]
    fn test_parse_state_root_and_json_flag_only() {
        let (root, json) = parse_state_root_and_json(vec!["--json".to_string()]).unwrap();
        assert!(root.is_none());
        assert!(json);
    }

    #[test]
    fn test_parse_state_root_and_json_path_and_flag() {
        let (root, json) =
            parse_state_root_and_json(vec!["/tmp/state".to_string(), "--json".to_string()])
                .unwrap();
        assert_eq!(root.unwrap(), PathBuf::from("/tmp/state"));
        assert!(json);
    }
}
