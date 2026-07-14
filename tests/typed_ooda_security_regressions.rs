use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, SystemTime};

use simard::typed_ooda::{
    Action, ActionKind, AdmissionSnapshot, AuthenticatedToolContext, BaseType, CapabilityErrorCode,
    CapabilityGrant, CapabilityHandler, CapabilityPolicy, EffectResult, OpaqueBytes,
    ProcessExecRequest, ProcessExecutionStatus, RecordActionRequest, RecordNoActionRequest,
    RecordProgressRequest, RepositoryRef, SpawnEngineerAction, TerminalRequestIdentity,
};

fn identity(request_id: &str, cycle_id: &str, goal_id: &str) -> TerminalRequestIdentity {
    TerminalRequestIdentity::new(request_id, "session-security", cycle_id, goal_id)
}

fn policy() -> CapabilityPolicy {
    CapabilityPolicy::new("security-v1").with_process_exec_mutations_per_cycle(1)
}

fn actor(
    cycle_id: &str,
    goal_id: &str,
    grants: impl IntoIterator<Item = CapabilityGrant>,
) -> AuthenticatedToolContext {
    AuthenticatedToolContext::new("goal-session-actor", "session-security", grants)
        .scoped_to_repository(RepositoryRef::new("rysweet", "Simard"))
        .bound_to_cycle_goal(cycle_id, goal_id)
        .with_engineer_permissions(["repo_read", "repo_write", "process_exec"])
}

fn handler(path: &std::path::Path) -> CapabilityHandler {
    CapabilityHandler::open(path, policy()).expect("open typed OODA ledger")
}

fn no_action(request_id: &str, cycle_id: &str, goal_id: &str) -> RecordNoActionRequest {
    RecordNoActionRequest {
        identity: identity(request_id, cycle_id, goal_id),
        reason: OpaqueBytes::from(b"no mutation needed".to_vec()),
        raw_semantic: OpaqueBytes::from(b"semantic bytes".to_vec()),
        evidence: Vec::new(),
    }
}

fn progress(request_id: &str, cycle_id: &str, goal_id: &str) -> RecordProgressRequest {
    RecordProgressRequest {
        identity: identity(request_id, cycle_id, goal_id),
        percent: 25,
        summary: OpaqueBytes::from(b"one quarter complete".to_vec()),
        evidence: Vec::new(),
    }
}

fn spawn_request(
    request_id: &str,
    cycle_id: &str,
    goal_id: &str,
    base_type: BaseType,
    permissions: &[&str],
) -> RecordActionRequest {
    RecordActionRequest {
        identity: identity(request_id, cycle_id, goal_id),
        action: Action::SpawnEngineer(SpawnEngineerAction {
            task: OpaqueBytes::from(b"implement the scoped task".to_vec()),
            repository: RepositoryRef::new("rysweet", "Simard"),
            base_type,
            requested_permissions: permissions
                .iter()
                .map(|permission| (*permission).to_string())
                .collect(),
            claim_key: format!("rysweet/Simard:{goal_id}"),
        }),
        raw_semantic: OpaqueBytes::from(b"typed action".to_vec()),
        evidence: Vec::new(),
    }
}

fn admitted() -> AdmissionSnapshot {
    AdmissionSnapshot {
        concurrent_engineers: 0,
        disk_used_percent: 10,
        active_claims: BTreeSet::new(),
        policy_revision: "security-v1".to_string(),
    }
}

