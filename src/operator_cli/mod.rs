mod args;
mod curation;
mod dashboard;
mod decisions;
mod engineer;
mod goal;
mod gym;
mod meeting;
mod merge;
mod ooda;
mod review;
mod safe_update;
mod worktree_gc;

use std::path::PathBuf;

use crate::agent_roles::AgentRole;
use crate::agent_supervisor::{SubordinateConfig, spawn_subordinate};
use crate::cmd_cleanup::handle_cleanup;
use crate::cmd_ensure_deps::handle_ensure_deps;
use crate::cmd_install::handle_install;
use crate::cmd_self_update::{handle_self_test, handle_self_update};
use crate::operator_commands::run_bootstrap_probe;
use crate::self_relaunch::{
    RelaunchConfig, all_gates_passed, build_canary, default_gates, handover, verify_canary,
};

use args::{next_optional_path, next_required, reject_extra_args};

/// Check whether an args iterator starts with a help flag, consuming it if so.
/// Returns `Some(help_text)` when `--help` / `-h` is the only argument,
/// `None` otherwise (leaving the iterator at the original position for
/// commands that don't peek).
fn check_help_flag<I: Iterator<Item = String>>(
    args: &mut std::iter::Peekable<I>,
    help_text: &'static str,
) -> Option<&'static str> {
    if let Some(first) = args.peek()
        && matches!(first.as_str(), "--help" | "-h" | "help")
    {
        let _ = args.next(); // consume the flag
        return Some(help_text);
    }
    None
}

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
  meeting read <base-type> <topology> <state-root>
  meeting repl [topic]
  meeting resume             — resume an interrupted meeting from the last WIP checkpoint
  meeting resume --discard   — discard the saved WIP checkpoint without resuming
  goal list                — print active + backlog snapshot to stdout
  goal unblock <goal-id>   — operator escape hatch: clear any Blocked
                             status (unconditional) and restore to
                             NotStarted (issue #1911)
  goal unblock-all         — bulk-clear ONLY goals stuck on the
                             deterministic brain-failure safeguard
                             marker; operator-, scope-, dependency-, and
                             subordinate-blocked goals are untouched
  goal remove <id>...      — drop one or more goal ids from the active
                             + backlog board. Variadic, idempotent
                             (unknown ids = no-op). Defeats the PR #1926
                             merge-on-write resurrection failure mode
                             (issues #1923 / #1925).
  goal cleanup --placeholders
                          — sweep every goal whose description is
                            exactly 'Goal <id>' (the test-fixture
                            placeholder pattern). Defence-in-depth
                            cleanup for issues #1923 / #1925.
  goal-curation run <base-type> <topology> <objective> [state-root]
  goal-curation read <base-type> <topology> [state-root]
                         — read goals from $SIMARD_STATE_ROOT (or
                           $HOME/.simard/state) by default, matching the
                           meeting greeting banner; pass [state-root] to
                           inspect a probe-isolated sandbox instead
  improvement-curation run <base-type> <topology> <objective> [state-root]
  improvement-curation read <base-type> <topology> <state-root>
  gym list
  gym run <scenario-id>
  gym compare <scenario-id>
  gym run-suite <suite-id>
  ooda run [--cycles=N] [--no-auto-reload] [state-root]
  dashboard serve [--port=8080]
  spawn <agent-name> <goal> <worktree-path> [--depth=N]
  merge-pr <pr-number>   — squash-merge PR in rysweet/Simard if it is merge-ready
  worktree-gc [--apply] [--idle-days=N] [--root=PATH ...] [--parent-repo=PATH]
                         — prune merged/stale engineer worktrees (dry-run by default)
  handover [--canary-dir=PATH] [--manifest-dir=PATH]
  update
  self-test
  safe-update            — drain → snapshot → pre-test → swap → exec
  rollback               — restore the latest backup over the install path
  rollback-watchdog [--once] [--interval=SECS] [--max-iterations=N]
                         — watch upgrade-status.json; rollback on validate_timeout
  ensure-deps
  cleanup
  act-on-decisions
  install
  version           — print the compiled-in semver and exit
  --version, -V     — alias for `version`

Operator utilities:
  review run <base-type> <topology> <objective> [state-root]
  review read <base-type> <topology> <state-root>
  bootstrap run <identity> <base-type> <topology> <objective> [state-root]

Compatibility binaries remain available: simard_operator_probe, simard-gym
";

const SPAWN_HELP: &str = "\
Simard spawn subcommand

Usage: simard spawn <agent-name> <goal> <worktree-path> [--depth=N]

Spawn a subordinate engineer agent in the given worktree.
";

const BOOTSTRAP_HELP: &str = "\
Simard bootstrap subcommand

Usage: simard bootstrap run <identity> <base-type> <topology> <objective> [state-root]

Run the bootstrap probe with the given identity.
";

const HANDOVER_HELP: &str = "\
Simard handover subcommand

Usage: simard handover [--canary-dir=PATH] [--manifest-dir=PATH]

Build a canary binary, run gate checks, and exec into the canary on success.
";

const UPDATE_HELP: &str = "\
Simard update subcommand

Usage: simard update

Download and install the latest Simard release.
";

const SELF_TEST_HELP: &str = "\
Simard self-test subcommand

Usage: simard self-test

Run the built-in self-test suite.
";

const ENSURE_DEPS_HELP: &str = "\
Simard ensure-deps subcommand

Usage: simard ensure-deps

Check and install required runtime dependencies.
";

const CLEANUP_HELP: &str = "\
Simard cleanup subcommand

Usage: simard cleanup

Remove stale state files and temporary artifacts.
";

const INSTALL_HELP: &str = "\
Simard install subcommand

Usage: simard install

Install or reinstall the Simard binary to the standard path.
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

    if matches!(command.as_str(), "--version" | "-V" | "version") {
        reject_extra_args(args)?;
        println!("simard {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    match command.as_str() {
        "engineer" => engineer::dispatch_engineer_command(args),
        "meeting" => meeting::dispatch_meeting_command(args),
        "goal" => goal::dispatch_goal_command(args),
        "goal-curation" => curation::dispatch_goal_curation_command(args),
        "improvement-curation" => curation::dispatch_improvement_curation_command(args),
        "review" => review::dispatch_review_command(args),
        "gym" => gym::dispatch_gym_command(args),
        "ooda" => ooda::dispatch_ooda_command(args),
        "dashboard" => dashboard::dispatch_dashboard_command(args),
        "spawn" => dispatch_spawn_command(args),
        "merge-pr" => merge::dispatch_merge_pr_command(args),
        "worktree-gc" => worktree_gc::dispatch_worktree_gc_command(args),
        "handover" => dispatch_handover_command(args),
        "bootstrap" => dispatch_bootstrap_command(args),
        "act-on-decisions" => {
            let mut args = args.peekable();
            if let Some(help) = check_help_flag(&mut args, decisions::ACT_ON_DECISIONS_HELP) {
                print!("{help}");
                return Ok(());
            }
            reject_extra_args(args)?;
            decisions::dispatch_act_on_decisions()
        }
        "update" => {
            let mut args = args.peekable();
            if let Some(help) = check_help_flag(&mut args, UPDATE_HELP) {
                print!("{help}");
                return Ok(());
            }
            reject_extra_args(args)?;
            handle_self_update()
        }
        "self-test" => {
            let mut args = args.peekable();
            if let Some(help) = check_help_flag(&mut args, SELF_TEST_HELP) {
                print!("{help}");
                return Ok(());
            }
            reject_extra_args(args)?;
            handle_self_test()
        }
        "safe-update" => {
            let mut args = args.peekable();
            if let Some(help) = check_help_flag(&mut args, safe_update::SAFE_UPDATE_HELP) {
                print!("{help}");
                return Ok(());
            }
            reject_extra_args(args)?;
            safe_update::handle_safe_update()
        }
        "rollback" => {
            let mut args = args.peekable();
            if let Some(help) = check_help_flag(&mut args, safe_update::ROLLBACK_HELP) {
                print!("{help}");
                return Ok(());
            }
            reject_extra_args(args)?;
            safe_update::handle_rollback()
        }
        "rollback-watchdog" => {
            let mut args = args.peekable();
            if let Some(help) = check_help_flag(&mut args, safe_update::ROLLBACK_WATCHDOG_HELP) {
                print!("{help}");
                return Ok(());
            }
            safe_update::handle_rollback_watchdog(args)
        }
        "ensure-deps" => {
            let mut args = args.peekable();
            if let Some(help) = check_help_flag(&mut args, ENSURE_DEPS_HELP) {
                print!("{help}");
                return Ok(());
            }
            reject_extra_args(args)?;
            handle_ensure_deps()
        }
        "cleanup" => {
            let mut args = args.peekable();
            if let Some(help) = check_help_flag(&mut args, CLEANUP_HELP) {
                print!("{help}");
                return Ok(());
            }
            reject_extra_args(args)?;
            handle_cleanup()
        }
        "install" => {
            let mut args = args.peekable();
            if let Some(help) = check_help_flag(&mut args, INSTALL_HELP) {
                print!("{help}");
                return Ok(());
            }
            reject_extra_args(args)?;
            handle_install()
        }
        other => Err(format!("unsupported command '{other}'").into()),
    }
}

pub fn operator_cli_usage() -> &'static str {
    "usage: simard <engineer|meeting|goal-curation|improvement-curation|gym|ooda|spawn|merge-pr|worktree-gc|handover|update|safe-update|rollback|rollback-watchdog|install|review|bootstrap> ..."
}

pub fn operator_cli_help() -> &'static str {
    OPERATOR_CLI_HELP
}

fn dispatch_bootstrap_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "bootstrap command")?;
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{BOOTSTRAP_HELP}");
            Ok(())
        }
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
    if matches!(agent_name.as_str(), "--help" | "-h" | "help") {
        print!("{SPAWN_HELP}");
        return Ok(());
    }
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
        if matches!(arg.as_str(), "--help" | "-h" | "help") {
            print!("{HANDOVER_HELP}");
            return Ok(());
        } else if let Some(v) = arg.strip_prefix("--canary-dir=") {
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

#[cfg(test)]
mod tests_goal;

#[cfg(test)]
mod tests_goal_remove;
