//! TDD tests for the stewardship loop (issue #1167).
//!
//! These tests are written **before** the implementation. They define the
//! contract for `src/stewardship/` and the `enqueue_stewardship_issue`
//! helper in `src/goal_curation/operations.rs`.
//!
//! Test plan (mirrors design spec §7):
//! - Routing matrix: amplihack / simard / ambiguous / overlap-precedence
//! - Signature: stable across noise, differs on kind change
//! - find_existing: matches signature in body, ignores absent
//! - End-to-end: FiledNew, MatchedExisting, idempotent re-invocation
//! - Failure propagation: gh search, gh create
//! - Input validation: empty required fields → InvalidRunSummary
//! - No-fallback: ambiguous routing never calls gh

use std::cell::RefCell;
use std::sync::Mutex;

use crate::error::SimardError;
use crate::goal_curation::GoalBoard;
use crate::stewardship::gh_client::IssueMutationTransport;
use crate::stewardship::mutation_guard::MutationGuard;
use crate::stewardship::mutation_store::MutationStore;
use crate::stewardship::{
    ArtifactProvenance, CycleId, GhClient, GhIssue, IssueMutationIdentity, LineageId,
    OrchestratorRunSummary, StewardshipOutcome, process_orchestrator_run,
};

// ─────────────────────────── FakeGhClient ───────────────────────────

type SearchResponseMap =
    std::collections::HashMap<(String, String), Result<Vec<GhIssue>, SimardError>>;

#[derive(Default)]
struct FakeGhClient {
    /// Pre-seeded responses for `search_issues`. Key: (repo, signature).
    search_responses: Mutex<SearchResponseMap>,
    /// Pre-seeded responses for `create_issue`. Key: repo.
    create_responses: Mutex<std::collections::HashMap<String, Result<GhIssue, SimardError>>>,
    /// Recorded calls.
    search_calls: Mutex<Vec<(String, String)>>,
    create_calls: Mutex<Vec<(String, String, String)>>,
}

impl FakeGhClient {
    fn new() -> Self {
        Self::default()
    }
    fn seed_search(&self, repo: &str, sig: &str, result: Result<Vec<GhIssue>, SimardError>) {
        self.search_responses
            .lock()
            .unwrap()
            .insert((repo.to_string(), sig.to_string()), result);
    }
    fn seed_create(&self, repo: &str, result: Result<GhIssue, SimardError>) {
        self.create_responses
            .lock()
            .unwrap()
            .insert(repo.to_string(), result);
    }
    fn search_call_count(&self) -> usize {
        self.search_calls.lock().unwrap().len()
    }
    fn create_call_count(&self) -> usize {
        self.create_calls.lock().unwrap().len()
    }
}

impl GhClient for FakeGhClient {
    fn search_issues(&self, repo: &str, signature: &str) -> Result<Vec<GhIssue>, SimardError> {
        self.search_calls
            .lock()
            .unwrap()
            .push((repo.to_string(), signature.to_string()));
        match self
            .search_responses
            .lock()
            .unwrap()
            .get(&(repo.to_string(), signature.to_string()))
        {
            Some(Ok(v)) => Ok(v.clone()),
            Some(Err(e)) => Err(e.clone()),
            None => Ok(vec![]),
        }
    }
}

impl IssueMutationTransport for FakeGhClient {
    fn create_issue(
        &self,
        repo: &str,
        _identity: &IssueMutationIdentity,
        title: &str,
        body: &str,
        _labels: &[String],
        _assignees: &[String],
    ) -> Result<GhIssue, SimardError> {
        self.create_calls.lock().unwrap().push((
            repo.to_string(),
            title.to_string(),
            body.to_string(),
        ));
        match self.create_responses.lock().unwrap().get(repo) {
            Some(Ok(v)) => Ok(v.clone()),
            Some(Err(e)) => Err(e.clone()),
            None => Ok(GhIssue {
                number: 999,
                url: format!("https://github.com/{repo}/issues/999"),
                title: title.to_string(),
                body: body.to_string(),
            }),
        }
    }
}

// ─────────────────────────── Helpers ───────────────────────────

fn sample_run() -> OrchestratorRunSummary {
    OrchestratorRunSummary {
        run_id: "run-abc123".to_string(),
        recipe_name: "smart-orchestrator".to_string(),
        failed_step: "step-7-tdd".to_string(),
        source_module: "simard::engineer_loop".to_string(),
        failure_kind: "PanicInStep".to_string(),
        error_text: "panic at /home/user/src/foo.rs:42:7\nbacktrace deadbeef".to_string(),
        condition_id: IssueMutationIdentity::new("condition:panic-in-step").unwrap(),
        cycle_id: CycleId::new("run-abc123").unwrap(),
        provenance: ArtifactProvenance::system(LineageId::new("orchestrator-run").unwrap()),
        disposition: crate::stewardship::StewardshipDisposition::AuthorizedIssue,
    }
}