#[test]
fn authenticated_actor_is_bound_to_exact_cycle_and_goal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let handler = handler(&dir.path().join("ledger.sqlite3"));
    let source = actor(
        "cycle-bound",
        "goal-bound",
        [CapabilityGrant::RecordNoAction],
    );
    let lease = handler
        .register_actor_session(
            &source,
            "request-register-bound-actor",
            "cycle-bound",
            "goal-bound",
            Duration::from_secs(60),
        )
        .expect("register actor");
    let authenticated = handler
        .authenticate_actor_session(
            &lease.token,
            "session-security",
            "cycle-bound",
            "goal-bound",
        )
        .expect("authenticate actor");

    for request in [
        no_action("request-cross-cycle", "cycle-other", "goal-bound"),
        no_action("request-cross-goal", "cycle-bound", "goal-other"),
    ] {
        let error = handler
            .record_no_action(&authenticated, request)
            .expect_err("server-bound target mismatch must fail");
        assert_eq!(
            error.code(),
            CapabilityErrorCode::AuthorizationScopeViolation
        );
    }
    let unbound = AuthenticatedToolContext::new(
        "goal-session-actor",
        "session-security",
        [CapabilityGrant::RecordNoAction],
    )
    .scoped_to_repository(RepositoryRef::new("rysweet", "Simard"));
    let error = handler
        .record_no_action(
            &unbound,
            no_action("request-unbound", "cycle-bound", "goal-bound"),
        )
        .expect_err("caller-supplied unbound context must not authorize a mutation");
    assert_eq!(
        error.code(),
        CapabilityErrorCode::AuthorizationScopeViolation
    );
    assert_eq!(
        handler
            .terminal_count("session-security", "cycle-bound")
            .expect("terminal count"),
        0
    );
}

#[test]
fn actor_session_registration_is_idempotent_and_cannot_rebind_scope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let handler = handler(&dir.path().join("ledger.sqlite3"));
    let source = actor(
        "cycle-bound",
        "goal-bound",
        [CapabilityGrant::RecordNoAction],
    );
    let first = handler
        .register_actor_session(
            &source,
            "request-register-actor",
            "cycle-bound",
            "goal-bound",
            Duration::from_secs(60),
        )
        .expect("register actor");
    let replay = handler
        .register_actor_session(
            &source,
            "request-register-actor",
            "cycle-bound",
            "goal-bound",
            Duration::from_secs(60),
        )
        .expect("identical registration replay");
    assert_eq!(replay, first);

    let missing = handler
        .register_actor_session(
            &source,
            "",
            "cycle-bound",
            "goal-bound",
            Duration::from_secs(60),
        )
        .expect_err("registration request id is required");
    assert_eq!(missing.code(), CapabilityErrorCode::InvalidIdentifier);

    let rebound = handler
        .register_actor_session(
            &source,
            "request-register-other-cycle",
            "cycle-other",
            "goal-bound",
            Duration::from_secs(60),
        )
        .expect_err("one session cannot be rebound to another cycle");
    assert_eq!(
        rebound.code(),
        CapabilityErrorCode::AuthorizationScopeViolation
    );
    let broader = actor(
        "cycle-bound",
        "goal-bound",
        [
            CapabilityGrant::RecordNoAction,
            CapabilityGrant::RecordProgress,
        ],
    );
    let escalated = handler
        .register_actor_session(
            &broader,
            "request-register-broader-actor",
            "cycle-bound",
            "goal-bound",
            Duration::from_secs(60),
        )
        .expect_err("an existing session cannot be rebound to broader grants");
    assert_eq!(
        escalated.code(),
        CapabilityErrorCode::AuthorizationScopeViolation
    );

    let cross_type = handler
        .record_no_action(
            &source,
            no_action("request-register-actor", "cycle-bound", "goal-bound"),
        )
        .expect_err("registration request id cannot be reused for a terminal");
    assert_eq!(cross_type.code(), CapabilityErrorCode::RequestConflict);
    handler
        .authenticate_actor_session(
            &first.token,
            "session-security",
            "cycle-bound",
            "goal-bound",
        )
        .expect("original binding remains valid");
}

