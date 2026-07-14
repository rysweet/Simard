//! Contract tests for all non-spawn terminal variants and typed action requests.
//!
//! TDD status: RED until the typed OODA capability implementation exists.

use std::collections::BTreeSet;

use simard::typed_ooda::{
    Action, ActionKind, AdmissionSnapshot, ArtifactRef, AuthenticatedToolContext, BackupPolicy,
    BlockerRef, CapabilityErrorCode, CapabilityGrant, CapabilityHandler, CapabilityPolicy,
    CompletionRef, EnvironmentRef, EvidenceRef, FileIssueAction, OpaqueBytes, PullRequestRef,
    RecordActionRequest, RecordBlockedRequest, RecordCompletedRequest, RepositoryRef,
    RequestDeployAction, RequestMergeAction, RetryPolicy, TerminalKind, TerminalRequestIdentity,
};

fn identity(request_id: &str, cycle_id: &str) -> TerminalRequestIdentity {
    TerminalRequestIdentity::new(request_id, "session-variants", cycle_id, "goal-4052")
}

fn actor(
    cycle_id: &str,
    grants: impl IntoIterator<Item = CapabilityGrant>,
) -> AuthenticatedToolContext {
    AuthenticatedToolContext::new("goal-session-actor", "session-variants", grants)
        .scoped_to_repository(simard::typed_ooda::RepositoryRef::new("rysweet", "Simard"))
        .bound_to_cycle_goal(cycle_id, "goal-4052")
}

fn handler() -> (tempfile::TempDir, CapabilityHandler) {
    let dir = tempfile::tempdir().expect("tempdir");
    let handler = CapabilityHandler::open(
        dir.path().join("outcomes.sqlite3"),
        CapabilityPolicy::new("policy-v1"),
    )
    .expect("handler");
    (dir, handler)
}

fn admitted() -> AdmissionSnapshot {
    AdmissionSnapshot {
        concurrent_engineers: 0,
        disk_used_percent: 5,
        active_claims: BTreeSet::new(),
        policy_revision: "admission-v1".to_string(),
    }
}

#[test]
fn blocked_terminal_preserves_reason_blocker_and_retry_policy() {
    let (_dir, handler) = handler();
    let actor = actor("cycle-blocked-1", [CapabilityGrant::RecordBlocked]);
    let reason = b"Waiting for credential rotation.\nNO ACTION is not a protocol.".to_vec();
    let request = RecordBlockedRequest {
        identity: identity("request-blocked-1", "cycle-blocked-1"),
        reason: OpaqueBytes::from(reason.clone()),
        blocker: BlockerRef::Credential {
            name: "SIMARD_DEPLOY_TOKEN".to_string(),
        },
        retry: RetryPolicy::AfterSignal {
            provider: "secret-store".to_string(),
            signal_id: "rotation-complete".to_string(),
        },
        raw_semantic: OpaqueBytes::from(vec![0xff, 0x00, b'B']),
        evidence: Vec::new(),
    };

    let outcome = handler
        .record_blocked(&actor, request)
        .expect("record blocked");

    assert_eq!(outcome.kind, TerminalKind::Blocked);
    let payload = outcome.payload.blocked().expect("blocked payload");
    assert_eq!(payload.reason.as_bytes(), reason);
    assert!(matches!(payload.blocker, BlockerRef::Credential { .. }));
    assert!(matches!(payload.retry, RetryPolicy::AfterSignal { .. }));
}

#[test]
fn completed_terminal_requires_typed_evidence_and_never_accepts_prose_as_proof() {
    let (_dir, handler) = handler();
    let completed_actor = actor("cycle-completed-1", [CapabilityGrant::RecordCompleted]);
    let repository = RepositoryRef::new("rysweet", "Simard");
    let evidence = EvidenceRef::CheckRun {
        repository: repository.clone(),
        check_id: 4052,
        conclusion: "success".to_string(),
    };
    let request = RecordCompletedRequest {
        identity: identity("request-completed-1", "cycle-completed-1"),
        summary: OpaqueBytes::from(b"All done, exactly as written.\n".to_vec()),
        completion: CompletionRef {
            criterion_id: "typed-ooda-live-cycle".to_string(),
            verification_evidence: vec![evidence.clone()],
        },
        raw_semantic: OpaqueBytes::from(b"semantic completion rationale".to_vec()),
        evidence: vec![evidence],
    };

    let outcome = handler
        .record_completed(&completed_actor, request)
        .expect("typed completion evidence");
    assert_eq!(outcome.kind, TerminalKind::Completed);

    let missing_actor = actor("cycle-completed-2", [CapabilityGrant::RecordCompleted]);
    let missing = handler
        .record_completed(
            &missing_actor,
            RecordCompletedRequest {
                identity: identity("request-completed-2", "cycle-completed-2"),
                summary: OpaqueBytes::from(
                    b"Model prose claims every check passed; that is not evidence.".to_vec(),
                ),
                completion: CompletionRef {
                    criterion_id: "typed-ooda-live-cycle".to_string(),
                    verification_evidence: Vec::new(),
                },
                raw_semantic: OpaqueBytes::from(b"looks convincing".to_vec()),
                evidence: Vec::new(),
            },
        )
        .expect_err("prose alone cannot satisfy deterministic evidence gates");
    assert_eq!(missing.code(), CapabilityErrorCode::StateTransitionRejected);
}

