use assert_cmd::Command;
use serde_json::Value;
use simard::typed_ooda::{
    AdmissionSnapshot, AuthenticatedToolContext, CapabilityGrant, CapabilityHandler,
    CapabilityPolicy, EvidenceRef, RepositoryRef, TerminalKind,
};
use std::collections::BTreeSet;
use std::time::Duration;

fn simard() -> Command {
    let mut command = Command::cargo_bin("simard").expect("simard binary");
    command.env("SIMARD_TYPED_OODA_FIXTURE", "1");
    command
}

fn output_json(assert: assert_cmd::assert::Assert) -> Value {
    serde_json::from_slice(&assert.get_output().stdout).expect("JSON output")
}

#[test]
fn fixture_completes_action_and_no_action_cycles_from_durable_records() {
    let state = tempfile::tempdir().expect("state");
    let action = output_json(
        simard()
            .args(["ooda", "fixture", "run", "--state-root"])
            .arg(state.path())
            .args([
                "--scenario",
                "spawn-engineer",
                "--request-id",
                "fixture-action-1",
            ])
            .assert()
            .success(),
    );
    assert_eq!(action["outcome"]["kind"], "action");
    assert_eq!(action["effect"]["state"], "succeeded");

    let no_action = output_json(
        simard()
            .args(["ooda", "fixture", "run", "--state-root"])
            .arg(state.path())
            .args([
                "--scenario",
                "no-action",
                "--request-id",
                "fixture-no-action-1",
            ])
            .assert()
            .success(),
    );
    assert_eq!(no_action["outcome"]["kind"], "no_action");
    assert!(no_action["effect"].is_null());

    let listed = output_json(
        simard()
            .args(["ooda", "outcomes", "list", "--state-root"])
            .arg(state.path())
            .args(["--limit", "10"])
            .assert()
            .success(),
    );
    assert_eq!(listed["outcomes"].as_array().expect("outcomes").len(), 2);
}

#[test]
fn fixture_is_rejected_without_explicit_test_gate() {
    let state = tempfile::tempdir().expect("state");
    Command::cargo_bin("simard")
        .expect("simard binary")
        .args(["ooda", "fixture", "run", "--state-root"])
        .arg(state.path())
        .args(["--scenario", "no-action", "--request-id", "fixture-denied"])
        .assert()
        .failure();
}

#[test]
fn scoped_terminal_cli_records_no_action_without_a_rust_hosted_actor() {
    let state = tempfile::tempdir().expect("state");
    let ledger = state.path().join("outcomes.sqlite3");
    let policy_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets/simard/policies/goal-session-capabilities.toml");
    let policy = CapabilityPolicy::from_toml_file(&policy_path).expect("policy");
    let handler = CapabilityHandler::open(&ledger, policy).expect("ledger");
    let session_id = "cli-agent-session";
    let cycle_id = "cli-agent-cycle";
    let goal_id = "cli-agent-goal";
    let actor = AuthenticatedToolContext::new(
        "goal-session-actor",
        session_id,
        [CapabilityGrant::RecordNoAction],
    )
    .scoped_to_repository(RepositoryRef::new("rysweet", "Simard"))
    .bound_to_cycle_goal(cycle_id, goal_id)
    .with_engineer_permissions(["repo_read", "repo_write"]);
    let lease = handler
        .register_actor_session(
            &actor,
            "register-cli-agent-session",
            cycle_id,
            goal_id,
            Duration::from_secs(60),
        )
        .expect("actor lease");
    let token_path = state.path().join("token");
    std::fs::write(&token_path, lease.token).expect("token");
    let admission_path = state.path().join("admission.json");
    std::fs::write(
        &admission_path,
        serde_json::to_vec(&AdmissionSnapshot {
            concurrent_engineers: 0,
            disk_used_percent: 0,
            active_claims: BTreeSet::new(),
            policy_revision: "goal-session-policy-v1".to_string(),
        })
        .expect("admission JSON"),
    )
    .expect("admission");
    let reason_path = state.path().join("reason");
    let raw_path = state.path().join("raw");
    std::fs::write(&reason_path, b"wait for the active engineer").expect("reason");
    std::fs::write(&raw_path, b"free-form semantic agent context").expect("raw");

    Command::cargo_bin("simard")
        .expect("simard binary")
        .args(["ooda", "terminal", "status", "--ledger-path"])
        .arg(&ledger)
        .args(["--policy-path"])
        .arg(&policy_path)
        .args(["--session-id", session_id, "--cycle-id", cycle_id])
        .assert()
        .success()
        .stdout("missing\n");

    Command::cargo_bin("simard")
        .expect("simard binary")
        .args(["ooda", "terminal", "no-action", "--ledger-path"])
        .arg(&ledger)
        .args(["--policy-path"])
        .arg(&policy_path)
        .args([
            "--session-id",
            session_id,
            "--cycle-id",
            cycle_id,
            "--goal-id",
            goal_id,
            "--auth-token-path",
        ])
        .arg(&token_path)
        .args(["--admission-path"])
        .arg(&admission_path)
        .args(["--request-id", "cli-no-action", "--reason-path"])
        .arg(&reason_path)
        .args(["--raw-semantic-path"])
        .arg(&raw_path)
        .assert()
        .success();

    Command::cargo_bin("simard")
        .expect("simard binary")
        .args(["ooda", "terminal", "status", "--ledger-path"])
        .arg(&ledger)
        .args(["--policy-path"])
        .arg(&policy_path)
        .args(["--session-id", session_id, "--cycle-id", cycle_id])
        .assert()
        .success()
        .stdout("present\n");

    let outcome = handler
        .terminal_for_cycle(session_id, cycle_id)
        .expect("terminal query")
        .expect("durable terminal");
    assert_eq!(outcome.kind, TerminalKind::NoAction);
    assert_eq!(
        outcome.raw_semantic.as_bytes(),
        b"free-form semantic agent context"
    );
}

