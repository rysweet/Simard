//! Public contract tests for the parser-free OODA capability boundary.
//!
//! TDD status: RED until `simard::typed_ooda` implements these contracts.
//! These tests intentionally exercise typed tool data only; no test derives a
//! business decision from model prose, markers, JSON-looking semantic text, or
//! error strings.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use simard::typed_ooda::{
    Action, ActionKind, AdmissionSnapshot, AuthenticatedToolContext, BaseType, CapabilityErrorCode,
    CapabilityGrant, CapabilityHandler, CapabilityPolicy, EvidenceRef, OpaqueBytes,
    RecordActionRequest, RecordNoActionRequest, RecordProgressRequest, RepositoryRef,
    SpawnEngineerAction, TerminalKind, TerminalRequestIdentity,
};

fn identity(request_id: &str, cycle_id: &str) -> TerminalRequestIdentity {
    TerminalRequestIdentity::new(request_id, "session-4052", cycle_id, "goal-4052")
}

fn actor(
    cycle_id: &str,
    grants: impl IntoIterator<Item = CapabilityGrant>,
) -> AuthenticatedToolContext {
    AuthenticatedToolContext::new("goal-session-actor", "session-4052", grants)
        .scoped_to_repository(RepositoryRef::new("rysweet", "Simard"))
        .bound_to_cycle_goal(cycle_id, "goal-4052")
        .with_engineer_permissions([
            "repo_read",
            "repo_write",
            "process_exec",
            "github_issue_write",
            "github_pr_write",
        ])
}

fn policy() -> CapabilityPolicy {
    CapabilityPolicy::new("policy-v1")
}

fn handler() -> (tempfile::TempDir, CapabilityHandler) {
    let dir = tempfile::tempdir().expect("tempdir");
    let handler = CapabilityHandler::open(dir.path().join("outcomes.sqlite3"), policy())
        .expect("open typed outcome ledger");
    (dir, handler)
}

fn spawn_action(task: Vec<u8>) -> Action {
    Action::SpawnEngineer(SpawnEngineerAction {
        task: OpaqueBytes::from(task),
        repository: RepositoryRef::new("rysweet", "Simard"),
        base_type: BaseType::Copilot,
        requested_permissions: BTreeSet::from(["repo_read".to_string(), "repo_write".to_string()]),
        claim_key: "rysweet/Simard:goal-4052".to_string(),
    })
}

fn action_request(request_id: &str, cycle_id: &str, task: Vec<u8>) -> RecordActionRequest {
    RecordActionRequest {
        identity: identity(request_id, cycle_id),
        action: spawn_action(task),
        raw_semantic: OpaqueBytes::from(b"semantic decision bytes".to_vec()),
        evidence: Vec::new(),
    }
}

fn admitted() -> AdmissionSnapshot {
    AdmissionSnapshot {
        concurrent_engineers: 0,
        disk_used_percent: 10,
        active_claims: BTreeSet::new(),
        policy_revision: "admission-v1".to_string(),
    }
}

#[test]
fn opaque_semantic_transport_round_trips_every_byte_exactly() {
    let cases = vec![
        Vec::new(),
        b"\nleading and trailing newlines\n".to_vec(),
        b"ACTION: SPAWN_ENGINEER\nNO ACTION\nPROGRESS: 100".to_vec(),
        br#"{"decision":"deploy","url":"https://example.invalid/pr/42"}"#.to_vec(),
        b"first-word-looking-marker\0embedded-nul".to_vec(),
        "non-ASCII: \u{1f980} e\u{301} \u{00e9}".as_bytes().to_vec(),
        vec![0xff, 0xfe, 0xfd, 0x00, 0x1b, b'[', b'3', b'1', b'm'],
        (0..=(256 * 1024))
            .map(|index| (index % 251) as u8)
            .collect(),
    ];

    for expected in cases {
        let encoded = serde_json::to_vec(&OpaqueBytes::from(expected.clone()))
            .expect("typed protocol serialization");
        let wire = String::from_utf8(encoded.clone()).expect("JSON is UTF-8");
        assert!(wire.contains("\"encoding\":\"base64\""));
        assert!(wire.contains("\"data\":"));
        let decoded: OpaqueBytes =
            serde_json::from_slice(&encoded).expect("typed protocol deserialization");
        assert_eq!(
            decoded.as_bytes(),
            expected.as_slice(),
            "typed transport must not trim, normalize, repair, or reinterpret bytes"
        );
    }
}

