use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use crate::goal_curation::{ActiveGoal, ArtifactKind, BacklogItem, GoalBoard, promote_to_active};
use crate::overseer::sensor::detect_workstream_gaps;
use crate::stewardship::gh_client::{GhIssue, IssueMutationTransport};
use crate::stewardship::mutation_guard::MutationGuard;
use crate::stewardship::mutation_store::MutationStore;
use crate::stewardship::types::{
    ArtifactProvenance, CycleId, IssueMutation, IssueMutationIdentity, IssueMutationLimit,
    IssueMutationOutcome, IssueMutationRequest, LineageId,
};

#[derive(Clone, Default)]
struct FakeTransport {
    created: Arc<Mutex<Vec<IssueMutationIdentity>>>,
}

impl FakeTransport {
    fn create_count(&self) -> usize {
        self.created.lock().unwrap().len()
    }
}

impl IssueMutationTransport for FakeTransport {
    fn create_issue(
        &self,
        repo: &str,
        identity: &IssueMutationIdentity,
        title: &str,
        body: &str,
        _labels: &[String],
        _assignees: &[String],
    ) -> crate::error::SimardResult<GhIssue> {
        self.created.lock().unwrap().push(identity.clone());
        Ok(GhIssue {
            number: 101 + self.create_count() as u64,
            url: format!(
                "https://github.com/{repo}/issues/{}",
                100 + self.create_count()
            ),
            title: title.to_string(),
            body: body.to_string(),
        })
    }
}

fn operator_provenance(lineage: &str) -> ArtifactProvenance {
    ArtifactProvenance::operator(LineageId::new(lineage).unwrap())
}

fn stewardship_provenance(lineage: &str) -> ArtifactProvenance {
    ArtifactProvenance::stewardship(LineageId::new(lineage).unwrap())
}

fn request(identity: &str) -> IssueMutationRequest {
    IssueMutationRequest::create(
        "rysweet/Simard",
        IssueMutationIdentity::new(identity).unwrap(),
        operator_provenance("operator-request"),
        format!("Issue for {identity}"),
        "durable finding",
    )
    .unwrap()
}

fn store(temp: &TempDir) -> MutationStore {
    MutationStore::new(temp.path().join("issue-mutations.json"))
}

#[test]
fn journal_initialization_is_explicit_and_mutation_free() {
    let temp = tempfile::tempdir().unwrap();
    let mutation_store = store(&temp);
    mutation_store.initialize_empty().unwrap();
    assert!(temp.path().join("issue-mutations.json").is_file());
    assert!(mutation_store.initialize_empty().is_err());
}

#[cfg(unix)]
#[test]
fn repeated_cycle_start_and_queries_do_not_rewrite_the_journal() {
    use std::os::unix::fs::MetadataExt;

    let temp = tempfile::tempdir().unwrap();
    let mutation_store = store(&temp);
    let cycle = CycleId::new("cycle-read-only").unwrap();
    let limit = IssueMutationLimit::new(1).unwrap();
    mutation_store.begin_cycle(cycle.clone(), limit).unwrap();
    let path = temp.path().join("issue-mutations.json");
    let inode = std::fs::metadata(&path).unwrap().ino();

    mutation_store.begin_cycle(cycle.clone(), limit).unwrap();
    assert_eq!(mutation_store.cycle_failure(&cycle).unwrap(), None);
    assert!(
        mutation_store
            .stewardship_issue_numbers("rysweet/Simard")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        std::fs::metadata(path).unwrap().ino(),
        inode,
        "read-only journal access must not trigger atomic file replacement"
    );
}

