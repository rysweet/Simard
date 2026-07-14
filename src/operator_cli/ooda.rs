use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::operator_commands_ooda::{DaemonDashboardConfig, run_ooda_daemon};

use super::args::next_required;

pub(super) const OODA_HELP: &str = "\
Simard OODA daemon subcommand

Usage: simard ooda <command> [args]

Commands:
  run [--cycles=N] [--no-auto-reload] [--no-dashboard] [--dashboard-port=PORT] [state-root]
                              Run the OODA loop daemon.
  outcomes get --state-root <PATH> --request-id <ID>
                              Read one authoritative typed terminal.
  outcomes list --state-root <PATH> [--limit <N>]
                              List authoritative typed terminals.
  approvals issue --state-root <PATH> --effect-id <ID>
                              Issue a privileged merge/deploy approval from
                              the configured server principal and signing key.
  fixture run --state-root <PATH> --scenario <spawn-engineer|no-action> --request-id <ID>
                              Run a deterministic typed acceptance cycle
                              (requires SIMARD_TYPED_OODA_FIXTURE=1).
  help, -h, --help            Show this help message and exit.
";

pub(super) fn dispatch_ooda_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "ooda command")?;
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{OODA_HELP}");
            Ok(())
        }
        "run" => {
            let mut max_cycles: u32 = 0; // 0 = infinite
            let mut state_root: Option<PathBuf> = None;
            let mut auto_reload = true;
            let mut dashboard = DaemonDashboardConfig::default();

            for arg in args {
                if let Some(n) = arg.strip_prefix("--cycles=") {
                    max_cycles = n
                        .parse()
                        .map_err(|_| format!("invalid --cycles value: {n}"))?;
                } else if arg == "--no-auto-reload" {
                    auto_reload = false;
                } else if arg == "--no-dashboard" {
                    dashboard.enabled = false;
                } else if let Some(p) = arg.strip_prefix("--dashboard-port=") {
                    dashboard.port = p
                        .parse()
                        .map_err(|_| format!("invalid --dashboard-port value: {p}"))?;
                } else if state_root.is_none() {
                    state_root = Some(PathBuf::from(arg));
                } else {
                    return Err(format!("unexpected argument: {arg}").into());
                }
            }

            run_ooda_daemon(max_cycles, state_root, auto_reload, dashboard)
        }
        "outcomes" => dispatch_outcomes(args),
        "fixture" => dispatch_fixture(args),
        "actor-run" => dispatch_actor_run(args),
        "approvals" => dispatch_approvals(args),
        other => Err(format!("unsupported command 'ooda {other}'").into()),
    }
}

fn dispatch_actor_run(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = parse_named_args(args)?;
    let ledger_path = Path::new(required_named(&parsed, "ledger-path")?);
    let policy_path = Path::new(required_named(&parsed, "policy-path")?);
    let session_id = required_named(&parsed, "session-id")?;
    let cycle_id = required_named(&parsed, "cycle-id")?;
    let goal_id = required_named(&parsed, "goal-id")?;
    let policy = crate::typed_ooda::CapabilityPolicy::from_toml_file(policy_path)?;
    let handler = crate::typed_ooda::CapabilityHandler::open(ledger_path, policy)?;
    let token = std::fs::read_to_string(required_named(&parsed, "auth-token-path")?)?;
    let actor = handler.authenticate_actor_session(token.trim(), session_id, cycle_id, goal_id)?;
    let admission: crate::typed_ooda::AdmissionSnapshot =
        serde_json::from_slice(&std::fs::read(required_named(&parsed, "admission-path")?)?)?;
    let invocation = crate::typed_ooda::GoalSessionInvocation {
        session_id: session_id.to_string(),
        cycle_id: cycle_id.to_string(),
        goal_id: goal_id.to_string(),
        task: crate::typed_ooda::OpaqueBytes::from(std::fs::read(required_named(
            &parsed,
            "task-path",
        )?)?),
        reason: crate::typed_ooda::OpaqueBytes::from(std::fs::read(required_named(
            &parsed,
            "reason-path",
        )?)?),
        observe_output: crate::typed_ooda::OpaqueBytes::from(std::fs::read(required_named(
            &parsed,
            "observe-output-path",
        )?)?),
        orient_output: crate::typed_ooda::OpaqueBytes::from(std::fs::read(required_named(
            &parsed,
            "orient-output-path",
        )?)?),
        decide_output: crate::typed_ooda::OpaqueBytes::from(std::fs::read(required_named(
            &parsed,
            "decide-output-path",
        )?)?),
    };
    let executor = crate::typed_ooda::GoalSessionExecutor::new(
        handler,
        actor,
        admission,
        Box::new(FixtureEffects),
    );
    let actor = crate::typed_ooda::RustyClawdGoalSessionActor::default();
    let execution =
        executor.execute_actor_step(&invocation, |received, tools| actor.run(received, tools))?;
    println!("{}", execution.outcome.outcome_id);
    Ok(())
}

