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
  terminal <spawn-engineer|no-action|blocked|completed> [SCOPED OPTIONS]
                              Record exactly one authenticated typed terminal.
  approvals issue --state-root <PATH> --effect-id <ID> --request-id <ID>
                              Issue a privileged merge/deploy approval from
                              the configured server principal and signing key.
  fixture run --state-root <PATH> --scenario <spawn-engineer|no-action|agent-spawn-engineer|agent-no-action> --request-id <ID>
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
        "terminal" => dispatch_terminal(args),
        "approvals" => dispatch_approvals(args),
        other => Err(format!("unsupported command 'ooda {other}'").into()),
    }
}

fn dispatch_terminal(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let terminal = next_required(&mut args, "terminal command")?;
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
    let request_id = required_named(&parsed, "request-id")?;
    let identity =
        crate::typed_ooda::TerminalRequestIdentity::new(request_id, session_id, cycle_id, goal_id);
    let raw_semantic = read_opaque(&parsed, "raw-semantic-path")?;
    let outcome = match terminal.as_str() {
        "spawn-engineer" => {
            let repository = actor
                .bound_repository()
                .cloned()
                .ok_or("authenticated actor has no repository scope")?;
            let claim_key = format!("{}/{}:{goal_id}", repository.owner, repository.name);
            handler.record_action(
                &actor,
                crate::typed_ooda::RecordActionRequest {
                    identity,
                    action: crate::typed_ooda::Action::SpawnEngineer(
                        crate::typed_ooda::SpawnEngineerAction {
                            task: read_opaque(&parsed, "task-path")?,
                            repository,
                            base_type: crate::typed_ooda::BaseType::Copilot,
                            requested_permissions: actor.engineer_permissions().clone(),
                            claim_key,
                        },
                    ),
                    raw_semantic,
                    evidence: Vec::new(),
                },
                &admission,
            )?
        }
        "no-action" => handler.record_no_action(
            &actor,
            crate::typed_ooda::RecordNoActionRequest {
                identity,
                reason: read_opaque(&parsed, "reason-path")?,
                raw_semantic,
                evidence: Vec::new(),
            },
        )?,
        "blocked" => handler.record_blocked(
            &actor,
            crate::typed_ooda::RecordBlockedRequest {
                identity,
                reason: read_opaque(&parsed, "reason-path")?,
                blocker: crate::typed_ooda::BlockerRef::External {
                    provider: "goal-session".to_string(),
                    reference: required_named(&parsed, "blocker")?.to_string(),
                },
                retry: crate::typed_ooda::RetryPolicy::Never,
                raw_semantic,
                evidence: Vec::new(),
            },
        )?,
        "completed" => handler.record_completed(
            &actor,
            crate::typed_ooda::RecordCompletedRequest {
                identity,
                summary: read_opaque(&parsed, "summary-path")?,
                completion: crate::typed_ooda::CompletionRef {
                    criterion_id: required_named(&parsed, "criterion-id")?.to_string(),
                    verification_evidence: Vec::new(),
                },
                raw_semantic,
                evidence: Vec::new(),
            },
        )?,
        other => return Err(format!("unsupported command 'ooda terminal {other}'").into()),
    };
    println!("{}", outcome.outcome_id);
    Ok(())
}