#[test]
fn completed_mutation_is_idempotent_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let transport = FakeTransport::default();
    let cycle = CycleId::new("cycle-restart").unwrap();
    let mutation = request("condition:stable-across-restart");

    let mut first = MutationGuard::new(store(&temp));
    first
        .begin_cycle(cycle.clone(), IssueMutationLimit::new(1).unwrap())
        .unwrap();
    let outcome = first.execute(&cycle, &mutation, &transport).unwrap();
    assert!(matches!(outcome, IssueMutationOutcome::Completed { .. }));
    assert_eq!(transport.create_count(), 1);
    drop(first);

    let mut restarted = MutationGuard::new(store(&temp));
    let changed_observation = IssueMutationRequest::create(
        "rysweet/Simard",
        mutation.identity.clone(),
        operator_provenance("operator-request"),
        "Issue title changed after a generated slug changed",
        "the next observation mentioned a different GitHub issue number",
    )
    .unwrap();
    let replay = restarted
        .execute(&cycle, &changed_observation, &transport)
        .unwrap();
    assert!(matches!(
        replay,
        IssueMutationOutcome::AlreadyCompleted { .. }
    ));
    assert_eq!(
        transport.create_count(),
        1,
        "a persisted mutation identity is authoritative after restart"
    );
}

#[test]
fn unfinished_reservation_requires_operator_reconciliation() {
    let temp = tempfile::tempdir().unwrap();
    let cycle = CycleId::new("cycle-reconcile").unwrap();
    let mutation = request("condition:reserved-before-crash");
    let mutation_store = store(&temp);
    mutation_store
        .begin_cycle(cycle.clone(), IssueMutationLimit::new(1).unwrap())
        .unwrap();
    mutation_store.reserve(&cycle, &mutation).unwrap();
    drop(mutation_store);

    let transport = FakeTransport::default();

    let mut restarted = MutationGuard::new(store(&temp));
    let error = restarted
        .execute(&cycle, &mutation, &transport)
        .unwrap_err();
    assert!(error.to_string().contains("unfinished reservation"));
    assert_eq!(
        transport.create_count(),
        0,
        "an unfinished reservation is reconciled before any retry"
    );
}

#[test]
fn unresolved_reservation_fails_closed_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let cycle = CycleId::new("cycle-ambiguous").unwrap();
    let mutation = request("condition:ambiguous-after-crash");
    let mutation_store = store(&temp);
    mutation_store
        .begin_cycle(cycle.clone(), IssueMutationLimit::new(1).unwrap())
        .unwrap();
    mutation_store.reserve(&cycle, &mutation).unwrap();
    drop(mutation_store);

    let transport = FakeTransport::default();
    let mut restarted = MutationGuard::new(store(&temp));
    let error = restarted
        .execute(&cycle, &mutation, &transport)
        .unwrap_err();
    assert!(
        error.to_string().contains("unfinished reservation"),
        "{error}"
    );
    assert_eq!(transport.create_count(), 0);
}