fn dispatch_approvals(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let command = next_required(&mut args, "approvals command")?;
    if command != "issue" {
        return Err(format!("unsupported command 'ooda approvals {command}'").into());
    }
    let parsed = parse_named_args(args)?;
    let state_root = Path::new(required_named(&parsed, "state-root")?);
    let effect_id = required_named(&parsed, "effect-id")?;
    let handler = open_ledger(state_root)?;
    let authority = crate::typed_ooda::ApprovalAuthority::from_environment()?;
    let approval = handler.issue_privileged_approval(&authority, effect_id)?;
    println!("{}", serde_json::to_string(&approval)?);
    Ok(())
}

fn dispatch_outcomes(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let command = next_required(&mut args, "outcomes command")?;
    let parsed = parse_named_args(args)?;
    let state_root = required_named(&parsed, "state-root")?;
    let handler = open_ledger(Path::new(state_root))?;
    match command.as_str() {
        "get" => {
            let request_id = required_named(&parsed, "request-id")?;
            let outcome = handler
                .terminal_for_request(request_id)?
                .ok_or_else(|| format!("typed outcome not found for request {request_id:?}"))?;
            let effect = handler.effect_for_outcome(&outcome.outcome_id)?;
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "outcome": outcome,
                    "effect": effect,
                }))?
            );
            Ok(())
        }
        "list" => {
            let limit = parsed
                .get("limit")
                .map(|value| value.parse::<usize>())
                .transpose()
                .map_err(|_| "--limit must be a positive integer")?
                .unwrap_or(100);
            let outcomes = handler.list_terminals(limit)?;
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({ "outcomes": outcomes }))?
            );
            Ok(())
        }
        other => Err(format!("unsupported command 'ooda outcomes {other}'").into()),
    }
}

fn dispatch_fixture(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("SIMARD_TYPED_OODA_FIXTURE").ok().as_deref() != Some("1") {
        return Err("typed OODA fixture is disabled; set SIMARD_TYPED_OODA_FIXTURE=1 only in an isolated acceptance environment".into());
    }
    let command = next_required(&mut args, "fixture command")?;
    if command != "run" {
        return Err(format!("unsupported command 'ooda fixture {command}'").into());
    }
    let parsed = parse_named_args(args)?;
    let state_root = Path::new(required_named(&parsed, "state-root")?);
    let scenario = required_named(&parsed, "scenario")?;
    let request_id = required_named(&parsed, "request-id")?;
    let handler = open_ledger(state_root)?;
    let session_id = "typed-ooda-fixture";
    let cycle_id = format!("cycle-{request_id}");
    let actor = crate::typed_ooda::AuthenticatedToolContext::new(
        "typed-ooda-fixture",
        session_id,
        [
            crate::typed_ooda::CapabilityGrant::RecordAction(
                crate::typed_ooda::ActionKind::SpawnEngineer,
            ),
            crate::typed_ooda::CapabilityGrant::RecordNoAction,
        ],
    )
    .scoped_to_repository(crate::typed_ooda::RepositoryRef::new("rysweet", "Simard"));
    let executor = crate::typed_ooda::GoalSessionExecutor::new(
        handler,
        actor,
        crate::typed_ooda::AdmissionSnapshot {
            concurrent_engineers: 0,
            disk_used_percent: 0,
            active_claims: BTreeSet::new(),
            policy_revision: "fixture-admission-v1".to_string(),
        },
        Box::new(FixtureEffects),
    );
    let invocation = crate::typed_ooda::GoalSessionInvocation {
        session_id: session_id.to_string(),
        cycle_id,
        goal_id: "fixture-goal".to_string(),
        task: crate::typed_ooda::OpaqueBytes::from(
            b"\nfixture task\0\x1b[31m marker-looking text\n".to_vec(),
        ),
        reason: crate::typed_ooda::OpaqueBytes::from(b"fixture reason\n".to_vec()),
        observe_output: crate::typed_ooda::OpaqueBytes::from(b"fixture observe\n".to_vec()),
        orient_output: crate::typed_ooda::OpaqueBytes::from(b"fixture orient\n".to_vec()),
        decide_output: crate::typed_ooda::OpaqueBytes::from(b"fixture decide\n".to_vec()),
    };
    let execution = executor.execute(&invocation, |received, tools| {
        match scenario {
            "spawn-engineer" => {
                tools.record_action(
                    request_id,
                    crate::typed_ooda::Action::SpawnEngineer(
                        crate::typed_ooda::SpawnEngineerAction {
                            task: received.task.clone(),
                            repository: crate::typed_ooda::RepositoryRef::new("rysweet", "Simard"),
                            base_type: crate::typed_ooda::BaseType::Copilot,
                            requested_permissions: BTreeSet::from(["repo_read".to_string()]),
                            claim_key: "rysweet/Simard:fixture-goal".to_string(),
                        },
                    ),
                    received.decide_output.clone(),
                    Vec::new(),
                )?;
            }
            "no-action" => {
                tools.record_no_action(
                    request_id,
                    received.reason.clone(),
                    received.decide_output.clone(),
                    Vec::new(),
                )?;
            }
            other => {
                return Err(crate::typed_ooda::RecipeProcessError::failed(format!(
                    "unknown fixture scenario {other:?}"
                )));
            }
        }
        Ok(())
    })?;
    let effect = executor
        .handler()
        .effect_for_outcome(&execution.outcome.outcome_id)?;
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "outcome": execution.outcome,
            "effect": effect,
        }))?
    );
    Ok(())
}