#[test]
fn opaque_transport_rejects_noncanonical_base64_instead_of_repairing_it() {
    for invalid in [
        br#"{"encoding":"utf8","data":"dGVzdA=="}"#.as_slice(),
        br#"{"encoding":"base64","data":"dGVzdA"}"#.as_slice(),
        br#"{"encoding":"base64","data":"dGVz dA=="}"#.as_slice(),
        br#"{"encoding":"base64","data":"***="}"#.as_slice(),
    ] {
        serde_json::from_slice::<OpaqueBytes>(invalid)
            .expect_err("noncanonical typed transport must fail explicitly");
    }
}

#[test]
fn unknown_action_variant_is_rejected_by_the_typed_protocol() {
    let wire = br#"{
        "kind":"delete_repository",
        "repository":{"owner":"rysweet","name":"Simard"}
    }"#;

    let error = serde_json::from_slice::<Action>(wire).expect_err("closed action union");
    assert!(
        error.to_string().contains("unknown variant"),
        "this assertion concerns typed protocol decoding only: {error}"
    );
}

#[test]
fn successful_action_commits_one_authoritative_terminal_and_effect_job() {
    let (_dir, handler) = handler();
    let actor = actor(
        "cycle-action-1",
        [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
    );
    let request = action_request(
        "request-action-1",
        "cycle-action-1",
        b"Implement typed OODA without parsing this task.\n".to_vec(),
    );

    let outcome = handler
        .record_action(&actor, request.clone(), &admitted())
        .expect("authorized admitted action");

    assert_eq!(outcome.kind, TerminalKind::Action);
    assert_eq!(
        outcome.raw_semantic.as_bytes(),
        request.raw_semantic.as_bytes()
    );
    assert_eq!(
        outcome
            .payload
            .action()
            .expect("action payload")
            .as_spawn_engineer()
            .expect("spawn action")
            .task
            .as_bytes(),
        b"Implement typed OODA without parsing this task.\n"
    );
    let stored = handler
        .terminal_for_cycle("session-4052", "cycle-action-1")
        .expect("ledger query")
        .expect("durable terminal");
    assert_eq!(stored.outcome_id, outcome.outcome_id);
    let effect = handler
        .effect_for_outcome(&outcome.outcome_id)
        .expect("effect query")
        .expect("action effect job");
    assert_eq!(effect.request_id, "request-action-1");
    assert_eq!(effect.state.as_str(), "pending");
}

#[test]
fn no_action_reason_and_raw_semantic_are_persisted_without_interpretation() {
    let (_dir, handler) = handler();
    let actor = actor("cycle-no-action-1", [CapabilityGrant::RecordNoAction]);
    let reason = b"NO ACTION is merely text here.\n{\"looks\":\"structured\"}\0".to_vec();
    let raw = vec![0xff, b'\n', b'A', b'C', b'T', b'I', b'O', b'N', b':'];
    let request = RecordNoActionRequest {
        identity: identity("request-no-action-1", "cycle-no-action-1"),
        reason: OpaqueBytes::from(reason.clone()),
        raw_semantic: OpaqueBytes::from(raw.clone()),
        evidence: Vec::new(),
    };

    let outcome = handler
        .record_no_action(&actor, request)
        .expect("authorized no-action");

    assert_eq!(outcome.kind, TerminalKind::NoAction);
    assert_eq!(
        outcome
            .payload
            .no_action()
            .expect("no-action payload")
            .reason
            .as_bytes(),
        reason
    );
    assert_eq!(outcome.raw_semantic.as_bytes(), raw);
    assert!(
        handler
            .effect_for_outcome(&outcome.outcome_id)
            .expect("effect query")
            .is_none(),
        "no-action must not synthesize a machine effect"
    );
}

#[test]
fn exact_replay_returns_the_existing_durable_result_even_after_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("outcomes.sqlite3");
    let actor = actor("cycle-replay-1", [CapabilityGrant::RecordNoAction]);
    let request = RecordNoActionRequest {
        identity: identity("request-replay-1", "cycle-replay-1"),
        reason: OpaqueBytes::from(b"same reason".to_vec()),
        raw_semantic: OpaqueBytes::from(b"same raw bytes".to_vec()),
        evidence: Vec::new(),
    };

    let first = {
        let handler = CapabilityHandler::open(&path, policy()).expect("first open");
        handler
            .record_no_action(&actor, request.clone())
            .expect("first commit")
    };
    let replay = {
        let handler = CapabilityHandler::open(&path, policy()).expect("reopen");
        handler
            .record_no_action(&actor, request)
            .expect("durable replay")
    };

    assert_eq!(replay, first);
}

