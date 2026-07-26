//! TDD tests for the stewardship loop (issue #1167).
//!
//! These tests define the contract for `src/stewardship/`.
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
use crate::stewardship::dedup::failure_signature;
use crate::stewardship::{
    GhClient, GhIssue, OrchestratorRunSummary, StewardshipOutcome, process_orchestrator_run,
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
fn process_run_matches_existing_when_signature_present() {
    let gh = FakeGhClient::new();
    let run = sample_run();
    let sig = failure_signature(&run.failure_kind, &run.error_text);

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

    let outcome = process_orchestrator_run(&run, &gh).unwrap();
    match outcome {
        StewardshipOutcome::MatchedExisting {
            issue_number, repo, ..
        } => {
            assert_eq!(issue_number, 11);
            assert_eq!(repo, "rysweet/Simard");
        }
        other => panic!("expected MatchedExisting, got {other:?}"),
    }

    assert_eq!(gh.search_call_count(), 1);
    assert_eq!(
        gh.create_call_count(),
        0,
        "must NOT create when match exists"
    );
}

#[test]
fn process_run_idempotent_on_second_invocation() {
    let gh = FakeGhClient::new();
    let run = sample_run();
    let sig = failure_signature(&run.failure_kind, &run.error_text);

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
    let first = process_orchestrator_run(&run, &gh).unwrap();
    assert!(matches!(
        first,
        StewardshipOutcome::FiledNew {
            issue_number: 42,
            ..
        }
    ));

    // Second call: search now returns the issue → MatchedExisting.
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
    let second = process_orchestrator_run(&run, &gh).unwrap();
    assert!(matches!(
        second,
        StewardshipOutcome::MatchedExisting {
            issue_number: 42,
            ..
        }
    ));
}

#[test]
fn process_run_routes_amplihack_failures_to_amplihack_repo() {
    let gh = FakeGhClient::new();
    let run = amplihack_run();
    let sig = failure_signature(&run.failure_kind, &run.error_text);

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

    let outcome = process_orchestrator_run(&run, &gh).unwrap();
    if let StewardshipOutcome::FiledNew { repo, .. } = outcome {
        assert_eq!(repo, "rysweet/amplihack");
    } else {
        panic!("expected FiledNew");
    }
}

// ─────────────────────────── Failure propagation ───────────────────────────

#[test]
fn process_run_propagates_gh_search_failure() {
    let gh = FakeGhClient::new();
    let run = sample_run();
    let sig = failure_signature(&run.failure_kind, &run.error_text);

    gh.seed_search(
        "rysweet/Simard",
        &sig,
        Err(SimardError::StewardshipGhCommandFailed {
            reason: "gh: rate limit exceeded".into(),
        }),
    );

    let err = process_orchestrator_run(&run, &gh).unwrap_err();
    assert!(matches!(
        err,
        SimardError::StewardshipGhCommandFailed { .. }
    ));
    assert_eq!(
        gh.create_call_count(),
        0,
        "must not create when search fails"
    );
}

#[test]
fn process_run_propagates_gh_create_failure() {
    let gh = FakeGhClient::new();
    let run = sample_run();
    let sig = failure_signature(&run.failure_kind, &run.error_text);

    gh.seed_search("rysweet/Simard", &sig, Ok(vec![]));
    gh.seed_create(
        "rysweet/Simard",
        Err(SimardError::StewardshipGhCommandFailed {
            reason: "gh: 422 validation failed".into(),
        }),
    );

    let err = process_orchestrator_run(&run, &gh).unwrap_err();
    assert!(matches!(
        err,
        SimardError::StewardshipGhCommandFailed { .. }
    ));
}

#[test]
fn process_run_routes_overseer_to_default_and_dedups() {
    // Regression for the `flag_workstream_gaps` failure: the Overseer files gap
    // issues with the bare source_module "overseer" (no keyword match). Before
    // the routing fallback this returned `StewardshipRoutingAmbiguous` and the
    // whole intervention failed every tick ("intervention failed ...
    // flag_workstream_gaps"). Now the router must route the unmatched source to
    // the DEFAULT repo (rysweet/Simard), file exactly ONE issue, and dedup on a
    // rerun with the same gap signature (no duplicate issues).
    let gh = FakeGhClient::new();
    let mut run = sample_run();
    run.source_module = "overseer".to_string();
    let sig = failure_signature(&run.failure_kind, &run.error_text);

    // First tick: no existing issue in the default repo → file exactly one.
    gh.seed_search("rysweet/Simard", &sig, Ok(vec![]));
    gh.seed_create(
        "rysweet/Simard",
        Ok(GhIssue {
            number: 314,
            url: "https://github.com/rysweet/Simard/issues/314".into(),
            title: "[stewardship] workstream gap".into(),
            body: format!("stewardship-signature: {sig}"),
        }),
    );

    let first = process_orchestrator_run(&run, &gh).unwrap();
    match first {
        StewardshipOutcome::FiledNew {
            repo, issue_number, ..
        } => {
            assert_eq!(
                repo, "rysweet/Simard",
                "an unmatched source_module routes to the default repo"
            );
            assert_eq!(issue_number, 314);
        }
        other => panic!("expected FiledNew in rysweet/Simard, got {other:?}"),
    }
    // The router must have searched the DEFAULT repo (not amplihack) and created
    // exactly one tracking issue.
    assert_eq!(
        gh.search_call_count(),
        1,
        "must search the default repo before filing"
    );
    assert_eq!(
        gh.create_call_count(),
        1,
        "exactly one gap issue filed on the first tick"
    );

    // Second tick, SAME gap signature: search now returns the existing issue →
    // MatchedExisting and NO second create (idempotent per signature — one
    // rolling tracking issue per distinct gap, never a duplicate each tick).
    gh.seed_search(
        "rysweet/Simard",
        &sig,
        Ok(vec![GhIssue {
            number: 314,
            url: "https://github.com/rysweet/Simard/issues/314".into(),
            title: "[stewardship] workstream gap".into(),
            body: format!("stewardship-signature: {sig}"),
        }]),
    );
    let second = process_orchestrator_run(&run, &gh).unwrap();
    assert!(
        matches!(
            second,
            StewardshipOutcome::MatchedExisting {
                issue_number: 314,
                ..
            }
        ),
        "rerun with the same gap signature must dedup to the existing issue: {second:?}"
    );
    assert_eq!(
        gh.create_call_count(),
        1,
        "idempotent per gap signature — no duplicate issue on rerun"
    );
}

// ─────────────────────────── Storm regression (multi-sweep dedup) ───────────

/// A stateful `GhClient` that models the daemon's real dedup surface once the
/// search-index-lag fallback is in play: a filed tracking issue is visible to a
/// subsequent `search_issues` call (as the strongly-consistent recent-open scan
/// makes it), so re-observing the same failure matches instead of re-filing.
/// Unlike the seeded [`FakeGhClient`], this one accumulates created issues and
/// answers searches from that live store — the shape a full observation window
/// actually sees.
#[derive(Default)]
struct StatefulGhClient {
    issues: Mutex<Vec<GhIssue>>,
    create_calls: Mutex<usize>,
    next_number: Mutex<u64>,
}

impl StatefulGhClient {
    fn new() -> Self {
        Self {
            issues: Mutex::new(Vec::new()),
            create_calls: Mutex::new(0),
            next_number: Mutex::new(4671),
        }
    }
    fn create_call_count(&self) -> usize {
        *self.create_calls.lock().unwrap()
    }
    fn open_issue_count(&self) -> usize {
        self.issues.lock().unwrap().len()
    }
}

impl GhClient for StatefulGhClient {
    fn search_issues(&self, _repo: &str, signature: &str) -> Result<Vec<GhIssue>, SimardError> {
        let needle = format!("stewardship-signature: {signature}");
        Ok(self
            .issues
            .lock()
            .unwrap()
            .iter()
            .filter(|i| i.body.contains(&needle))
            .cloned()
            .collect())
    }
    fn create_issue(&self, repo: &str, title: &str, body: &str) -> Result<GhIssue, SimardError> {
        *self.create_calls.lock().unwrap() += 1;
        let mut num = self.next_number.lock().unwrap();
        let issue = GhIssue {
            number: *num,
            url: format!("https://github.com/{repo}/issues/{num}"),
            title: title.to_string(),
            body: body.to_string(),
        };
        *num += 1;
        self.issues.lock().unwrap().push(issue.clone());
        Ok(issue)
    }
}

/// End-to-end regression for the stewardship issue *storm* (rysweet/Simard
/// #4671-#4686): a whole observation window of identical detections routed
/// through [`process_orchestrator_run`] must produce exactly ONE open issue —
/// the first sweep files, every later sweep dedups to `MatchedExisting`. This is
/// the public-API expression of the "duplicate detections within a window
/// produce at most one open issue" contract; it would fail the moment the
/// file-if-no-match dedup guard regressed and the loop filed per-tick.
#[test]
fn process_run_files_one_issue_across_a_full_window_of_identical_detections() {
    let gh = StatefulGhClient::new();
    let run = sample_run();

    let mut filed_new = 0usize;
    let mut matched_existing = 0usize;
    for _ in 0..15 {
        match process_orchestrator_run(&run, &gh).unwrap() {
            StewardshipOutcome::FiledNew { .. } => filed_new += 1,
            StewardshipOutcome::MatchedExisting { .. } => matched_existing += 1,
        }
    }

    assert_eq!(
        filed_new, 1,
        "only the first sweep files a new tracking issue"
    );
    assert_eq!(
        matched_existing, 14,
        "every subsequent sweep dedups to the existing issue"
    );
    assert_eq!(
        gh.create_call_count(),
        1,
        "exactly one `create_issue` across the whole window — no duplicate flood"
    );
    assert_eq!(
        gh.open_issue_count(),
        1,
        "exactly one open tracking issue remains after the window"
    );
}

// ─────────────────────────── Input validation ───────────────────────────

#[test]
fn process_run_rejects_empty_run_id() {
    let gh = FakeGhClient::new();
    let mut run = sample_run();
    run.run_id = String::new();
    let err = process_orchestrator_run(&run, &gh).unwrap_err();
    assert!(matches!(
        err,
        SimardError::StewardshipInvalidRunSummary { field } if field == "run_id"
    ));
    assert_eq!(gh.search_call_count(), 0);
}

#[test]
fn process_run_rejects_empty_source_module() {
    let gh = FakeGhClient::new();
    let mut run = sample_run();
    run.source_module = String::new();
    let err = process_orchestrator_run(&run, &gh).unwrap_err();
    assert!(matches!(
        err,
        SimardError::StewardshipInvalidRunSummary { field } if field == "source_module"
    ));
}

#[test]
fn process_run_rejects_empty_failure_kind() {
    let gh = FakeGhClient::new();
    let mut run = sample_run();
    run.failure_kind = String::new();
    let err = process_orchestrator_run(&run, &gh).unwrap_err();
    assert!(matches!(
        err,
        SimardError::StewardshipInvalidRunSummary { field } if field == "failure_kind"
    ));
}

#[test]
fn process_run_rejects_empty_error_text() {
    let gh = FakeGhClient::new();
    let mut run = sample_run();
    run.error_text = String::new();
    let err = process_orchestrator_run(&run, &gh).unwrap_err();
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

// ─────────────────────────────────────────────────────────────────────────────
// Issue #4721 (WS-2): thin deterministic merge-judge RAIL + draft gate.
//
// TDD tests for the reworked `recipe_merge_judge.rs`. They pin the contract for
// the NOT-YET-IMPLEMENTED deterministic decision seam that replaces the deleted
// `parse_merge_verdict_from_text` / JSON-envelope stdout scrape:
//
//   crate::stewardship::recipe_merge_judge::resolve_final_verdict(
//       read: &ReadOutcome, snapshot: &PrSnapshot, base_allowlist: &[String],
//   ) -> JudgeOutcome
//
// Rules (design R3/R4):
//   * Ready  iff the freshness-checked record says `merge` AND every hard gate
//     passes (base allow-list, MERGEABLE, CI green, NOT draft).
//   * NotReady if the record says `hold`, OR says `merge` but any gate fails
//     (a LOUD refusal — the agent verdict is advisory; the rail is authority).
//   * Unclear (fail-closed) if there is no valid record for this run
//     (Missing / Mismatch).
//
// Plus the fail-closed draft gate added to `evaluate_objective_gates`
// (design R7): `Some(true)` and `None` both refuse; only `Some(false)` passes.
//
// These are expected to FAIL TO COMPILE until the rework lands (TDD red).
#[cfg(test)]
mod issue_4721_rail_tests {
    use crate::stewardship::merge_authority::{
        CheckRollupEntry, PrSnapshot, evaluate_objective_gates,
    };
    use crate::stewardship::merge_judge::Verdict;
    use crate::stewardship::merge_verdict_store::{MergeVerdictRecord, ReadOutcome, VerdictKind};
    use crate::stewardship::recipe_merge_judge::resolve_final_verdict;

    fn allowlist() -> Vec<String> {
        vec!["main".to_string()]
    }

    /// A snapshot that PASSES every hard gate: on `main`, MERGEABLE, one green
    /// check, explicitly not a draft.
    fn green_snapshot() -> PrSnapshot {
        PrSnapshot {
            body: "body".into(),
            mergeable: "MERGEABLE".into(),
            review_decision: "APPROVED".into(),
            checks: vec![CheckRollupEntry {
                name: "ci".into(),
                state: "SUCCESS".into(),
            }],
            base_ref_name: "main".into(),
            labels: vec![],
            is_draft: Some(false),
        }
    }

    fn merge_record() -> ReadOutcome {
        ReadOutcome::Found(MergeVerdictRecord::new(
            1,
            "o/r",
            VerdictKind::Merge,
            "crusty passed",
            "tok",
        ))
    }

    fn hold_record() -> ReadOutcome {
        ReadOutcome::Found(MergeVerdictRecord::new(
            1,
            "o/r",
            VerdictKind::Hold,
            "crusty flagged",
            "tok",
        ))
    }

    // ── the one "yes" path ──────────────────────────────────────────────────

    #[test]
    fn merge_verdict_with_all_gates_green_is_ready() {
        let out = resolve_final_verdict(&merge_record(), &green_snapshot(), &allowlist());
        assert_eq!(
            out.verdict,
            Verdict::Ready,
            "merge record + all gates green must authorize the merge"
        );
    }

    // ── the "loud refusal" paths (acceptance R4/R9) ─────────────────────────

    #[test]
    fn merge_verdict_with_red_ci_is_refused() {
        let mut snap = green_snapshot();
        snap.checks = vec![CheckRollupEntry {
            name: "ci".into(),
            state: "FAILURE".into(),
        }];
        let out = resolve_final_verdict(&merge_record(), &snap, &allowlist());
        assert_eq!(
            out.verdict,
            Verdict::NotReady,
            "a `merge` verdict against RED CI MUST be refused by the rail"
        );
        assert!(
            !out.rationale.trim().is_empty(),
            "the refusal must be loud (non-empty rationale naming the failed gate)"
        );
    }

    #[test]
    fn merge_verdict_with_draft_pr_is_refused() {
        let mut snap = green_snapshot();
        snap.is_draft = Some(true);
        let out = resolve_final_verdict(&merge_record(), &snap, &allowlist());
        assert_eq!(out.verdict, Verdict::NotReady, "draft PR must be refused");
        assert!(
            out.rationale.to_lowercase().contains("draft"),
            "refusal rationale should name the draft gate, got: {}",
            out.rationale
        );
    }

    #[test]
    fn merge_verdict_with_unknown_draft_state_fails_closed() {
        let mut snap = green_snapshot();
        snap.is_draft = None; // gh output missing isDraft ⇒ treat as draft
        let out = resolve_final_verdict(&merge_record(), &snap, &allowlist());
        assert_eq!(
            out.verdict,
            Verdict::NotReady,
            "unknown draft state must fail closed (refuse), never merge"
        );
    }

    #[test]
    fn merge_verdict_with_non_mergeable_pr_is_refused() {
        let mut snap = green_snapshot();
        snap.mergeable = "CONFLICTING".into();
        let out = resolve_final_verdict(&merge_record(), &snap, &allowlist());
        assert_eq!(
            out.verdict,
            Verdict::NotReady,
            "a non-mergeable PR must be refused regardless of the recorded verdict"
        );
    }

    #[test]
    fn merge_verdict_with_wrong_base_branch_is_refused() {
        let mut snap = green_snapshot();
        snap.base_ref_name = "stale-parent".into();
        let out = resolve_final_verdict(&merge_record(), &snap, &allowlist());
        assert_eq!(out.verdict, Verdict::NotReady);
    }

    // ── the agent said "hold" ───────────────────────────────────────────────

    #[test]
    fn hold_verdict_is_not_ready_even_when_gates_green() {
        let out = resolve_final_verdict(&hold_record(), &green_snapshot(), &allowlist());
        assert_eq!(
            out.verdict,
            Verdict::NotReady,
            "a `hold` verdict must never merge, even with all gates green"
        );
    }

    // ── no valid record for THIS run ────────────────────────────────────────

    #[test]
    fn missing_record_is_unclear() {
        let out = resolve_final_verdict(&ReadOutcome::Missing, &green_snapshot(), &allowlist());
        assert_eq!(
            out.verdict,
            Verdict::Unclear,
            "no record for this run ⇒ fail-closed Unclear (merge authority refuses)"
        );
    }

    #[test]
    fn stale_or_foreign_record_mismatch_is_unclear() {
        let out = resolve_final_verdict(
            &ReadOutcome::Mismatch("run_token mismatch".into()),
            &green_snapshot(),
            &allowlist(),
        );
        assert_eq!(
            out.verdict,
            Verdict::Unclear,
            "a stale/foreign record (Mismatch) must fail closed, never merge"
        );
    }

    // ── draft gate wired into evaluate_objective_gates (design R7) ───────────

    #[test]
    fn objective_gates_reject_draft_pr() {
        let mut snap = green_snapshot();
        snap.is_draft = Some(true);
        let err = evaluate_objective_gates(&snap, &allowlist())
            .expect_err("draft PR must fail the objective gates");
        assert!(
            err.to_lowercase().contains("draft"),
            "gate error should name the draft state, got: {err}"
        );
    }

    #[test]
    fn objective_gates_reject_unknown_draft_state_fail_closed() {
        let mut snap = green_snapshot();
        snap.is_draft = None;
        assert!(
            evaluate_objective_gates(&snap, &allowlist()).is_err(),
            "unknown draft state (None) must fail closed in the objective gates"
        );
    }

    #[test]
    fn objective_gates_pass_for_non_draft_green_pr() {
        assert!(
            evaluate_objective_gates(&green_snapshot(), &allowlist()).is_ok(),
            "an explicitly non-draft, green, mergeable PR on main must pass"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #4721 (WS-2): source-contract acceptance — the forbidden JSON-scrape
// pattern must be GONE from recipe_merge_judge.rs, and the safety rail's
// hard-coded invariants must remain. These are deterministic file-content
// assertions (no LLM, no subprocess).
#[cfg(test)]
mod issue_4721_source_contract_tests {
    use std::path::PathBuf;

    fn read_source(rel: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"))
    }

    #[test]
    fn recipe_merge_judge_has_no_json_scrape_or_verdict_text_parser() {
        let src = read_source("src/stewardship/recipe_merge_judge.rs");
        for forbidden in [
            "parse_merge_verdict_from_text",
            "step_results",
            "extract_recipe_decision_output",
            "parse_merge_outcome",
            "run_brain_ladder",
        ] {
            assert!(
                !src.contains(forbidden),
                "recipe_merge_judge.rs must no longer reference `{forbidden}` \
                 (the forbidden JSON-emit→scrape→act pattern was removed)"
            );
        }
        assert!(
            !src.contains("--output-format"),
            "the rail must not run the recipe with `--output-format json` anymore; \
             the verdict now arrives via the typed record, not stdout"
        );
    }

    #[test]
    fn recipe_merge_judge_reads_typed_record_and_reverifies_gates() {
        let src = read_source("src/stewardship/recipe_merge_judge.rs");
        assert!(
            src.contains("merge_verdict_store"),
            "the rail must READ the typed verdict via merge_verdict_store, not scrape stdout"
        );
        assert!(
            src.contains("evaluate_objective_gates"),
            "the rail must INDEPENDENTLY re-verify the objective gates before authorizing a merge"
        );
        assert!(
            src.contains("resolve_final_verdict"),
            "the rail's deterministic decision seam must exist"
        );
    }

    #[test]
    fn recipe_merge_judge_never_uses_admin_or_no_verify() {
        // Safety invariant (R8): the rail must never weaken the gate.
        let src = read_source("src/stewardship/recipe_merge_judge.rs");
        assert!(!src.contains("--admin"), "NEVER pass --admin");
        assert!(!src.contains("--no-verify"), "NEVER pass --no-verify");
    }

    #[test]
    fn merge_readiness_recipe_records_verdict_via_tool_and_prints_no_json() {
        let src = read_source("prompt_assets/simard/recipes/merge-readiness-judge.yaml");
        assert!(
            src.contains("merge record-verdict"),
            "the recipe must record its verdict via `simard merge record-verdict` (act-via-tool)"
        );
        assert!(
            src.contains("run_token"),
            "the recipe must thread the rail-supplied run_token to the record-verdict tool"
        );
        assert!(
            !src.contains(r#"{"verdict""#),
            "the recipe must NOT print a JSON verdict envelope for the daemon to scrape"
        );
    }
}