struct FixtureEffects;

impl crate::typed_ooda::EffectExecutor for FixtureEffects {
    fn execute(
        &self,
        _job: &crate::typed_ooda::EffectJob,
    ) -> Result<crate::typed_ooda::EffectResult, crate::typed_ooda::EffectExecutionError> {
        Ok(crate::typed_ooda::EffectResult::Succeeded {
            evidence: Vec::new(),
        })
    }
}

fn open_ledger(
    state_root: &Path,
) -> Result<crate::typed_ooda::CapabilityHandler, Box<dyn std::error::Error>> {
    let directory = state_root.join("typed-ooda");
    std::fs::create_dir_all(&directory)?;
    Ok(crate::typed_ooda::CapabilityHandler::open(
        directory.join("outcomes.sqlite3"),
        crate::typed_ooda::CapabilityPolicy::goal_session_default("goal-session-policy-v1"),
    )?)
}

fn parse_named_args(
    args: impl Iterator<Item = String>,
) -> Result<std::collections::BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let values: Vec<_> = args.collect();
    let mut parsed = std::collections::BTreeMap::new();
    let mut index = 0;
    while index < values.len() {
        let flag = values[index]
            .strip_prefix("--")
            .ok_or_else(|| format!("expected named option, got {:?}", values[index]))?;
        let value = values
            .get(index + 1)
            .ok_or_else(|| format!("--{flag} requires a value"))?;
        if parsed.insert(flag.to_string(), value.clone()).is_some() {
            return Err(format!("duplicate option --{flag}").into());
        }
        index += 2;
    }
    Ok(parsed)
}

fn required_named<'a>(
    values: &'a std::collections::BTreeMap<String, String>,
    key: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    values
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| format!("missing required option --{key}").into())
}

#[cfg(test)]
mod tests {
    use crate::operator_cli::dispatch_operator_cli;

    #[test]
    fn test_ooda_missing_subcommand() {
        let result = dispatch_operator_cli(vec!["ooda".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected ooda command")
        );
    }

    #[test]
    fn test_ooda_unknown_subcommand() {
        let result = dispatch_operator_cli(vec!["ooda".to_string(), "xyz".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command 'ooda xyz'")
        );
    }

    #[test]
    fn test_ooda_help_exits_ok() {
        let result = dispatch_operator_cli(vec!["ooda".to_string(), "--help".to_string()]);
        assert!(result.is_ok(), "ooda --help must exit Ok, got: {result:?}");
    }

    #[test]
    fn test_ooda_short_help_exits_ok() {
        let result = dispatch_operator_cli(vec!["ooda".to_string(), "-h".to_string()]);
        assert!(result.is_ok(), "ooda -h must exit Ok, got: {result:?}");
    }

    #[test]
    fn test_ooda_run_invalid_cycles() {
        let result = dispatch_operator_cli(vec![
            "ooda".to_string(),
            "run".to_string(),
            "--cycles=abc".to_string(),
        ]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid --cycles"));
    }

    #[test]
    fn test_ooda_run_extra_positional_after_state_root() {
        let result = dispatch_operator_cli(vec![
            "ooda".to_string(),
            "run".to_string(),
            "/state".to_string(),
            "extra".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected argument")
        );
    }
}