#[test]
fn ambiguous_legacy_actor_bindings_are_rejected_during_migration() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ledger.sqlite3");
    let connection = rusqlite::Connection::open(&path).expect("legacy database");
    connection
        .execute_batch(
            "
            CREATE TABLE actor_sessions (
                session_id TEXT NOT NULL,
                cycle_id TEXT NOT NULL,
                goal_id TEXT NOT NULL,
                actor_identity TEXT NOT NULL,
                repository_json BLOB NOT NULL,
                grants_json BLOB NOT NULL,
                observe_only INTEGER NOT NULL,
                token_hash TEXT NOT NULL,
                expires_at INTEGER NOT NULL,
                PRIMARY KEY(session_id, cycle_id)
            );
            ",
        )
        .expect("legacy schema");
    let repository =
        serde_json::to_vec(&RepositoryRef::new("rysweet", "Simard")).expect("repository");
    let grants =
        serde_json::to_vec(&BTreeSet::from([CapabilityGrant::RecordNoAction])).expect("grants");
    for cycle in ["legacy-cycle-a", "legacy-cycle-b"] {
        connection
            .execute(
                "INSERT INTO actor_sessions(
                    session_id, cycle_id, goal_id, actor_identity, repository_json,
                    grants_json, observe_only, token_hash, expires_at
                 ) VALUES (?1, ?2, 'legacy-goal', 'goal-session-actor', ?3, ?4, 0, 'hash', ?5)",
                rusqlite::params!["legacy-session", cycle, repository, grants, i64::MAX],
            )
            .expect("legacy actor row");
    }
    drop(connection);

    let handler = handler(&path);
    let rejected = handler
        .authenticate_actor_session("token", "legacy-session", "legacy-cycle-a", "legacy-goal")
        .expect_err("ambiguous legacy session must not retain either binding");
    assert_eq!(rejected.code(), CapabilityErrorCode::Unauthenticated);
}

#[test]
fn request_ids_are_required_and_unique_across_mutation_types() {
    let dir = tempfile::tempdir().expect("tempdir");
    let handler = handler(&dir.path().join("ledger.sqlite3"));
    let actor = actor(
        "cycle-cross-type",
        "goal-1",
        [
            CapabilityGrant::RecordProgress,
            CapabilityGrant::RecordNoAction,
        ],
    );

    let missing = handler
        .record_progress(&actor, progress("", "cycle-missing-id", "goal-1"))
        .expect_err("empty request id must fail");
    assert_eq!(missing.code(), CapabilityErrorCode::InvalidIdentifier);

    handler
        .record_progress(
            &actor,
            progress("request-cross-type", "cycle-cross-type", "goal-1"),
        )
        .expect("first mutation");
    let conflict = handler
        .record_no_action(
            &actor,
            no_action("request-cross-type", "cycle-cross-type", "goal-1"),
        )
        .expect_err("request id reuse across mutation types must fail");
    assert_eq!(conflict.code(), CapabilityErrorCode::RequestConflict);
    assert_eq!(
        handler
            .terminal_count("session-security", "cycle-cross-type")
            .expect("terminal count"),
        0
    );
}

#[test]
fn concurrent_identical_progress_replays_return_one_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ledger.sqlite3");
    drop(handler(&path));
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        threads.push(thread::spawn(move || {
            let handler = handler(&path);
            let actor = actor(
                "cycle-progress-race",
                "goal-1",
                [CapabilityGrant::RecordProgress],
            );
            barrier.wait();
            handler.record_progress(
                &actor,
                progress("request-progress-race", "cycle-progress-race", "goal-1"),
            )
        }));
    }
    let records: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().expect("thread").expect("identical replay"))
        .collect();
    assert_eq!(records[0], records[1]);
    assert_eq!(
        handler(&path)
            .progress_for_cycle("session-security", "cycle-progress-race")
            .expect("progress query")
            .len(),
        1
    );
}

#[test]
fn concurrent_terminal_replays_serialize_one_winner() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ledger.sqlite3");
    drop(handler(&path));
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        threads.push(thread::spawn(move || {
            let handler = handler(&path);
            let actor = actor(
                "cycle-terminal-race",
                "goal-1",
                [CapabilityGrant::RecordNoAction],
            );
            barrier.wait();
            handler.record_no_action(
                &actor,
                no_action("request-terminal-race", "cycle-terminal-race", "goal-1"),
            )
        }));
    }
    let outcomes: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().expect("thread").expect("identical replay"))
        .collect();
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(
        handler(&path)
            .terminal_count("session-security", "cycle-terminal-race")
            .expect("terminal count"),
        1
    );
}