fn amplihack_run() -> OrchestratorRunSummary {
    OrchestratorRunSummary {
        run_id: "run-xyz789".to_string(),
        recipe_name: "smart-orchestrator".to_string(),
        failed_step: "decompose".to_string(),
        source_module: "amplihack::recipe-runner".to_string(),
        failure_kind: "NonZeroExit".to_string(),
        error_text: "exit 1: decomposition produced 0 workstreams".to_string(),
        condition_id: IssueMutationIdentity::new("condition:zero-workstreams").unwrap(),
        cycle_id: CycleId::new("run-xyz789").unwrap(),
        provenance: ArtifactProvenance::system(LineageId::new("orchestrator-run").unwrap()),
        disposition: crate::stewardship::StewardshipDisposition::AuthorizedIssue,
    }
}

fn process_run(
    run: &OrchestratorRunSummary,
    gh: &FakeGhClient,
    _board: &mut GoalBoard,
) -> crate::error::SimardResult<StewardshipOutcome> {
    let temp = tempfile::tempdir().unwrap();
    let mut guard = MutationGuard::new(MutationStore::new(temp.path().join("mutations.json")));
    process_orchestrator_run(run, gh, &mut guard)
}

// ─────────────────────────── Routing tests ───────────────────────────

#[test]
fn process_run_does_not_trust_unjournaled_remote_signature() {
    let gh = FakeGhClient::new();
    let mut board = GoalBoard::new();
    let run = sample_run();
    let sig = run.condition_id.as_str().to_string();

    gh.seed_search(
        "rysweet/Simard",
        &sig,
        Ok(vec![GhIssue {
            number: 11,
            url: "https://github.com/rysweet/Simard/issues/11".into(),
            title: "[stewardship] previously filed".into(),
            body: format!("stewardship-signature: {sig}\n## Error\nold"),
        }]),
    );

    let outcome = process_run(&run, &gh, &mut board).unwrap();
    assert!(matches!(
        outcome,
        StewardshipOutcome::FiledNew {
            issue_number: 999,
            ..
        }
    ));
    assert_eq!(gh.search_call_count(), 0);
    assert_eq!(gh.create_call_count(), 1);
    assert!(board.backlog.is_empty());
}

#[test]
fn process_run_idempotent_on_second_invocation() {
    let gh = FakeGhClient::new();
    let board = GoalBoard::new();
    let run = sample_run();
    let sig = run.condition_id.as_str().to_string();

    // First call: empty search → file new (#42).
    gh.seed_search("rysweet/Simard", &sig, Ok(vec![]));
    gh.seed_create(
        "rysweet/Simard",
        Ok(GhIssue {
            number: 42,
            url: "https://github.com/rysweet/Simard/issues/42".into(),
            title: "t".into(),
            body: format!("stewardship-signature: {sig}"),
        }),
    );
    let temp = tempfile::tempdir().unwrap();
    let mut guard = MutationGuard::new(MutationStore::new(temp.path().join("mutations.json")));
    let first = process_orchestrator_run(&run, &gh, &mut guard).unwrap();
    assert!(matches!(
        first,
        StewardshipOutcome::FiledNew {
            issue_number: 42,
            ..
        }
    ));

    // Second call: search now returns the issue → MatchedExisting; no new backlog row.
    gh.seed_search(
        "rysweet/Simard",
        &sig,
        Ok(vec![GhIssue {
            number: 42,
            url: "https://github.com/rysweet/Simard/issues/42".into(),
            title: "t".into(),
            body: format!("stewardship-signature: {sig}"),
        }]),
    );
    let second = process_orchestrator_run(&run, &gh, &mut guard).unwrap();
    assert!(matches!(
        second,
        StewardshipOutcome::MatchedExisting {
            issue_number: 42,
            ..
        }
    ));
    assert!(board.backlog.is_empty());
}