#[test]
fn conflicting_request_id_reuse_fails_without_changing_the_first_record() {
    let (_dir, handler) = handler();
    let actor = actor("cycle-conflict-1", [CapabilityGrant::RecordNoAction]);
    let first = RecordNoActionRequest {
        identity: identity("request-conflict-1", "cycle-conflict-1"),
        reason: OpaqueBytes::from(b"first reason".to_vec()),
        raw_semantic: OpaqueBytes::from(b"first semantics".to_vec()),
        evidence: Vec::new(),
    };
    let mut conflicting = first.clone();
    conflicting.reason = OpaqueBytes::from(b"different reason".to_vec());

    let committed = handler
        .record_no_action(&actor, first)
        .expect("first commit");
    let error = handler
        .record_no_action(&actor, conflicting)
        .expect_err("conflicting replay must fail");

    assert_eq!(error.code(), CapabilityErrorCode::RequestConflict);
    assert_eq!(
        handler
            .terminal_for_cycle("session-4052", "cycle-conflict-1")
            .expect("ledger query")
            .expect("first terminal")
            .outcome_id,
        committed.outcome_id
    );
}

#[test]
fn a_second_request_id_cannot_create_another_terminal_for_the_cycle() {
    let (_dir, handler) = handler();
    let actor = actor("cycle-single-terminal", [CapabilityGrant::RecordNoAction]);
    let make_request = |request_id: &str| RecordNoActionRequest {
        identity: identity(request_id, "cycle-single-terminal"),
        reason: OpaqueBytes::from(b"same semantic result".to_vec()),
        raw_semantic: OpaqueBytes::from(b"same raw bytes".to_vec()),
        evidence: Vec::new(),
    };

    handler
        .record_no_action(&actor, make_request("request-terminal-1"))
        .expect("first terminal");
    let error = handler
        .record_no_action(&actor, make_request("request-terminal-2"))
        .expect_err("second terminal must fail");

    assert_eq!(error.code(), CapabilityErrorCode::TerminalAlreadyRecorded);
    assert_eq!(
        handler
            .terminal_count("session-4052", "cycle-single-terminal")
            .expect("count terminals"),
        1
    );
}

#[test]
fn denied_mutation_records_blocked_while_session_mismatch_fails_without_a_terminal() {
    let (_dir, handler) = handler();
    let denied_actor = actor("cycle-denied-1", []);
    let request = action_request("request-denied-1", "cycle-denied-1", b"task".to_vec());

    let denied = handler
        .record_action(&denied_actor, request, &admitted())
        .expect("missing mutation grant must become an auditable blocked terminal");
    assert_eq!(denied.kind, simard::typed_ooda::TerminalKind::Blocked);

    let wrong_session_actor = AuthenticatedToolContext::new(
        "goal-session-actor",
        "different-session",
        [CapabilityGrant::RecordNoAction],
    );
    let mismatch = handler
        .record_no_action(
            &wrong_session_actor,
            RecordNoActionRequest {
                identity: identity("request-mismatch-1", "cycle-mismatch-1"),
                reason: OpaqueBytes::from(b"reason".to_vec()),
                raw_semantic: OpaqueBytes::from(b"raw".to_vec()),
                evidence: Vec::new(),
            },
        )
        .expect_err("authenticated session mismatch must fail");
    assert_eq!(mismatch.code(), CapabilityErrorCode::Unauthenticated);

    assert_eq!(
        handler
            .terminal_count("session-4052", "cycle-denied-1")
            .expect("count denied cycle"),
        1
    );
    assert_eq!(
        handler
            .terminal_count("session-4052", "cycle-mismatch-1")
            .expect("count mismatch cycle"),
        0
    );
}