#[test]
fn concurrent_incompatible_terminals_leave_exactly_one_transition() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ledger.sqlite3");
    drop(handler(&path));
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for request_id in ["request-terminal-a", "request-terminal-b"] {
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        threads.push(thread::spawn(move || {
            let handler = handler(&path);
            let actor = actor(
                "cycle-terminal-conflict",
                "goal-1",
                [CapabilityGrant::RecordNoAction],
            );
            barrier.wait();
            handler.record_no_action(
                &actor,
                no_action(request_id, "cycle-terminal-conflict", "goal-1"),
            )
        }));
    }
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().expect("thread"))
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    assert_eq!(
        handler(&path)
            .terminal_count("session-security", "cycle-terminal-conflict")
            .expect("terminal count"),
        1
    );
}

#[test]
fn effect_completion_retry_and_failure_are_fenced_by_owner_generation_and_expiry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let handler = handler(&dir.path().join("ledger.sqlite3"));
    let actor = actor(
        "cycle-effect",
        "goal-effect",
        [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
    );
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
    let outcome = handler
        .record_action(
            &actor,
            spawn_request(
                "request-effect",
                "cycle-effect",
                "goal-effect",
                BaseType::Copilot,
                &["repo_read"],
            ),
            &admitted(),
        )
        .expect("record action");
    let lease = handler
        .claim_next_effect(
            "worker-a",
            "request-effect-claim",
            now,
            Duration::from_secs(30),
        )
        .expect("claim")
        .expect("effect");
    let cross_type = handler
        .recover_expired_effects("request-effect-claim", now)
        .expect_err("claim request id cannot be reused by recovery");
    assert_eq!(cross_type.code(), CapabilityErrorCode::RequestConflict);

    let mut foreign = lease.clone();
    foreign.lease_owner = Some("worker-b".to_string());
    let error = handler
        .finish_effect(
            &foreign,
            "request-effect-foreign-finish",
            now + Duration::from_secs(1),
            &EffectResult::Succeeded {
                evidence: Vec::new(),
            },
        )
        .expect_err("foreign owner must fail");
    assert_eq!(error.code(), CapabilityErrorCode::StaleLease);
    let error = handler
        .release_effect_for_retry(
            &foreign,
            "request-effect-foreign-retry",
            now + Duration::from_secs(1),
            "foreign retry",
        )
        .expect_err("foreign owner retry must fail");
    assert_eq!(error.code(), CapabilityErrorCode::StaleLease);

    let mut stale_generation = lease.clone();
    stale_generation.lease_generation += 1;
    let error = handler
        .release_effect_for_retry(
            &stale_generation,
            "request-effect-stale-retry",
            now + Duration::from_secs(1),
            "retry",
        )
        .expect_err("wrong generation must fail");
    assert_eq!(error.code(), CapabilityErrorCode::StaleLease);
    let error = handler
        .finish_effect(
            &stale_generation,
            "request-effect-generation-failure",
            now + Duration::from_secs(1),
            &EffectResult::Failed {
                error: "wrong generation".to_string(),
            },
        )
        .expect_err("wrong-generation failure must fail");
    assert_eq!(error.code(), CapabilityErrorCode::StaleLease);

    let error = handler
        .finish_effect(
            &lease,
            "request-effect-expired-finish",
            now + Duration::from_secs(31),
            &EffectResult::Failed {
                error: "late".to_string(),
            },
        )
        .expect_err("expired lease must fail");
    assert_eq!(error.code(), CapabilityErrorCode::StaleLease);
    let error = handler
        .release_effect_for_retry(
            &lease,
            "request-effect-expired-retry",
            now + Duration::from_secs(31),
            "late retry",
        )
        .expect_err("expired lease retry must fail");
    assert_eq!(error.code(), CapabilityErrorCode::StaleLease);
    assert_eq!(
        handler
            .effect_for_outcome(&outcome.outcome_id)
            .expect("effect query")
            .expect("effect")
            .state
            .as_str(),
        "running"
    );
}