#[test]
fn unfinished_close_requires_operator_reconciliation() {
    #[derive(Default)]
    struct CloseTransport {
        close_calls: Mutex<usize>,
    }

    impl IssueMutationTransport for CloseTransport {
        fn create_issue(
            &self,
            _repo: &str,
            _identity: &IssueMutationIdentity,
            _title: &str,
            _body: &str,
            _labels: &[String],
            _assignees: &[String],
        ) -> crate::error::SimardResult<GhIssue> {
            unreachable!()
        }

        fn close_issue(&self, _repo: &str, _number: u64) -> crate::error::SimardResult<GhIssue> {
            *self.close_calls.lock().unwrap() += 1;
            unreachable!("an unfinished close must reconcile before retry")
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let cycle = CycleId::new("cycle-close-reconcile").unwrap();
    let mutation = IssueMutationRequest {
        repo: "rysweet/Simard".to_string(),
        identity: IssueMutationIdentity::new("condition:close-55").unwrap(),
        provenance: operator_provenance("operator-close"),
        mutation: IssueMutation::Close { number: 55 },
    };
    let mutation_store = store(&temp);
    mutation_store
        .begin_cycle(cycle.clone(), IssueMutationLimit::new(1).unwrap())
        .unwrap();
    mutation_store.reserve(&cycle, &mutation).unwrap();
    drop(mutation_store);

    let transport = CloseTransport::default();
    let mut restarted = MutationGuard::new(store(&temp));
    let error = restarted
        .execute(&cycle, &mutation, &transport)
        .unwrap_err();
    assert!(error.to_string().contains("unfinished reservation"));
    assert_eq!(*transport.close_calls.lock().unwrap(), 0);
}

#[test]
fn mutation_limit_fails_cycle_before_limit_plus_one_external_call() {
    let temp = tempfile::tempdir().unwrap();
    let transport = FakeTransport::default();
    let cycle = CycleId::new("cycle-bounded").unwrap();
    let mut guard = MutationGuard::new(store(&temp));
    guard
        .begin_cycle(cycle.clone(), IssueMutationLimit::new(1).unwrap())
        .unwrap();

    guard
        .execute(&cycle, &request("condition:first"), &transport)
        .unwrap();
    let error = guard
        .execute(&cycle, &request("condition:second"), &transport)
        .unwrap_err();
    assert!(error.to_string().contains("mutation limit"), "{error}");
    assert_eq!(transport.create_count(), 1);

    drop(guard);
    let mut restarted = MutationGuard::new(store(&temp));
    let restart_error = restarted
        .execute(&cycle, &request("condition:third"), &transport)
        .unwrap_err();
    assert!(
        restart_error.to_string().contains("mutation limit"),
        "{restart_error}"
    );
    assert_eq!(
        transport.create_count(),
        1,
        "restart preserves the consumed cycle budget"
    );
}

#[test]
fn stewardship_provenance_cannot_authorize_an_issue_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let transport = FakeTransport::default();
    let cycle = CycleId::new("cycle-recursion").unwrap();
    let mutation = IssueMutationRequest::create(
        "rysweet/Simard",
        IssueMutationIdentity::new("condition:recursive").unwrap(),
        stewardship_provenance("stewardship-root"),
        "recursive issue",
        "must be rejected",
    )
    .unwrap();
    let mut guard = MutationGuard::new(store(&temp));
    guard
        .begin_cycle(cycle.clone(), IssueMutationLimit::new(1).unwrap())
        .unwrap();

    let error = guard.execute(&cycle, &mutation, &transport).unwrap_err();
    assert!(error.to_string().contains("provenance"), "{error}");
    assert_eq!(transport.create_count(), 0);
}

#[test]
fn stewardship_backlog_cannot_promote_or_reappear_as_a_gap_after_restart() {
    let mut board = GoalBoard::new();
    board.backlog.push(BacklogItem {
        id: "stewardship-noise".to_string(),
        description: "stewardship-created backlog item".to_string(),
        source: "typed-source-is-not-parsed".to_string(),
        score: 1.0,
    });
    board.set_provenance(
        ArtifactKind::BacklogItem,
        "stewardship-noise",
        stewardship_provenance("stewardship-root"),
    );

    let serialized = serde_json::to_string(&board).unwrap();
    let mut restarted: GoalBoard = serde_json::from_str(&serialized).unwrap();
    let error = promote_to_active(&mut restarted, "stewardship-noise", 1, None).unwrap_err();
    assert!(error.to_string().contains("provenance"), "{error}");

    restarted.active.push(ActiveGoal::new(
        "recursive-goal",
        "must not become a gap",
        1,
    ));
    restarted.set_provenance(
        ArtifactKind::Goal,
        "recursive-goal",
        stewardship_provenance("stewardship-root"),
    );
    let restarted_again: GoalBoard =
        serde_json::from_str(&serde_json::to_string(&restarted).unwrap()).unwrap();
    let gaps = detect_workstream_gaps(&restarted_again, &[], &[], &[]);
    assert!(
        gaps.iter().all(|gap| gap.ref_id != "recursive-goal"),
        "stewardship lineage must remain excluded after deserialization: {gaps:?}"
    );
}

#[test]
fn legacy_provenance_is_not_silently_treated_as_safe() {
    let legacy: ArtifactProvenance = serde_json::from_str("{}").unwrap();
    assert!(
        !legacy.is_recursive_input_eligible(),
        "missing provenance must fail closed as LegacyUnknown"
    );
}

#[test]
fn mutation_request_rejects_unbounded_transport_fields() {
    let error = IssueMutationRequest::create(
        "rysweet/Simard",
        IssueMutationIdentity::new("condition:oversized-body").unwrap(),
        operator_provenance("operator-request"),
        "bounded title",
        "x".repeat(64 * 1024 + 1),
    )
    .unwrap_err();
    assert!(error.to_string().contains("body"), "{error}");
}

#[test]
fn classifying_one_legacy_board_item_does_not_upgrade_unclassified_siblings() {
    let mut board: GoalBoard = serde_json::from_str(
        r#"{
            "active": [
                {
                    "id": "legacy-a",
                    "description": "legacy goal a",
                    "priority": 1,
                    "status": "NotStarted",
                    "assigned_to": null
                },
                {
                    "id": "legacy-b",
                    "description": "legacy goal b",
                    "priority": 1,
                    "status": "NotStarted",
                    "assigned_to": null
                }
            ],
            "backlog": []
        }"#,
    )
    .unwrap();

    board.set_provenance(
        ArtifactKind::Goal,
        "legacy-a",
        operator_provenance("operator-classified-a"),
    );

    assert!(
        board
            .provenance_for(ArtifactKind::Goal, "legacy-a")
            .is_recursive_input_eligible()
    );
    assert!(
        !board
            .provenance_for(ArtifactKind::Goal, "legacy-b")
            .is_recursive_input_eligible(),
        "classifying one legacy item must not silently trust every sibling"
    );
}