#[test]
fn observe_only_and_repository_scope_denials_are_durable_blocked_outcomes() {
    let (_dir, handler) = handler();
    let observe_only = actor(
        "cycle-observe-only",
        [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
    )
    .with_observe_only(true);
    let blocked = handler
        .record_action(
            &observe_only,
            action_request(
                "request-observe-only",
                "cycle-observe-only",
                b"must not dispatch".to_vec(),
            ),
            &admitted(),
        )
        .expect("observe-only denial is durable");
    assert_eq!(blocked.kind, TerminalKind::Blocked);
    assert!(
        handler
            .effect_for_outcome(&blocked.outcome_id)
            .expect("effect query")
            .is_none()
    );

    let scoped = actor(
        "cycle-wrong-repo",
        [CapabilityGrant::RecordAction(ActionKind::FileIssue)],
    );
    let wrong_repo = handler
        .record_action(
            &scoped,
            RecordActionRequest {
                identity: identity("request-wrong-repo", "cycle-wrong-repo"),
                action: Action::FileIssue(simard::typed_ooda::FileIssueAction {
                    repository: RepositoryRef::new("rysweet", "Other"),
                    title: OpaqueBytes::from(b"wrong scope".to_vec()),
                    body: OpaqueBytes::from(Vec::new()),
                    labels: Vec::new(),
                }),
                raw_semantic: OpaqueBytes::from(b"must not file".to_vec()),
                evidence: Vec::new(),
            },
            &admitted(),
        )
        .expect("repository denial is durable");
    assert_eq!(wrong_repo.kind, TerminalKind::Blocked);
    assert!(
        handler
            .effect_for_outcome(&wrong_repo.outcome_id)
            .expect("effect query")
            .is_none()
    );
}

#[test]
fn invalid_arguments_and_admission_rejection_are_explicit_failures() {
    let (_dir, handler) = handler();
    let invalid_actor = actor(
        "cycle-invalid-1",
        [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
    );

    let invalid = handler
        .record_action(
            &invalid_actor,
            action_request("request-invalid-1", "cycle-invalid-1", Vec::new()),
            &admitted(),
        )
        .expect_err("empty engineer task must fail validation");
    assert_eq!(invalid.code(), CapabilityErrorCode::InvalidArgument);

    let mut rejected_snapshot = admitted();
    rejected_snapshot
        .active_claims
        .insert("rysweet/Simard:goal-4052".to_string());
    let rejected_actor = actor(
        "cycle-rejected-1",
        [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
    );
    let rejected = handler
        .record_action(
            &rejected_actor,
            action_request(
                "request-rejected-1",
                "cycle-rejected-1",
                b"conflicting task".to_vec(),
            ),
            &rejected_snapshot,
        )
        .expect_err("exact claim conflict must fail closed");
    assert_eq!(rejected.code(), CapabilityErrorCode::AdmissionRejected);

    assert_eq!(
        handler
            .terminal_count("session-4052", "cycle-invalid-1")
            .expect("count invalid cycle"),
        0
    );
    assert_eq!(
        handler
            .terminal_count("session-4052", "cycle-rejected-1")
            .expect("count rejected cycle"),
        0
    );
}

#[test]
fn oversized_semantic_payload_fails_without_truncation_or_a_terminal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let policy = CapabilityPolicy::new("policy-v1").with_max_semantic_payload_bytes(16);
    let handler =
        CapabilityHandler::open(dir.path().join("outcomes.sqlite3"), policy).expect("open handler");
    let actor = actor("cycle-oversized-1", [CapabilityGrant::RecordNoAction]);
    let oversized = vec![b'x'; 17];

    let error = handler
        .record_no_action(
            &actor,
            RecordNoActionRequest {
                identity: identity("request-oversized-1", "cycle-oversized-1"),
                reason: OpaqueBytes::from(oversized.clone()),
                raw_semantic: OpaqueBytes::from(b"raw".to_vec()),
                evidence: Vec::new(),
            },
        )
        .expect_err("oversized payload must fail rather than truncate");

    assert_eq!(error.code(), CapabilityErrorCode::PayloadTooLarge);
    assert_eq!(
        handler
            .terminal_count("session-4052", "cycle-oversized-1")
            .expect("terminal count"),
        0
    );
}

#[test]
fn progress_is_separate_and_does_not_satisfy_the_terminal_requirement() {
    let (_dir, handler) = handler();
    let progress_actor = actor("cycle-progress-1", [CapabilityGrant::RecordProgress]);
    let request = RecordProgressRequest {
        identity: identity("request-progress-1", "cycle-progress-1"),
        percent: 42,
        summary: OpaqueBytes::from(b"typed progress summary".to_vec()),
        evidence: vec![EvidenceRef::Commit {
            repository: RepositoryRef::new("rysweet", "Simard"),
            sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
        }],
    };

    let progress = handler
        .record_progress(&progress_actor, request)
        .expect("authorized progress");

    assert_eq!(progress.percent, 42);
    assert_eq!(
        handler
            .terminal_count("session-4052", "cycle-progress-1")
            .expect("count terminals"),
        0,
        "progress is durable evidence, not an Act terminal"
    );

    let invalid_actor = actor("cycle-progress-2", [CapabilityGrant::RecordProgress]);
    let invalid = handler
        .record_progress(
            &invalid_actor,
            RecordProgressRequest {
                identity: identity("request-progress-invalid", "cycle-progress-2"),
                percent: 101,
                summary: OpaqueBytes::from(b"invalid percentage".to_vec()),
                evidence: Vec::new(),
            },
        )
        .expect_err("progress above 100 must fail");
    assert_eq!(invalid.code(), CapabilityErrorCode::InvalidArgument);
}

#[test]
fn persistence_open_failure_is_not_converted_to_a_business_outcome() {
    let dir = tempfile::tempdir().expect("tempdir");
    let error = CapabilityHandler::open(dir.path(), policy())
        .expect_err("a directory cannot be opened as the ledger database");

    assert_eq!(error.code(), CapabilityErrorCode::PersistenceFailed);
}

#[test]
fn expired_running_effect_is_recovered_under_the_original_request_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("outcomes.sqlite3");
    let actor = actor(
        "cycle-recovery-1",
        [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
    );
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    let outcome = {
        let handler = CapabilityHandler::open(&path, policy()).expect("open handler");
        let outcome = handler
            .record_action(
                &actor,
                action_request(
                    "request-recovery-1",
                    "cycle-recovery-1",
                    b"recover this exact effect".to_vec(),
                ),
                &admitted(),
            )
            .expect("record action");
        let claimed = handler
            .claim_next_effect(
                "worker-before-crash",
                "request-claim-before-crash",
                now,
                Duration::from_secs(30),
            )
            .expect("claim effect")
            .expect("pending effect");
        assert_eq!(claimed.request_id, "request-recovery-1");
        assert_eq!(claimed.state.as_str(), "running");
        outcome
    };

    let handler = CapabilityHandler::open(&path, policy()).expect("reopen after crash");
    let recovered = handler
        .recover_expired_effects("request-recover-after-crash", now + Duration::from_secs(31))
        .expect("recover expired leases");
    assert_eq!(recovered, 1);

    let effect = handler
        .effect_for_outcome(&outcome.outcome_id)
        .expect("effect query")
        .expect("effect remains durable");
    assert_eq!(effect.request_id, "request-recovery-1");
    assert_eq!(effect.state.as_str(), "indeterminate");
    assert_eq!(
        handler
            .terminal_count("session-4052", "cycle-recovery-1")
            .expect("terminal count"),
        1,
        "recovery must not ask the actor for a replacement terminal"
    );
}