#[test]
fn engineer_dispatch_requires_copilot_base_type_and_permission_intersection() {
    let dir = tempfile::tempdir().expect("tempdir");
    let handler = handler(&dir.path().join("ledger.sqlite3"));
    let scoped_actor = actor(
        "cycle-wrong-type",
        "goal-wrong-type",
        [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
    )
    .with_engineer_permissions(["repo_read"]);

    let invalid_type = handler
        .record_action(
            &scoped_actor,
            spawn_request(
                "request-wrong-type",
                "cycle-wrong-type",
                "goal-wrong-type",
                BaseType::RustyClawd,
                &["repo_read"],
            ),
            &admitted(),
        )
        .expect_err("noncanonical engineer base type must fail");
    assert_eq!(
        invalid_type.code(),
        CapabilityErrorCode::AuthorizationScopeViolation
    );

    let broad_actor = actor(
        "cycle-broad-perms",
        "goal-broad-perms",
        [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
    )
    .with_engineer_permissions(["repo_read"]);
    let too_broad = handler
        .record_action(
            &broad_actor,
            spawn_request(
                "request-broad-perms",
                "cycle-broad-perms",
                "goal-broad-perms",
                BaseType::Copilot,
                &["repo_read", "repo_write"],
            ),
            &admitted(),
        )
        .expect_err("permissions outside actor scope must fail");
    assert_eq!(
        too_broad.code(),
        CapabilityErrorCode::AuthorizationScopeViolation
    );

    let unknown_actor = actor(
        "cycle-unknown-perms",
        "goal-unknown-perms",
        [CapabilityGrant::RecordAction(ActionKind::SpawnEngineer)],
    )
    .with_engineer_permissions(["repo_read", "unknown_permission"]);
    let unknown = handler
        .record_action(
            &unknown_actor,
            spawn_request(
                "request-unknown-perms",
                "cycle-unknown-perms",
                "goal-unknown-perms",
                BaseType::Copilot,
                &["unknown_permission"],
            ),
            &admitted(),
        )
        .expect_err("unknown base-type permission must fail before admission");
    assert_eq!(
        unknown.code(),
        CapabilityErrorCode::AuthorizationScopeViolation
    );
}

#[test]
fn process_exec_replays_and_concurrency_cannot_bypass_or_double_spend_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ledger.sqlite3");
    let output = dir.path().join("executions.txt");
    drop(handler(&path));
    let process_actor = actor(
        "cycle-process",
        "goal-process",
        [CapabilityGrant::ProcessExec],
    )
    .scoped_to_working_directory(dir.path());
    let request = ProcessExecRequest {
        identity: identity("request-process-1", "cycle-process", "goal-process"),
        program: PathBuf::from("/bin/sh"),
        args: vec![
            "-c".to_string(),
            format!("printf executed >> {}", output.display()),
        ],
        working_directory: dir.path().to_path_buf(),
    };

    let first = handler(&path)
        .execute_process(&process_actor, request.clone())
        .expect("first process");
    assert_eq!(first.status, ProcessExecutionStatus::Completed);
    let replay = handler(&path)
        .execute_process(&process_actor, request.clone())
        .expect("identical replay");
    assert_eq!(replay, first);
    assert_eq!(
        std::fs::read_to_string(&output).expect("execution marker"),
        "executed"
    );

    let mut alternate_shape = request.clone();
    alternate_shape.args.push("ignored".to_string());
    let conflict = handler(&path)
        .execute_process(&process_actor, alternate_shape)
        .expect_err("same id with alternate payload must conflict");
    assert_eq!(conflict.code(), CapabilityErrorCode::RequestConflict);

    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for suffix in ["a", "b"] {
        let path = path.clone();
        let root = dir.path().to_path_buf();
        let barrier = Arc::clone(&barrier);
        threads.push(thread::spawn(move || {
            let actor = actor(
                "cycle-process-race",
                "goal-process",
                [CapabilityGrant::ProcessExec],
            )
            .scoped_to_working_directory(root.clone());
            let request = ProcessExecRequest {
                identity: identity(
                    &format!("request-process-{suffix}"),
                    "cycle-process-race",
                    "goal-process",
                ),
                program: PathBuf::from("/bin/true"),
                args: Vec::new(),
                working_directory: root,
            };
            let handler = handler(&path);
            barrier.wait();
            handler.execute_process(&actor, request)
        }));
    }

    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().expect("thread"))
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let exhausted = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("one request must exhaust cap");
    assert_eq!(exhausted.code(), CapabilityErrorCode::MutationCapExhausted);
}