#[test]
fn process_run_routes_amplihack_failures_to_amplihack_repo() {
    let gh = FakeGhClient::new();
    let mut board = GoalBoard::new();
    let run = amplihack_run();
    let sig = run.condition_id.as_str().to_string();

    gh.seed_search("rysweet/amplihack", &sig, Ok(vec![]));
    gh.seed_create(
        "rysweet/amplihack",
        Ok(GhIssue {
            number: 7,
            url: "https://github.com/rysweet/amplihack/issues/7".into(),
            title: "t".into(),
            body: format!("stewardship-signature: {sig}"),
        }),
    );

    let outcome = process_run(&run, &gh, &mut board).unwrap();
    if let StewardshipOutcome::FiledNew { repo, .. } = outcome {
        assert_eq!(repo, "rysweet/amplihack");
    } else {
        panic!("expected FiledNew");
    }
}

// ─────────────────────────── Failure propagation ───────────────────────────

#[test]
fn process_run_does_not_consult_untrusted_remote_search() {
    let gh = FakeGhClient::new();
    let mut board = GoalBoard::new();
    let run = sample_run();
    let sig = run.condition_id.as_str().to_string();

    gh.seed_search(
        "rysweet/Simard",
        &sig,
        Err(SimardError::StewardshipGhCommandFailed {
            reason: "gh: rate limit exceeded".into(),
        }),
    );

    let outcome = process_run(&run, &gh, &mut board).unwrap();
    assert!(matches!(outcome, StewardshipOutcome::FiledNew { .. }));
    assert_eq!(gh.search_call_count(), 0);
    assert_eq!(gh.create_call_count(), 1);
}

#[test]
fn process_run_propagates_gh_create_failure() {
    let gh = FakeGhClient::new();
    let mut board = GoalBoard::new();
    let run = sample_run();
    let sig = run.condition_id.as_str().to_string();

    gh.seed_search("rysweet/Simard", &sig, Ok(vec![]));
    gh.seed_create(
        "rysweet/Simard",
        Err(SimardError::StewardshipGhCommandFailed {
            reason: "gh: 422 validation failed".into(),
        }),
    );

    let err = process_run(&run, &gh, &mut board).unwrap_err();
    assert!(matches!(
        err,
        SimardError::StewardshipGhCommandFailed { .. }
    ));
    assert!(board.backlog.is_empty(), "no backlog row when create fails");
}

#[test]
fn process_run_rejects_routine_workstream_gap_before_github_or_backlog() {
    let gh = FakeGhClient::new();
    let mut board = GoalBoard::new();
    let mut run = sample_run();
    run.source_module = "overseer".to_string();
    run.failure_kind = "workstream_gap:goal".to_string();
    run.disposition = crate::stewardship::StewardshipDisposition::ObservationOnly;

    let error = process_run(&run, &gh, &mut board).unwrap_err();
    assert!(error.to_string().contains("observation-only"), "{error}");
    assert_eq!(gh.search_call_count(), 0);
    assert_eq!(gh.create_call_count(), 0);
    assert!(board.backlog.is_empty());
}

// ─────────────────────────── Input validation ───────────────────────────

#[test]
fn process_run_rejects_empty_run_id() {
    let gh = FakeGhClient::new();
    let mut board = GoalBoard::new();
    let mut run = sample_run();
    run.run_id = String::new();
    let err = process_run(&run, &gh, &mut board).unwrap_err();
    assert!(matches!(
        err,
        SimardError::StewardshipInvalidRunSummary { field } if field == "run_id"
    ));
    assert_eq!(gh.search_call_count(), 0);
}

#[test]
fn process_run_rejects_empty_source_module() {
    let gh = FakeGhClient::new();
    let mut board = GoalBoard::new();
    let mut run = sample_run();
    run.source_module = String::new();
    let err = process_run(&run, &gh, &mut board).unwrap_err();
    assert!(matches!(
        err,
        SimardError::StewardshipInvalidRunSummary { field } if field == "source_module"
    ));
}

#[test]
fn process_run_rejects_empty_failure_kind() {
    let gh = FakeGhClient::new();
    let mut board = GoalBoard::new();
    let mut run = sample_run();
    run.failure_kind = String::new();
    let err = process_run(&run, &gh, &mut board).unwrap_err();
    assert!(matches!(
        err,
        SimardError::StewardshipInvalidRunSummary { field } if field == "failure_kind"
    ));
}

#[test]
fn process_run_rejects_empty_error_text() {
    let gh = FakeGhClient::new();
    let mut board = GoalBoard::new();
    let mut run = sample_run();
    run.error_text = String::new();
    let err = process_run(&run, &gh, &mut board).unwrap_err();
    assert!(matches!(
        err,
        SimardError::StewardshipInvalidRunSummary { field } if field == "error_text"
    ));
}

// Suppress unused warnings on the RefCell import (kept for future extensions).
#[allow(dead_code)]
fn _unused_refcell_anchor() -> RefCell<()> {
    RefCell::new(())
}