fn read_opaque(
    values: &std::collections::BTreeMap<String, String>,
    key: &str,
) -> Result<crate::typed_ooda::OpaqueBytes, Box<dyn std::error::Error>> {
    Ok(crate::typed_ooda::OpaqueBytes::from(std::fs::read(
        required_named(values, key)?,
    )?))
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
    let request_id = required_named(&parsed, "request-id")?;
    let handler = open_ledger(state_root)?;
    let authority = crate::typed_ooda::ApprovalAuthority::from_environment()?;
    let approval = handler.issue_privileged_approval(&authority, request_id, effect_id)?;
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
    if matches!(scenario, "agent-spawn-engineer" | "agent-no-action") {
        return dispatch_agent_fixture(state_root, scenario, request_id);
    }
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
    .scoped_to_repository(crate::typed_ooda::RepositoryRef::new("rysweet", "Simard"))
    .with_engineer_permissions(["repo_read"]);
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

fn dispatch_agent_fixture(
    state_root: &Path,
    scenario: &str,
    request_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo_root = std::env::current_dir()?;
    let route = crate::typed_ooda::TypedGoalSessionRoute::production(&repo_root)?;
    let policy = route.load_policy()?;
    let ledger_path = crate::typed_ooda::ledger_path(state_root);
    std::fs::create_dir_all(
        ledger_path
            .parent()
            .ok_or_else(|| std::io::Error::other("typed-OODA ledger path has no parent"))?,
    )?;
    let handler = crate::typed_ooda::CapabilityHandler::open(&ledger_path, policy)?;
    let session_id = format!("typed-ooda-agent-fixture-{request_id}");
    let cycle_id = format!("agent-cycle-{request_id}");
    let goal_id = format!("agent-goal-{request_id}");
    let actor = crate::typed_ooda::AuthenticatedToolContext::new(
        "goal-session-actor",
        &session_id,
        [
            crate::typed_ooda::CapabilityGrant::RecordAction(
                crate::typed_ooda::ActionKind::SpawnEngineer,
            ),
            crate::typed_ooda::CapabilityGrant::RecordNoAction,
            crate::typed_ooda::CapabilityGrant::RecordBlocked,
            crate::typed_ooda::CapabilityGrant::RecordCompleted,
        ],
    )
    .scoped_to_repository(crate::typed_ooda::RepositoryRef::new("rysweet", "Simard"))
    .scoped_to_working_directory(&repo_root)
    .with_engineer_permissions(["repo_read", "repo_write"]);
    let (task, reason) = match scenario {
        "agent-spawn-engineer" => (
            "No engineer, branch, or pull request exists for this bounded goal. Start one engineer to implement it.",
            "The goal is actionable now and needs a single engineer.",
        ),
        "agent-no-action" => (
            "An engineer is already active for this goal and reported progress moments ago.",
            "Avoid duplicate work while the active engineer continues.",
        ),
        _ => unreachable!("caller restricts agent fixture scenarios"),
    };
    let invocation = crate::typed_ooda::GoalSessionInvocation {
        session_id: session_id.clone(),
        cycle_id,
        goal_id,
        task: crate::typed_ooda::OpaqueBytes::from(task.as_bytes().to_vec()),
        reason: crate::typed_ooda::OpaqueBytes::from(reason.as_bytes().to_vec()),
        observe_output: crate::typed_ooda::OpaqueBytes::from(
            b"Observe found the stated engineer lifecycle facts.".to_vec(),
        ),
        orient_output: crate::typed_ooda::OpaqueBytes::from(
            b"Orient found no conflicting higher-priority constraint.".to_vec(),
        ),
        decide_output: crate::typed_ooda::OpaqueBytes::from(
            b"Decide delegated the semantic terminal choice to this actor.".to_vec(),
        ),
    };
    let execution = route.execute(
        &repo_root,
        &ledger_path,
        &handler,
        &actor,
        &crate::typed_ooda::AdmissionSnapshot {
            concurrent_engineers: usize::from(scenario == "agent-no-action"),
            disk_used_percent: 0,
            active_claims: if scenario == "agent-no-action" {
                BTreeSet::from([format!("rysweet/Simard:{}", invocation.goal_id)])
            } else {
                BTreeSet::new()
            },
            policy_revision: "goal-session-policy-v1".to_string(),
        },
        &invocation,
    )?;
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({ "outcome": execution.outcome }))?
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
    let ledger_path = crate::typed_ooda::ledger_path(state_root);
    std::fs::create_dir_all(
        ledger_path
            .parent()
            .ok_or_else(|| std::io::Error::other("typed-OODA ledger path has no parent"))?,
    )?;
    Ok(crate::typed_ooda::CapabilityHandler::open(
        ledger_path,
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