#[test]
fn completed_terminal_cli_requires_typed_evidence_and_records_a_completion() {
    let state = tempfile::tempdir().expect("state");
    let ledger = state.path().join("outcomes.sqlite3");
    let policy_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets/simard/policies/goal-session-capabilities.toml");
    let policy = CapabilityPolicy::from_toml_file(&policy_path).expect("policy");
    let handler = CapabilityHandler::open(&ledger, policy).expect("ledger");
    let session_id = "cli-complete-session";
    let cycle_id = "cli-complete-cycle";
    let goal_id = "cli-complete-goal";
    let actor = AuthenticatedToolContext::new(
        "goal-session-actor",
        session_id,
        [CapabilityGrant::RecordCompleted],
    )
    .scoped_to_repository(RepositoryRef::new("rysweet", "Simard"))
    .bound_to_cycle_goal(cycle_id, goal_id)
    .with_engineer_permissions(["repo_read", "repo_write"]);
    let lease = handler
        .register_actor_session(
            &actor,
            "register-cli-complete-session",
            cycle_id,
            goal_id,
            Duration::from_secs(60),
        )
        .expect("actor lease");
    let token_path = state.path().join("token");
    std::fs::write(&token_path, lease.token).expect("token");
    let admission_path = state.path().join("admission.json");
    std::fs::write(
        &admission_path,
        serde_json::to_vec(&AdmissionSnapshot {
            concurrent_engineers: 0,
            disk_used_percent: 0,
            active_claims: BTreeSet::new(),
            policy_revision: "goal-session-policy-v1".to_string(),
        })
        .expect("admission JSON"),
    )
    .expect("admission");
    let summary_path = state.path().join("summary");
    let raw_path = state.path().join("raw");
    std::fs::write(&summary_path, b"goal shipped and verified").expect("summary");
    std::fs::write(&raw_path, b"free-form semantic completion context").expect("raw");

    // A completion without typed evidence must be rejected by the capability.
    let empty_evidence = state.path().join("empty-evidence.json");
    std::fs::write(&empty_evidence, b"[]").expect("empty evidence");
    Command::cargo_bin("simard")
        .expect("simard binary")
        .args(["ooda", "terminal", "completed", "--ledger-path"])
        .arg(&ledger)
        .args(["--policy-path"])
        .arg(&policy_path)
        .args([
            "--session-id",
            session_id,
            "--cycle-id",
            cycle_id,
            "--goal-id",
            goal_id,
            "--auth-token-path",
        ])
        .arg(&token_path)
        .args(["--admission-path"])
        .arg(&admission_path)
        .args(["--request-id", "cli-complete-empty", "--summary-path"])
        .arg(&summary_path)
        .args(["--criterion-id", "goal-session-complete", "--evidence-path"])
        .arg(&empty_evidence)
        .args(["--raw-semantic-path"])
        .arg(&raw_path)
        .assert()
        .failure();

    // A completion with typed evidence succeeds and records a Completed terminal.
    let evidence_path = state.path().join("evidence.json");
    std::fs::write(
        &evidence_path,
        serde_json::to_vec(&vec![EvidenceRef::Commit {
            repository: RepositoryRef::new("rysweet", "Simard"),
            sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
        }])
        .expect("evidence JSON"),
    )
    .expect("evidence");
    Command::cargo_bin("simard")
        .expect("simard binary")
        .args(["ooda", "terminal", "completed", "--ledger-path"])
        .arg(&ledger)
        .args(["--policy-path"])
        .arg(&policy_path)
        .args([
            "--session-id",
            session_id,
            "--cycle-id",
            cycle_id,
            "--goal-id",
            goal_id,
            "--auth-token-path",
        ])
        .arg(&token_path)
        .args(["--admission-path"])
        .arg(&admission_path)
        .args(["--request-id", "cli-complete-ok", "--summary-path"])
        .arg(&summary_path)
        .args(["--criterion-id", "goal-session-complete", "--evidence-path"])
        .arg(&evidence_path)
        .args(["--raw-semantic-path"])
        .arg(&raw_path)
        .assert()
        .success();

    let outcome = handler
        .terminal_for_cycle(session_id, cycle_id)
        .expect("terminal query")
        .expect("durable terminal");
    assert_eq!(outcome.kind, TerminalKind::Completed);
}
