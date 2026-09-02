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
            .contains("unexpected argument")
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
        "safe-update",
        "rollback",
        "rollback-watchdog",
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
    let result = dispatch_operator_cli(vec!["handover".to_string(), "--bad-flag=x".to_string()]);
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

// ── safe-update / rollback / rollback-watchdog dispatch ──

#[test]
fn test_safe_update_rejects_extra_args() {
    let result = dispatch_operator_cli(vec!["safe-update".to_string(), "extra".to_string()]);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("unexpected trailing arguments")
    );
}

#[test]
fn test_rollback_rejects_extra_args() {
    let result = dispatch_operator_cli(vec!["rollback".to_string(), "extra".to_string()]);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("unexpected trailing arguments")
    );
}

#[test]
fn test_rollback_watchdog_max_iterations_zero_exits_cleanly() {
    // --max-iterations=0 means the loop body still runs once but exits before sleeping.
    // This proves the dispatch path wires the flag; no real rollback work is performed
    // because the temporary state dir contains no upgrade-status.json.
    let result = dispatch_operator_cli(vec![
        "rollback-watchdog".to_string(),
        "--max-iterations=1".to_string(),
        "--interval=1".to_string(),
    ]);
    assert!(
        result.is_ok(),
        "rollback-watchdog --max-iterations=1 should exit cleanly, got: {result:?}"
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

// ─────────────────────────────────────────────────────────────────────────────
// Issue #1911 — `simard goal` subcommand dispatcher-wiring tests.
//
// T8 in the test contract: the dispatcher in `src/operator_cli/mod.rs`
// must route `goal list`, `goal unblock <id>`, and `goal unblock-all` to
// the goal subcommand handler. Validate the wiring by exercising
// `dispatch_operator_cli` with argument-parsing-only paths that do NOT
// touch cognitive memory or fork subprocesses. The deeper
// integration-test surface (TSV schema, audit lines, override facts) lives
// in `src/operator_cli/tests_goal.rs`.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_goal_subcommand_missing_returns_error() {
    let result = dispatch_operator_cli(vec!["goal".to_string()]);
    assert!(
        result.is_err(),
        "bare `simard goal` must require a subcommand"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("expected goal command") || msg.contains("expected goal subcommand"),
        "error should explain the missing subcommand; got: {msg}"
    );
}

#[test]
fn test_goal_subcommand_unknown_verb_returns_error() {
    let result = dispatch_operator_cli(vec!["goal".to_string(), "frobnicate".to_string()]);
    assert!(
        result.is_err(),
        "unknown `simard goal frobnicate` must error"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("unsupported command 'goal frobnicate'")
            || msg.contains("unsupported command: goal frobnicate"),
        "error should name the unsupported subcommand; got: {msg}"
    );
}

#[test]
fn test_goal_unblock_missing_id_returns_error() {
    let result = dispatch_operator_cli(vec!["goal".to_string(), "unblock".to_string()]);
    assert!(
        result.is_err(),
        "`simard goal unblock` without a goal-id must error"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("expected goal id") || msg.contains("expected goal-id"),
        "error should explain the missing goal id; got: {msg}"
    );
}

#[test]
fn test_help_text_mentions_goal_subcommands() {
    let help = operator_cli_help();
    for needle in &["goal list", "goal unblock", "goal unblock-all"] {
        assert!(
            help.contains(needle),
            "help must document '{needle}' subcommand for issue #1911"
        );
    }
}

// ── issue #1981: --help must work on every subcommand ──

#[test]
fn test_all_subcommands_accept_help_flag() {
    // Every operator subcommand must accept --help, -h, and help
    // without erroring. This is the regression test for issue #1981.
    let subcommands = &[
        "engineer",
        "meeting",
        "goal",
        "goal-curation",
        "improvement-curation",
        "review",
        "gym",
        "ooda",
        "dashboard",
        "signal",
        "spawn",
        "merge-pr",
        "worktree-gc",
        "handover",
        "bootstrap",
        "act-on-decisions",
        "update",
        "self-test",
        "safe-update",
        "rollback",
        "rollback-watchdog",
        "ensure-deps",
        "cleanup",
        "install",
    ];
    for subcmd in subcommands {
        for flag in &["--help", "-h"] {
            let result = dispatch_operator_cli(vec![subcmd.to_string(), flag.to_string()]);
            assert!(
                result.is_ok(),
                "`simard {subcmd} {flag}` must exit Ok (issue #1981), got: {result:?}"
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #4721 (WS-2): `simard merge record-verdict` — the agent-facing gated
// write tool the merge-readiness recipe calls to durably record its typed
// verdict (replacing the forbidden JSON-emit→scrape pattern).
//
// TDD contract for the NOT-YET-IMPLEMENTED tool in `operator_cli::merge`
// (design C1 / resolved-requirements A7/A8):
//
//   merge::parse_record_verdict_args(Vec<String>)
//        -> Result<merge::RecordVerdictArgs, String>
//   merge::run_record_verdict(Vec<String>) -> i32   // 0 ok / 2 usage / 3 IO
//
//   RecordVerdictArgs { pr: u32, repo: String, verdict: VerdictKind,
//                       reason: String, run_token: String,
//                       state_root: Option<PathBuf> }
//
// Flags: --pr <u32> --repo <owner/name> --verdict merge|hold
//        --reason "<text>" --run-token <str> [--state-root <path>]
// Both `--flag value` and `--flag=value` forms are accepted.
//
// These are expected to FAIL TO COMPILE until C1 lands (TDD red).
#[cfg(test)]
mod issue_4721_record_verdict_tests {
    use std::path::PathBuf;

    use crate::operator_cli::merge::{parse_record_verdict_args, run_record_verdict};
    use crate::stewardship::merge_verdict_store::{ReadOutcome, VerdictKind, read_verified};

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn temp_state_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "simard-recordverdict-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ── parse: happy paths ──────────────────────────────────────────────────

    #[test]
    fn parse_valid_merge_verdict() {
        let parsed = parse_record_verdict_args(args(&[
            "--pr",
            "1500",
            "--repo",
            "rysweet/Simard",
            "--verdict",
            "merge",
            "--reason",
            "crusty passed; CI green",
            "--run-token",
            "tok-abc",
        ]))
        .expect("valid merge invocation");
        assert_eq!(parsed.pr, 1500);
        assert_eq!(parsed.repo, "rysweet/Simard");
        assert_eq!(parsed.verdict, VerdictKind::Merge);
        assert_eq!(parsed.reason, "crusty passed; CI green");
        assert_eq!(parsed.run_token, "tok-abc");
    }

    #[test]
    fn parse_valid_hold_verdict_with_equals_form() {
        let parsed = parse_record_verdict_args(args(&[
            "--pr=42",
            "--repo=o/r",
            "--verdict=hold",
            "--reason=needs more tests",
            "--run-token=tok",
        ]))
        .expect("valid hold invocation, equals form");
        assert_eq!(parsed.pr, 42);
        assert_eq!(parsed.verdict, VerdictKind::Hold);
    }

    #[test]
    fn parse_accepts_state_root_override() {
        let parsed = parse_record_verdict_args(args(&[
            "--pr",
            "1",
            "--repo",
            "o/r",
            "--verdict",
            "merge",
            "--reason",
            "x",
            "--run-token",
            "t",
            "--state-root",
            "/tmp/some-root",
        ]))
        .expect("state-root override accepted");
        assert_eq!(parsed.state_root, Some(PathBuf::from("/tmp/some-root")));
    }

    // ── parse: validation errors (exit 2) ───────────────────────────────────

    #[test]
    fn parse_rejects_invalid_verdict_word() {
        for bad in ["yes", "ready", "MERGE", "Hold", ""] {
            let r = parse_record_verdict_args(args(&[
                "--pr",
                "1",
                "--repo",
                "o/r",
                "--verdict",
                bad,
                "--reason",
                "x",
                "--run-token",
                "t",
            ]));
            assert!(
                r.is_err(),
                "verdict {bad:?} must be rejected (only lowercase merge|hold)"
            );
        }
    }

    #[test]
    fn parse_rejects_non_numeric_pr() {
        let r = parse_record_verdict_args(args(&[
            "--pr",
            "abc",
            "--repo",
            "o/r",
            "--verdict",
            "merge",
            "--reason",
            "x",
            "--run-token",
            "t",
        ]));
        assert!(r.is_err(), "non-numeric --pr must be rejected");
    }

    #[test]
    fn parse_rejects_empty_reason() {
        let r = parse_record_verdict_args(args(&[
            "--pr",
            "1",
            "--repo",
            "o/r",
            "--verdict",
            "merge",
            "--reason",
            "",
            "--run-token",
            "t",
        ]));
        assert!(r.is_err(), "empty --reason must be rejected");
    }

    #[test]
    fn parse_rejects_each_missing_required_flag() {
        // Every one of these omits exactly one required flag.
        let cases: &[&[&str]] = &[
            &[
                "--repo",
                "o/r",
                "--verdict",
                "merge",
                "--reason",
                "x",
                "--run-token",
                "t",
            ], // no --pr
            &[
                "--pr",
                "1",
                "--verdict",
                "merge",
                "--reason",
                "x",
                "--run-token",
                "t",
            ], // no --repo
            &[
                "--pr",
                "1",
                "--repo",
                "o/r",
                "--reason",
                "x",
                "--run-token",
                "t",
            ], // no --verdict
            &[
                "--pr",
                "1",
                "--repo",
                "o/r",
                "--verdict",
                "merge",
                "--run-token",
                "t",
            ], // no --reason
            &[
                "--pr",
                "1",
                "--repo",
                "o/r",
                "--verdict",
                "merge",
                "--reason",
                "x",
            ], // no --run-token
        ];
        for c in cases {
            assert!(
                parse_record_verdict_args(args(c)).is_err(),
                "invocation missing a required flag must be rejected: {c:?}"
            );
        }
    }

    #[test]
    fn parse_rejects_malformed_repo() {
        for bad in ["no-slash", "a/b/c", "/abs", "owner/", "../evil"] {
            let r = parse_record_verdict_args(args(&[
                "--pr",
                "1",
                "--repo",
                bad,
                "--verdict",
                "merge",
                "--reason",
                "x",
                "--run-token",
                "t",
            ]));
            assert!(r.is_err(), "malformed --repo {bad:?} must be rejected");
        }
    }

    // ── run: exit codes + durable write readable by the rail ────────────────

    #[test]
    fn run_records_merge_and_rail_reads_it_back() {
        let root = temp_state_root("run-merge");
        let code = run_record_verdict(args(&[
            "--pr",
            "77",
            "--repo",
            "rysweet/Simard",
            "--verdict",
            "merge",
            "--reason",
            "crusty passed",
            "--run-token",
            "run-xyz",
            "--state-root",
            root.to_str().unwrap(),
        ]));
        assert_eq!(code, 0, "a valid record write must exit 0");

        // The exact record the deterministic rail will consume (design R1↔R3).
        match read_verified(&root, "rysweet/Simard", 77, "run-xyz") {
            ReadOutcome::Found(rec) => {
                assert_eq!(rec.verdict, VerdictKind::Merge);
                assert_eq!(rec.reason, "crusty passed");
                assert_eq!(rec.run_token, "run-xyz");
            }
            other => panic!("rail must read back the tool's record, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_usage_error_exits_2() {
        let root = temp_state_root("run-usage");
        let code = run_record_verdict(args(&[
            "--pr",
            "notanumber",
            "--repo",
            "o/r",
            "--verdict",
            "merge",
            "--reason",
            "x",
            "--run-token",
            "t",
            "--state-root",
            root.to_str().unwrap(),
        ]));
        assert_eq!(code, 2, "a usage/validation error must exit 2");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_overwrites_prior_record_for_same_pr() {
        let root = temp_state_root("run-overwrite");
        let first = args(&[
            "--pr",
            "5",
            "--repo",
            "o/r",
            "--verdict",
            "hold",
            "--reason",
            "old",
            "--run-token",
            "t1",
            "--state-root",
            root.to_str().unwrap(),
        ]);
        assert_eq!(run_record_verdict(first), 0);
        let second = args(&[
            "--pr",
            "5",
            "--repo",
            "o/r",
            "--verdict",
            "merge",
            "--reason",
            "new",
            "--run-token",
            "t2",
            "--state-root",
            root.to_str().unwrap(),
        ]);
        assert_eq!(run_record_verdict(second), 0);

        match read_verified(&root, "o/r", 5, "t2") {
            ReadOutcome::Found(rec) => assert_eq!(rec.verdict, VerdictKind::Merge),
            other => panic!("second write must win, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn issue_4721_help_advertises_merge_record_verdict() {
    let help = operator_cli_help();
    assert!(
        help.contains("record-verdict"),
        "operator help must advertise the `merge record-verdict` tool"
    );
}