#[test]
fn file_issue_action_commits_a_request_without_scraping_identifiers_from_text() {
    let (_dir, handler) = handler();
    let actor = actor(
        "cycle-issue-1",
        [CapabilityGrant::RecordAction(ActionKind::FileIssue)],
    );
    let action = Action::FileIssue(FileIssueAction {
        repository: RepositoryRef::new("rysweet", "Simard"),
        title: OpaqueBytes::from(b"Track typed OODA rollout".to_vec()),
        body: OpaqueBytes::from(
            b"References https://example.invalid/issues/999 only as opaque body text.".to_vec(),
        ),
        labels: vec!["ooda".to_string()],
    });

    let outcome = handler
        .record_action(
            &actor,
            RecordActionRequest {
                identity: identity("request-issue-1", "cycle-issue-1"),
                action,
                raw_semantic: OpaqueBytes::from(b"semantic issue rationale".to_vec()),
                evidence: Vec::new(),
            },
            &admitted(),
        )
        .expect("record issue action");

    assert_eq!(outcome.kind, TerminalKind::Action);
    assert_eq!(
        outcome.payload.action().expect("action").kind(),
        ActionKind::FileIssue
    );
}

#[test]
fn merge_and_deploy_actions_create_requests_but_do_not_execute_privileged_effects() {
    let (_dir, handler) = handler();
    let merge_actor = actor(
        "cycle-merge-1",
        [
            CapabilityGrant::RecordAction(ActionKind::RequestMerge),
            CapabilityGrant::RecordAction(ActionKind::RequestDeploy),
        ],
    );

    let merge = handler
        .record_action(
            &merge_actor,
            RecordActionRequest {
                identity: identity("request-merge-1", "cycle-merge-1"),
                action: Action::RequestMerge(RequestMergeAction {
                    pull_request: PullRequestRef {
                        repository: RepositoryRef::new("rysweet", "Simard"),
                        number: 4052,
                    },
                    expected_head_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
                    strategy: "squash".to_string(),
                }),
                raw_semantic: OpaqueBytes::from(b"request merge, do not merge directly".to_vec()),
                evidence: Vec::new(),
            },
            &admitted(),
        )
        .expect("record merge request");
    assert_eq!(
        handler
            .effect_for_outcome(&merge.outcome_id)
            .expect("effect query")
            .expect("merge request effect")
            .kind
            .as_str(),
        "request_merge"
    );

    let deploy_actor = actor(
        "cycle-deploy-1",
        [
            CapabilityGrant::RecordAction(ActionKind::RequestMerge),
            CapabilityGrant::RecordAction(ActionKind::RequestDeploy),
        ],
    );
    let deploy = handler
        .record_action(
            &deploy_actor,
            RecordActionRequest {
                identity: identity("request-deploy-1", "cycle-deploy-1"),
                action: Action::RequestDeploy(RequestDeployAction {
                    artifact: ArtifactRef {
                        digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .to_string(),
                        source_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
                    },
                    environment: EnvironmentRef::new("production"),
                    backup_policy: BackupPolicy::VerifiedFull,
                }),
                raw_semantic: OpaqueBytes::from(b"request deploy, do not deploy directly".to_vec()),
                evidence: Vec::new(),
            },
            &admitted(),
        )
        .expect("record deploy request");
    assert_eq!(
        handler
            .effect_for_outcome(&deploy.outcome_id)
            .expect("effect query")
            .expect("deploy request effect")
            .kind
            .as_str(),
        "request_deploy"
    );
}

#[test]
fn a_goal_session_actor_cannot_use_progress_or_direct_merge_deploy_permissions() {
    let policy = CapabilityPolicy::goal_session_default("goal-session-policy-v1");

    assert!(policy.allows(CapabilityGrant::RecordBlocked));
    assert!(policy.allows(CapabilityGrant::RecordCompleted));
    assert!(policy.allows(CapabilityGrant::RecordNoAction));
    assert!(policy.allows(CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)));
    assert!(!policy.allows(CapabilityGrant::RecordProgress));
    assert!(!policy.allows(CapabilityGrant::DirectMerge));
    assert!(!policy.allows(CapabilityGrant::DirectDeploy));
}