#[test]
fn ambiguous_mutation_fails_cycle_durably() {
    #[derive(Default)]
    struct FailingTransport;

    impl IssueMutationTransport for FailingTransport {
        fn create_issue(
            &self,
            _repo: &str,
            _identity: &IssueMutationIdentity,
            _title: &str,
            _body: &str,
            _labels: &[String],
            _assignees: &[String],
        ) -> crate::error::SimardResult<GhIssue> {
            Err(crate::error::SimardError::StewardshipGhCommandFailed {
                reason: "ambiguous transport failure".to_string(),
            })
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let cycle = CycleId::new("cycle-ambiguous-fatal").unwrap();
    let mut guard = MutationGuard::new(store(&temp));
    guard
        .begin_cycle(cycle.clone(), IssueMutationLimit::new(2).unwrap())
        .unwrap();
    guard
        .execute(
            &cycle,
            &request("condition:ambiguous-first"),
            &FailingTransport,
        )
        .unwrap_err();
    drop(guard);

    let transport = FakeTransport::default();
    let mut restarted = MutationGuard::new(store(&temp));
    let error = restarted
        .execute(
            &cycle,
            &request("condition:must-not-run-after-failure"),
            &transport,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cycle 'cycle-ambiguous-fatal' already failed"),
        "{error}"
    );
    assert_eq!(transport.create_count(), 0);
}

#[test]
fn recursive_input_view_excludes_stewardship_and_legacy_after_restart() {
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal::new(
        "operator-goal",
        "eligible operator goal",
        1,
    ));
    board.set_provenance(
        ArtifactKind::Goal,
        "operator-goal",
        operator_provenance("operator-goal"),
    );
    board.active.push(ActiveGoal::new(
        "stewardship-goal",
        "recursive stewardship goal",
        1,
    ));
    board.set_provenance(
        ArtifactKind::Goal,
        "stewardship-goal",
        stewardship_provenance("stewardship-goal"),
    );
    board.active.push(ActiveGoal::new(
        "legacy-goal",
        "unclassified legacy goal",
        1,
    ));
    board.provenance_version = 0;

    let restarted: GoalBoard =
        serde_json::from_str(&serde_json::to_string(&board).unwrap()).unwrap();
    let eligible = restarted.recursive_input_view();
    assert_eq!(
        eligible
            .active
            .iter()
            .map(|goal| goal.id.as_str())
            .collect::<Vec<_>>(),
        vec!["operator-goal"]
    );
}

#[test]
fn stewardship_gap_remains_ineligible_after_serialization() {
    let gap = crate::overseer::signal::GapItem {
        provenance: stewardship_provenance("stewardship-gap"),
        category: crate::overseer::signal::GapCategory::GoalUncovered,
        ref_id: "recursive".to_string(),
        title: "recursive".to_string(),
        why_it_matters: "must remain excluded".to_string(),
        signature: "goal:recursive".to_string(),
    };
    let restarted: crate::overseer::signal::GapItem =
        serde_json::from_str(&serde_json::to_string(&gap).unwrap()).unwrap();
    assert!(!restarted.provenance.is_recursive_input_eligible());
}
