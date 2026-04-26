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

use std::sync::Mutex;

use crate::error::SimardError;
use crate::goal_curation::GoalBoard;
use crate::stewardship::dedup::{failure_signature, find_existing, normalize};
use crate::stewardship::routing::route_failure;
use crate::stewardship::{
    GhClient, GhIssue, OrchestratorRunSummary, StewardshipOutcome, TargetRepo,
    process_orchestrator_run,
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
    fn create_issue(&self, repo: &str, title: &str, body: &str) -> Result<GhIssue, SimardError> {
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
    }
}

// ─────────────────────────── Routing tests ───────────────────────────

#[test]
fn route_amplihack_keywords() {
    for src in &[
        "amplihack",
        "amplihack::recipe-runner",
        "recipe-runner",
        "orchestrator",
        "recipe::default-workflow",
    ] {
        let r = route_failure(src).unwrap_or_else(|e| panic!("{src}: {e:?}"));
        assert!(
            matches!(r, TargetRepo::Amplihack),
            "{src} should be Amplihack"
        );
    }
}

#[test]
fn route_simard_keywords() {
    for src in &[
        "engineer_loop",
        "base_type_copilot",
        "self_improve::prioritization",
        "goal_curation::operations",
        "agent_loop",
        "session_builder",
        "simard::stewardship",
    ] {
        let r = route_failure(src).unwrap_or_else(|e| panic!("{src}: {e:?}"));
        assert!(matches!(r, TargetRepo::Simard), "{src} should be Simard");
    }
}

#[test]
fn route_ambiguous_errors() {
    let err = route_failure("unknown_subsystem").unwrap_err();
    assert!(matches!(
        err,
        SimardError::StewardshipRoutingAmbiguous { .. }
    ));
}

#[test]
fn route_amplihack_wins_on_overlap() {
    // Amplihack precedes Simard checks; an overlap pins to Amplihack.
    let r = route_failure("amplihack::engineer_loop").unwrap();
    assert!(matches!(r, TargetRepo::Amplihack));
}

#[test]
fn target_repo_slug() {
    assert_eq!(TargetRepo::Amplihack.slug(), "rysweet/amplihack");
    assert_eq!(TargetRepo::Simard.slug(), "rysweet/Simard");
}

// ─────────────────────────── Dedup / signature tests ───────────────────────────

#[test]
fn signature_stable_across_timestamps_paths_hashes_runids_linecols() {
    let kind = "PanicInStep";
    let a = "panic at /home/alice/src/foo.rs:42:7 at 2026-04-22T10:00:00Z run-abc123 hash deadbeefcafebabe";
    let b = "panic at /tmp/build/xyz/foo.rs:99:1 at 2027-01-01T23:59:59Z run-zzz000 hash 1234567890abcdef";
    let sa = failure_signature(kind, a);
    let sb = failure_signature(kind, b);
    assert_eq!(
        sa, sb,
        "noise (timestamps/paths/hashes/run-ids/line:col) must be normalized away"
    );
    assert_eq!(sa.len(), 16, "signature must be 16 hex chars");
}

#[test]
fn signature_differs_on_kind_change() {
    let msg = "stable error text";
    assert_ne!(
        failure_signature("PanicInStep", msg),
        failure_signature("AssertionFailure", msg)
    );
}

#[test]
fn signature_differs_on_message_change() {
    assert_ne!(
        failure_signature("X", "hello"),
        failure_signature("X", "goodbye")
    );
}

#[test]
fn normalize_strips_ansi_and_collapses_whitespace() {
    let n = normalize("\x1b[31mERR\x1b[0m   \n\n  trailing  ");
    assert_eq!(n, "ERR trailing");
}

#[test]
fn find_existing_matches_signature_in_body() {
    let sig = "abcdef0123456789";
    let issues = vec![
        GhIssue {
            number: 7,
            url: "https://github.com/rysweet/Simard/issues/7".into(),
            title: "[stewardship] PanicInStep in simard::engineer_loop".into(),
            body: format!(
                "filed-by: simard-stewardship\nstewardship-signature: {sig}\n## Error\n..."
            ),
        },
        GhIssue {
            number: 8,
            url: "u".into(),
            title: "other".into(),
            body: "no signature here".into(),
        },
    ];
    let hit = find_existing(&issues, sig).expect("should match issue #7");
    assert_eq!(hit.number, 7);
}

#[test]
fn find_existing_ignores_when_signature_absent() {
    let issues = vec![GhIssue {
        number: 1,
        url: "u".into(),
        title: "t".into(),
        body: "nothing relevant".into(),
    }];
    assert!(find_existing(&issues, "deadbeefdeadbeef").is_none());
}

// ─────────────────────────── End-to-end tests ───────────────────────────

#[test]
fn process_run_files_new_when_no_match() {
    let gh = FakeGhClient::new();
    let mut board = GoalBoard::new();
    let run = sample_run();
    let sig = failure_signature(&run.failure_kind, &run.error_text);

    gh.seed_search("rysweet/Simard", &sig, Ok(vec![]));
    gh.seed_create(
        "rysweet/Simard",
        Ok(GhIssue {
            number: 42,
            url: "https://github.com/rysweet/Simard/issues/42".into(),
            title: "[stewardship] PanicInStep in simard::engineer_loop".into(),
            body: format!("stewardship-signature: {sig}"),
        }),
    );

    let outcome = process_orchestrator_run(&run, &gh, &mut board).unwrap();
    match outcome {
        StewardshipOutcome::FiledNew {
            repo,
            issue_number,
            url,
            signature,
        } => {
            assert_eq!(repo, "rysweet/Simard");
            assert_eq!(issue_number, 42);
            assert_eq!(url, "https://github.com/rysweet/Simard/issues/42");
            assert_eq!(signature, sig);
        }
        other => panic!("expected FiledNew, got {other:?}"),
    }

    assert_eq!(gh.search_call_count(), 1);
    assert_eq!(gh.create_call_count(), 1);
    assert_eq!(
        board.backlog.len(),
        1,
        "backlog should hold 1 stewardship item"
    );
    let item = &board.backlog[0];
    assert_eq!(item.id, "stewardship-rysweet_Simard-42");
    assert_eq!(item.source, "stewardship:rysweet/Simard#42");
    assert!(item.description.contains("42") || item.description.contains(&sig));
}
