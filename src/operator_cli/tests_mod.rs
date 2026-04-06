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