#[test]
fn process_exec_failed_calls_spend_the_cap_and_unknown_wire_shapes_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ledger.sqlite3");
    let handler = handler(&path);
    let source_actor = actor(
        "cycle-process-failed",
        "goal-process",
        [CapabilityGrant::ProcessExec],
    )
    .scoped_to_working_directory(dir.path());
    let lease = handler
        .register_actor_session(
            &source_actor,
            "request-register-process-actor",
            "cycle-process-failed",
            "goal-process",
            Duration::from_secs(60),
        )
        .expect("register process actor");
    let process_actor = handler
        .authenticate_actor_session(
            &lease.token,
            "session-security",
            "cycle-process-failed",
            "goal-process",
        )
        .expect("authenticate process actor");
    let failed = ProcessExecRequest {
        identity: identity(
            "request-process-failed",
            "cycle-process-failed",
            "goal-process",
        ),
        program: PathBuf::from("/bin/false"),
        args: Vec::new(),
        working_directory: dir.path().to_path_buf(),
    };
    assert_eq!(
        handler
            .execute_process(&process_actor, failed)
            .expect("failed command is still a recorded execution")
            .status,
        ProcessExecutionStatus::Failed
    );
    let exhausted = handler
        .execute_process(
            &process_actor,
            ProcessExecRequest {
                identity: identity(
                    "request-process-after-failure",
                    "cycle-process-failed",
                    "goal-process",
                ),
                program: PathBuf::from("/bin/true"),
                args: Vec::new(),
                working_directory: dir.path().to_path_buf(),
            },
        )
        .expect_err("a failed execution still spends the scoped cap");
    assert_eq!(exhausted.code(), CapabilityErrorCode::MutationCapExhausted);

    let wire = format!(
        r#"{{
            "identity": {{
                "request_id": "request-wire",
                "session_id": "session-security",
                "cycle_id": "cycle-wire",
                "goal_id": "goal-process"
            }},
            "program": "/bin/true",
            "args": [],
            "working_directory": {},
            "unaccounted_retry": true
        }}"#,
        serde_json::to_string(dir.path()).expect("path JSON")
    );
    serde_json::from_str::<ProcessExecRequest>(&wire)
        .expect_err("alternate request fields must not be normalized away");
}

#[test]
fn concurrent_duplicate_process_requests_execute_once_and_spend_one_slot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ledger.sqlite3");
    let marker = dir.path().join("duplicate-execution.txt");
    drop(handler(&path));
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let path = path.clone();
        let root = dir.path().to_path_buf();
        let marker = marker.clone();
        let barrier = Arc::clone(&barrier);
        threads.push(thread::spawn(move || {
            let actor = actor(
                "cycle-process-duplicate",
                "goal-process",
                [CapabilityGrant::ProcessExec],
            )
            .scoped_to_working_directory(root.clone());
            let request = ProcessExecRequest {
                identity: identity(
                    "request-process-duplicate",
                    "cycle-process-duplicate",
                    "goal-process",
                ),
                program: PathBuf::from("/bin/sh"),
                args: vec![
                    "-c".to_string(),
                    format!(
                        "sleep 0.05; printf '%s\\n' executed >> {}",
                        marker.display()
                    ),
                ],
                working_directory: root,
            };
            barrier.wait();
            handler(&path).execute_process(&actor, request)
        }));
    }
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().expect("thread").expect("duplicate replay"))
        .collect();
    assert!(
        results
            .iter()
            .any(|result| result.status == ProcessExecutionStatus::Completed)
    );
    assert_eq!(
        std::fs::read_to_string(&marker).expect("execution marker"),
        "executed\n"
    );

    let process_actor = actor(
        "cycle-process-duplicate",
        "goal-process",
        [CapabilityGrant::ProcessExec],
    )
    .scoped_to_working_directory(dir.path());
    let exhausted = handler(&path)
        .execute_process(
            &process_actor,
            ProcessExecRequest {
                identity: identity(
                    "request-process-second",
                    "cycle-process-duplicate",
                    "goal-process",
                ),
                program: PathBuf::from("/bin/true"),
                args: Vec::new(),
                working_directory: dir.path().to_path_buf(),
            },
        )
        .expect_err("duplicate replay must not create an extra cap slot");
    assert_eq!(exhausted.code(), CapabilityErrorCode::MutationCapExhausted);
}
