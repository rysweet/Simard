//! Tests for the deploy-aware done-gate ([`super`]).
//!
//! The gate logic is pure (evidence is injected), so these are real passing
//! tests — including the headline reproduction of the cognitive-memory backup
//! false-completion. The prompt content-pin tests are active: they assert the
//! shipped `prompt_assets/` copy carries the three-part done-gate wording.

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::archive_completed;
use crate::goal_curation::types::{ActiveGoal, GoalBoard, GoalProgress, WipRef};

use super::{
    COMPLETION_VERIFICATION_METRIC, CompletionEvidenceGate, CompletionVerdict, EvidenceSource,
    FALSE_COMPLETION_RATE_METRIC, GhCliEvidenceSource, MissingEvidence, VerificationOutcome,
    archive_completed_evidence_aware, archive_completed_with_evidence, classify_from_missing,
    classify_outcome, completion_evidence_enabled, error_class_from_missing, false_completion_rate,
    has_derivable_signal, is_self_affecting, parse_pr_url, record_completion_verification,
    record_false_completion_rate,
};

/// Canned, hermetic [`EvidenceSource`]. Each field is `Result<bool, String>` so
/// a test can model a clean answer or a transient query failure.
struct FakeEvidence {
    pr_merged: Result<bool, String>,
    issue_closed: Result<bool, String>,
    deployed: Result<bool, String>,
}

impl FakeEvidence {
    /// All three queries succeed with the given booleans.
    fn ok(pr_merged: bool, issue_closed: bool, deployed: bool) -> Self {
        Self {
            pr_merged: Ok(pr_merged),
            issue_closed: Ok(issue_closed),
            deployed: Ok(deployed),
        }
    }
}

fn to_result(r: &Result<bool, String>) -> SimardResult<bool> {
    r.clone()
        .map_err(|reason| SimardError::VerificationFailed { reason })
}

impl EvidenceSource for FakeEvidence {
    fn any_pr_merged(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        to_result(&self.pr_merged)
    }
    fn issue_closed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        to_result(&self.issue_closed)
    }
    fn is_deployed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        to_result(&self.deployed)
    }
}

fn simard_goal(id: &str, status: GoalProgress) -> ActiveGoal {
    ActiveGoal {
        labels: Vec::new(),
        id: id.to_string(),
        description: "improve the daemon".to_string(),
        priority: 1,
        status,
        assigned_to: None,
        repo: None, // routes to Simard => self-affecting
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
        parent_goal_id: None,
        priority_explicit: false,
    }
}

// --- evaluate: the three-part rule ------------------------------------------

#[test]
fn complete_when_merged_closed_and_deployed() {
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(true, true, true));
    let goal = simard_goal("g1", GoalProgress::Completed);
    let verdict = gate.evaluate(&goal);
    match verdict {
        CompletionVerdict::Complete(ev) => {
            assert!(ev.pr_merged && ev.issue_closed && ev.deployed);
            assert!(ev.self_affecting);
        }
        other => panic!("expected Complete, got {other:?}"),
    }
}

#[test]
fn blocked_when_pr_not_merged() {
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(false, true, true));
    let v = gate.evaluate(&simard_goal("g", GoalProgress::Completed));
    assert!(!v.is_complete());
    let missing = blocked_missing(&v);
    assert!(missing.contains(&MissingEvidence::PrNotMerged));
}

#[test]
fn blocked_when_issue_open() {
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(true, false, true));
    let v = gate.evaluate(&simard_goal("g", GoalProgress::Completed));
    assert!(blocked_missing(&v).contains(&MissingEvidence::IssueOpen));
}

#[test]
fn blocked_when_self_affecting_but_not_deployed() {
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(true, true, false));
    let v = gate.evaluate(&simard_goal("g", GoalProgress::Completed));
    assert!(blocked_missing(&v).contains(&MissingEvidence::NotDeployed));
}

#[test]
fn deploy_clause_skipped_for_non_self_affecting_goal() {
    // A goal targeting another repo's own surface: clause 3 does not apply, so
    // even with deployed=false (which the gate must ignore) it can complete.
    let mut goal = simard_goal("g", GoalProgress::Completed);
    goal.repo = Some("amplihack-rs".to_string());
    goal.description = "improve amplihack-rs internals".to_string();
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(true, true, false));
    let v = gate.evaluate(&goal);
    match v {
        CompletionVerdict::Complete(ev) => {
            assert!(!ev.self_affecting);
            assert!(ev.deployed, "deployed is unconditionally true off-target");
        }
        other => panic!("expected Complete (clause 3 skipped), got {other:?}"),
    }
}

#[test]
fn blocked_could_not_verify_on_source_error_never_completes() {
    // A transient gh/git failure must block (fail-closed), not complete.
    let gate = CompletionEvidenceGate::new(FakeEvidence {
        pr_merged: Err("gh timed out".to_string()),
        issue_closed: Ok(true),
        deployed: Ok(true),
    });
    let v = gate.evaluate(&simard_goal("g", GoalProgress::Completed));
    assert!(!v.is_complete());
    let missing = blocked_missing(&v);
    assert!(
        matches!(
            missing.first(),
            Some(MissingEvidence::CouldNotVerify { .. })
        ),
        "got {missing:?}"
    );
}

#[test]
fn all_three_missing_are_reported_together() {
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(false, false, false));
    let v = gate.evaluate(&simard_goal("g", GoalProgress::Completed));
    let missing = blocked_missing(&v);
    assert!(missing.contains(&MissingEvidence::PrNotMerged));
    assert!(missing.contains(&MissingEvidence::IssueOpen));
    assert!(missing.contains(&MissingEvidence::NotDeployed));
}

// --- is_self_affecting classifier -------------------------------------------

#[test]
fn classifier_treats_default_repo_as_self_affecting() {
    assert!(is_self_affecting(&simard_goal(
        "g",
        GoalProgress::Completed
    )));
}

#[test]
fn classifier_treats_explicit_simard_slug_as_self_affecting() {
    let mut g = simard_goal("g", GoalProgress::Completed);
    g.repo = Some("Simard".to_string());
    assert!(is_self_affecting(&g));
    g.repo = Some("simard".to_string()); // case-insensitive
    assert!(is_self_affecting(&g));
}

#[test]
fn classifier_excludes_other_repo_surface() {
    let mut g = simard_goal("g", GoalProgress::Completed);
    g.repo = Some("amplihack-rs".to_string());
    g.description = "refactor amplihack-rs scheduler".to_string();
    assert!(!is_self_affecting(&g));
}

#[test]
fn classifier_includes_dependency_pin_bump_even_off_repo() {
    // A #2403-style goal: bumps Simard's own Cargo.toml pin while routed
    // elsewhere — still self-affecting (the rebuilt binary must run).
    let mut g = simard_goal("g", GoalProgress::Completed);
    g.repo = Some("amplihack-rs".to_string());
    g.description = "bump amplihack-memory pinned rev in Simard's Cargo.toml".to_string();
    assert!(is_self_affecting(&g));
}

#[test]
fn classifier_includes_pin_bump_via_wip_ref_touching_cargo_toml() {
    let mut g = simard_goal("g", GoalProgress::Completed);
    g.repo = Some("amplihack-rs".to_string());
    g.description = "land upstream fix".to_string();
    g.wip_refs = vec![WipRef {
        kind: "file".to_string(),
        ref_id: "1".to_string(),
        label: "Cargo.toml".to_string(),
        url: None,
    }];
    assert!(is_self_affecting(&g));
}

#[test]
fn classifier_excludes_explicit_docs_only_goal() {
    let mut g = simard_goal("g", GoalProgress::Completed);
    g.description = "docs-only: refresh the architecture reference".to_string();
    assert!(!is_self_affecting(&g));
}

// --- archive integration + HEADLINE false-completion reproduction -----------

#[test]
fn archive_keeps_goal_active_when_gate_blocks() {
    // Reproduces the cognitive-memory backup false-completion:
    // status == Completed, but NO merged PR and the linked issue still OPEN.
    // The legacy archive_completed would have removed it; the evidence-aware
    // archive must KEEP it active and record the blocker.
    let backup_goal = {
        let mut g = simard_goal("cognitive-memory-backup", GoalProgress::Completed);
        g.description = "make cognitive-memory backups work".to_string();
        g.wip_refs = vec![WipRef {
            kind: "issue".to_string(),
            ref_id: "9999".to_string(),
            label: "backups broken".to_string(),
            url: None,
        }];
        g
    };

    // Contrast: the legacy unguarded path WOULD archive (the bug).
    let mut legacy_board = GoalBoard::new();
    legacy_board.active.push(backup_goal.clone());
    let legacy_archived = archive_completed(&mut legacy_board);
    assert_eq!(
        legacy_archived.len(),
        1,
        "legacy archive_completed silently archives the false completion (the bug)"
    );
    assert!(legacy_board.active.is_empty());

    // Fix: evidence-aware archive keeps it active with a recorded blocker.
    let mut board = GoalBoard::new();
    board.active.push(backup_goal.clone());
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(false, false, false));
    let (archived, blocked) = archive_completed_with_evidence(&mut board, &gate);

    assert!(archived.is_empty(), "must NOT archive without evidence");
    assert_eq!(board.active.len(), 1, "goal stays on the active board");
    assert_eq!(board.active[0].id, "cognitive-memory-backup");

    assert_eq!(blocked.len(), 1);
    let (blocked_goal, missing) = &blocked[0];
    assert_eq!(blocked_goal.id, "cognitive-memory-backup");
    assert!(missing.contains(&MissingEvidence::PrNotMerged));
    assert!(missing.contains(&MissingEvidence::IssueOpen));

    // The retained goal is annotated so the dashboard / next cycle see why.
    let annotated = board.active[0].current_activity.as_deref().unwrap_or("");
    assert!(
        annotated.to_lowercase().contains("completion blocked"),
        "retained goal must surface the blocker, got: {annotated:?}"
    );
}

#[test]
fn archive_removes_goal_only_with_full_evidence() {
    let mut board = GoalBoard::new();
    board
        .active
        .push(simard_goal("done", GoalProgress::Completed));
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(true, true, true));
    let (archived, blocked) = archive_completed_with_evidence(&mut board, &gate);
    assert_eq!(archived.len(), 1);
    assert!(blocked.is_empty());
    assert!(board.active.is_empty());
}

#[test]
fn archive_leaves_non_complete_goals_untouched() {
    let mut board = GoalBoard::new();
    board
        .active
        .push(simard_goal("wip", GoalProgress::InProgress { percent: 40 }));
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(true, true, true));
    let (archived, blocked) = archive_completed_with_evidence(&mut board, &gate);
    assert!(archived.is_empty());
    assert!(blocked.is_empty());
    assert_eq!(
        board.active.len(),
        1,
        "non-candidate goal is retained as-is"
    );
    assert!(board.active[0].current_activity.is_none());
}

// --- kill-switch ------------------------------------------------------------

#[test]
#[serial_test::serial(simard_completion_evidence_env, cognitive_memory)]
fn kill_switch_off_disables_gate() {
    let prev = std::env::var("SIMARD_COMPLETION_EVIDENCE").ok();

    unsafe {
        std::env::set_var("SIMARD_COMPLETION_EVIDENCE", "off");
    }
    assert!(!completion_evidence_enabled());

    unsafe {
        std::env::set_var("SIMARD_COMPLETION_EVIDENCE", "on");
    }
    assert!(completion_evidence_enabled());

    unsafe {
        std::env::remove_var("SIMARD_COMPLETION_EVIDENCE");
    }
    assert!(completion_evidence_enabled(), "unset keeps the gate active");

    // Restore prior environment.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("SIMARD_COMPLETION_EVIDENCE", v),
            None => std::env::remove_var("SIMARD_COMPLETION_EVIDENCE"),
        }
    }
}

// --- prompt content-pins -----------------------------------------------------

const GOAL_CURATOR_PROMPT: &str =
    include_str!("../../../prompt_assets/simard/goal_curator_system.md");
const OODA_DECIDE_PROMPT: &str = include_str!("../../../prompt_assets/simard/ooda_decide.md");

#[test]
fn goal_curator_prompt_pins_done_gate_wording() {
    assert!(
        GOAL_CURATOR_PROMPT.contains(
            "A goal is complete only with a merged PR, a closed linked issue, and — for changes to Simard's own running code — a verified deploy."
        ),
        "goal_curator_system.md must pin the three-part done-gate sentence"
    );
}

#[test]
fn ooda_decide_prompt_pins_done_gate_wording() {
    assert!(
        OODA_DECIDE_PROMPT.contains(
            "Do not propose STATUS: ACHIEVED without merged + closed + (if self-affecting) deployed evidence."
        ),
        "ooda_decide.md must pin the STATUS: ACHIEVED evidence sentence"
    );
}

// --- #2456 verification outcome (extends the gate, does not duplicate it) ----

/// A goal targeting another repo's surface, with no PR/issue refs — so it has
/// NO derivable external completion signal.
fn no_signal_goal(id: &str) -> ActiveGoal {
    let mut g = simard_goal(id, GoalProgress::Completed);
    g.repo = Some("amplihack-rs".to_string());
    g.description = "improve amplihack-rs internals".to_string();
    g.wip_refs = vec![];
    g
}

#[test]
fn outcome_verified_when_gate_completes() {
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(true, true, true));
    let goal = simard_goal("g", GoalProgress::Completed);
    let verdict = gate.evaluate(&goal);
    assert_eq!(
        classify_outcome(&goal, &verdict),
        VerificationOutcome::Verified
    );
}

#[test]
fn outcome_refuted_when_derivable_signal_contradicts_claim() {
    // Self-affecting goal (routes to Simard) claimed done, but PR not merged:
    // a derivable signal says "not done" → a false completion.
    let goal = simard_goal("g", GoalProgress::Completed);
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(false, true, true));
    let verdict = gate.evaluate(&goal);
    assert_eq!(
        classify_outcome(&goal, &verdict),
        VerificationOutcome::Refuted
    );
    assert!(VerificationOutcome::Refuted.is_false_completion());
}

#[test]
fn outcome_unverified_when_no_external_signal_derivable() {
    // No PR/issue ref and not self-affecting: nothing to verify against. The
    // gate blocks, but the honest label is "unverified", NOT "refuted" — and
    // certainly not silently verified on the self-report.
    let goal = no_signal_goal("g");
    assert!(!has_derivable_signal(&goal));
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(false, true, true));
    let verdict = gate.evaluate(&goal);
    assert_eq!(
        classify_outcome(&goal, &verdict),
        VerificationOutcome::UnverifiedNoSignal
    );
}

#[test]
fn outcome_error_when_verification_query_failed() {
    // A transient gh/git failure is "unknown", distinct from a refutation —
    // even when the goal does have a derivable signal.
    let goal = simard_goal("g", GoalProgress::Completed);
    let gate = CompletionEvidenceGate::new(FakeEvidence {
        pr_merged: Err("gh timed out".to_string()),
        issue_closed: Ok(true),
        deployed: Ok(true),
    });
    let verdict = gate.evaluate(&goal);
    assert_eq!(
        classify_outcome(&goal, &verdict),
        VerificationOutcome::Error
    );
}

#[test]
fn classify_from_missing_prioritizes_could_not_verify() {
    // CouldNotVerify dominates even alongside a real refutation signal.
    let goal = simard_goal("g", GoalProgress::Completed);
    let missing = vec![
        MissingEvidence::PrNotMerged,
        MissingEvidence::CouldNotVerify {
            detail: "boom".to_string(),
        },
    ];
    assert_eq!(
        classify_from_missing(&goal, &missing),
        VerificationOutcome::Error
    );
}

#[test]
fn has_derivable_signal_detects_pr_issue_or_self_affecting() {
    // Self-affecting (default repo) → signal via deploy state.
    assert!(has_derivable_signal(&simard_goal(
        "g",
        GoalProgress::Completed
    )));

    // Off-repo with a tracked PR → signal via the PR.
    let mut pr_goal = no_signal_goal("g");
    pr_goal.wip_refs = vec![WipRef {
        kind: "pr".to_string(),
        ref_id: "42".to_string(),
        label: "the fix".to_string(),
        url: None,
    }];
    assert!(has_derivable_signal(&pr_goal));

    // Off-repo, no refs → no derivable signal.
    assert!(!has_derivable_signal(&no_signal_goal("g")));
}

#[test]
fn false_completion_rate_is_refuted_share_of_checkable() {
    use VerificationOutcome::*;
    // No checkable outcomes → None.
    assert!(false_completion_rate(&[]).is_none());
    assert!(false_completion_rate(&[UnverifiedNoSignal, Error]).is_none());
    assert_eq!(false_completion_rate(&[Verified, Verified]).unwrap(), 0.0);
    assert_eq!(false_completion_rate(&[Refuted, Refuted]).unwrap(), 1.0);
    // Signal-less / error are excluded from the denominator: refuted 1 of
    // (verified 1 + refuted 1) = 0.5, NOT 1/4.
    let mixed = [Verified, Refuted, UnverifiedNoSignal, Error];
    assert!((false_completion_rate(&mixed).unwrap() - 0.5).abs() < 1e-12);
}

#[test]
fn outcome_metric_labels_and_codes_are_stable() {
    // Pin the metric vocabulary from issue #2456.
    assert_eq!(
        COMPLETION_VERIFICATION_METRIC,
        "goal_completion_verification"
    );
    assert_eq!(FALSE_COMPLETION_RATE_METRIC, "goal_false_completion_rate");
    assert_eq!(VerificationOutcome::Verified.metric_label(), "verified");
    assert_eq!(
        VerificationOutcome::UnverifiedNoSignal.metric_label(),
        "unverified_no_signal"
    );
    assert_eq!(VerificationOutcome::Refuted.metric_label(), "refuted");
    assert_eq!(VerificationOutcome::Error.metric_label(), "error");
    // Codes are distinct so a time series can aggregate by value.
    let codes = [
        VerificationOutcome::Verified.metric_code(),
        VerificationOutcome::UnverifiedNoSignal.metric_code(),
        VerificationOutcome::Refuted.metric_code(),
        VerificationOutcome::Error.metric_code(),
    ];
    for (i, a) in codes.iter().enumerate() {
        for b in &codes[i + 1..] {
            assert!((a - b).abs() > f64::EPSILON, "codes must be distinct");
        }
    }
}

#[test]
fn metric_recorders_are_test_safe_noops_across_batch_shapes() {
    use VerificationOutcome::*;
    // Both recorders are cfg!(test)-guarded no-ops, so they must never write the
    // operator's real metrics.jsonl from a unit test — and must not panic on any
    // batch shape, including empty / non-checkable batches (no rate to emit).
    record_completion_verification(Verified);
    record_completion_verification(Refuted);
    record_completion_verification(UnverifiedNoSignal);
    record_completion_verification(Error);

    record_false_completion_rate(&[]);
    record_false_completion_rate(&[UnverifiedNoSignal, Error]); // not checkable → None
    record_false_completion_rate(&[Verified, Refuted, UnverifiedNoSignal, Error]);

    // The value the batch recorder would emit is exactly false_completion_rate.
    let mixed = [Verified, Refuted, UnverifiedNoSignal, Error];
    assert!((false_completion_rate(&mixed).unwrap() - 0.5).abs() < 1e-12);
}

// --- helpers ----------------------------------------------------------------

fn blocked_missing(v: &CompletionVerdict) -> Vec<MissingEvidence> {
    match v {
        CompletionVerdict::Blocked { missing, .. } => missing.clone(),
        CompletionVerdict::Complete(_) => panic!("expected Blocked, got Complete"),
    }
}

// --- error_class_from_missing (#2458 link to the failure→lesson loop) ------

#[test]
fn error_class_from_missing_maps_each_kind_to_a_stable_token() {
    assert_eq!(
        error_class_from_missing(&[MissingEvidence::PrNotMerged]),
        "pr_not_merged"
    );
    assert_eq!(
        error_class_from_missing(&[MissingEvidence::IssueOpen]),
        "issue_open"
    );
    assert_eq!(
        error_class_from_missing(&[MissingEvidence::NotDeployed]),
        "not_deployed"
    );
}

#[test]
fn error_class_from_missing_joins_multiple_in_check_order() {
    // The gate pushes PR → issue → deploy; the class preserves that order so the
    // same refutation always yields the same key.
    let class = error_class_from_missing(&[
        MissingEvidence::PrNotMerged,
        MissingEvidence::IssueOpen,
        MissingEvidence::NotDeployed,
    ]);
    assert_eq!(class, "pr_not_merged__issue_open__not_deployed");
}

#[test]
fn error_class_from_missing_is_deterministic_and_dedups() {
    let a = error_class_from_missing(&[MissingEvidence::IssueOpen, MissingEvidence::IssueOpen]);
    let b = error_class_from_missing(&[MissingEvidence::IssueOpen]);
    assert_eq!(a, b, "duplicate kinds must not duplicate tokens");
}

#[test]
fn error_class_from_missing_excludes_could_not_verify() {
    // `CouldNotVerify` is the `Error` outcome, never a refutation — it must not
    // leak into a concrete failure class. A PR refutation alongside it keeps
    // only the concrete token.
    let class = error_class_from_missing(&[
        MissingEvidence::CouldNotVerify {
            detail: "gh timeout".to_string(),
        },
        MissingEvidence::PrNotMerged,
    ]);
    assert_eq!(class, "pr_not_merged");
}

#[test]
fn error_class_from_missing_defaults_when_no_concrete_kind() {
    // Defensive: a list with only `CouldNotVerify` (which never classifies as
    // `Refuted`) yields the sentinel rather than an empty string.
    let class = error_class_from_missing(&[MissingEvidence::CouldNotVerify {
        detail: "x".to_string(),
    }]);
    assert_eq!(class, "refuted_unknown");
    assert_eq!(error_class_from_missing(&[]), "refuted_unknown");
}

// --- standing / perpetual goals never archive (issue #2580) -----------------

/// Build a standing/perpetual goal at the given status.
fn standing_goal(id: &str, status: GoalProgress) -> ActiveGoal {
    let mut g = simard_goal(id, status);
    g.description = "Continuously research and improve cognition. STANDING PERPETUAL goal.".into();
    assert!(g.is_perpetual(), "test fixture must read as perpetual");
    g
}

#[test]
fn gate_never_archives_a_perpetual_goal_even_with_full_evidence() {
    let mut board = GoalBoard::new();
    board
        .active
        .push(standing_goal("research", GoalProgress::Completed));
    // Full evidence would archive a normal goal; a standing goal must not.
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(true, true, true));
    let (archived, blocked) = archive_completed_with_evidence(&mut board, &gate);
    assert!(
        archived.is_empty(),
        "a standing goal must never be archived by the completion gate"
    );
    assert!(blocked.is_empty());
    assert_eq!(board.active.len(), 1, "standing goal stays on the board");
    // ...and it is rolled to a fresh, actionable cycle rather than left as done.
    assert_eq!(board.active[0].status, GoalProgress::NotStarted);
    assert!(board.active[0].is_perpetual());
}

#[test]
fn archive_completed_rolls_perpetual_goal_instead_of_removing() {
    let mut board = GoalBoard::new();
    board
        .active
        .push(standing_goal("research", GoalProgress::Completed));
    board
        .active
        .push(simard_goal("normal-done", GoalProgress::Completed));

    let archived = archive_completed(&mut board);

    // The normal goal archives; the standing goal is rolled and retained.
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, "normal-done");
    let research = board
        .active
        .iter()
        .find(|g| g.id == "research")
        .expect("standing goal must remain on the board");
    assert_eq!(research.status, GoalProgress::NotStarted);
    assert!(research.assigned_to.is_none());
    assert!(research.is_perpetual());
}

#[test]
fn is_complete_candidate_is_false_for_perpetual_goals() {
    // Even at 100% progress, a standing goal is not a completion candidate.
    let done = standing_goal("s", GoalProgress::Completed);
    let full = standing_goal("s2", GoalProgress::InProgress { percent: 100 });
    assert!(!super::is_complete_candidate(&done));
    assert!(!super::is_complete_candidate(&full));
    // A normal completed goal still is a candidate (regression guard).
    assert!(super::is_complete_candidate(&simard_goal(
        "n",
        GoalProgress::Completed
    )));
}

// --- MissingEvidence::label + render_missing (surfaced to dashboard/logs) ----

#[test]
fn missing_evidence_label_is_stable_for_every_kind() {
    // These strings are surfaced verbatim in `current_activity` and logs, so a
    // change is a behavior change the dashboard sees — pin each one.
    assert_eq!(MissingEvidence::PrNotMerged.label(), "PR not merged");
    assert_eq!(MissingEvidence::IssueOpen.label(), "issue still open");
    assert_eq!(
        MissingEvidence::NotDeployed.label(),
        "merged but not deployed"
    );
    assert_eq!(
        MissingEvidence::CouldNotVerify {
            detail: "gh 503".to_string(),
        }
        .label(),
        "could not verify: gh 503"
    );
}

#[test]
fn render_missing_joins_labels_with_semicolons_in_order() {
    // The rendered blocker string preserves the gate's PR → issue → deploy order
    // and separates entries with "; " (empty list renders empty).
    assert_eq!(super::render_missing(&[]), "");
    assert_eq!(
        super::render_missing(&[MissingEvidence::PrNotMerged]),
        "PR not merged"
    );
    assert_eq!(
        super::render_missing(&[
            MissingEvidence::PrNotMerged,
            MissingEvidence::IssueOpen,
            MissingEvidence::NotDeployed,
        ]),
        "PR not merged; issue still open; merged but not deployed"
    );
}

// --- archive_completed_evidence_aware: the public, kill-switch-aware entry ----
//
// This is the production entrypoint the daemon calls each cycle. It composes
// the gate, drives the `&dyn EvidenceSource` blanket impl, and (gate-on) emits
// the #2456 verification metrics as cfg!(test) no-ops. The tests exercise it
// hermetically through an in-memory board + the canned `FakeEvidence`.

#[test]
fn evidence_aware_archive_archives_fully_verified_goal() {
    // Gate ON (default env): a Completed goal with full evidence archives.
    let mut board = GoalBoard::new();
    board
        .active
        .push(simard_goal("verified", GoalProgress::Completed));
    let (archived, blocked) =
        archive_completed_evidence_aware(&mut board, &FakeEvidence::ok(true, true, true));
    assert_eq!(archived.len(), 1, "fully-verified goal must archive");
    assert_eq!(archived[0].id, "verified");
    assert!(blocked.is_empty());
    assert!(board.active.is_empty());
}

#[test]
// `archive_completed_evidence_aware` reads `SIMARD_COMPLETION_EVIDENCE` via
// `completion_evidence_enabled()`. The kill-switch tests flip that var (under the
// same serial key), and this test's outcome DIFFERS between gate-on (blocked,
// retained) and the legacy off path (the Completed goal is archived) — so it must
// be serialized against those mutators or it can flake if it reads "off" mid-race.
#[serial_test::serial(simard_completion_evidence_env)]
fn evidence_aware_archive_retains_and_annotates_unverified_goals() {
    // Gate ON: a Completed goal with NO merge/close/deploy evidence must stay on
    // the board, be reported as blocked, and carry a human-readable annotation.
    let mut board = GoalBoard::new();
    board
        .active
        .push(simard_goal("unverified", GoalProgress::Completed));
    let (archived, blocked) =
        archive_completed_evidence_aware(&mut board, &FakeEvidence::ok(false, false, false));

    assert!(
        archived.is_empty(),
        "no goal archives without evidence, got {archived:?}"
    );
    assert_eq!(blocked.len(), 1, "the completed goal is blocked");
    let (blocked_goal, missing) = &blocked[0];
    assert_eq!(blocked_goal.id, "unverified");
    assert!(missing.contains(&MissingEvidence::PrNotMerged));
    assert_eq!(board.active.len(), 1, "blocked goal stays on the board");
    let activity = board.active[0].current_activity.as_deref().unwrap_or("");
    assert!(
        activity.to_lowercase().contains("completion blocked"),
        "retained goal must surface the blocker, got {activity:?}"
    );
}

#[test]
#[serial_test::serial(simard_completion_evidence_env, cognitive_memory)]
fn evidence_aware_archive_falls_back_to_legacy_when_kill_switch_off() {
    // With SIMARD_COMPLETION_EVIDENCE=off, the evidence-aware archive must behave
    // exactly like the legacy unguarded `archive_completed`: a Completed goal is
    // archived regardless of (absent) evidence, and `blocked` is always empty.
    let prev = std::env::var("SIMARD_COMPLETION_EVIDENCE").ok();
    // SAFETY: serialised via serial(...); no concurrent reader/writer of this key.
    unsafe {
        std::env::set_var("SIMARD_COMPLETION_EVIDENCE", "off");
    }

    let mut board = GoalBoard::new();
    board
        .active
        .push(simard_goal("legacy-done", GoalProgress::Completed));
    // Evidence says "nothing merged/closed/deployed"; the gate would block — but
    // it is disabled, so the legacy path archives anyway.
    let (archived, blocked) =
        archive_completed_evidence_aware(&mut board, &FakeEvidence::ok(false, false, false));

    // Restore env before any assertion can unwind.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("SIMARD_COMPLETION_EVIDENCE", v),
            None => std::env::remove_var("SIMARD_COMPLETION_EVIDENCE"),
        }
    }

    assert_eq!(
        archived.len(),
        1,
        "kill-switch-off restores unguarded archiving"
    );
    assert_eq!(archived[0].id, "legacy-done");
    assert!(
        blocked.is_empty(),
        "legacy mode never reports blocked goals"
    );
    assert!(
        board.active.is_empty(),
        "the goal was archived off the board"
    );
}

#[test]
fn evidence_aware_archive_leaves_incomplete_goals_in_place() {
    // A below-100% in-progress goal is not a completion candidate, so it is
    // neither archived nor blocked, and its annotation is left untouched.
    let mut board = GoalBoard::new();
    board
        .active
        .push(simard_goal("wip", GoalProgress::InProgress { percent: 25 }));
    let (archived, blocked) =
        archive_completed_evidence_aware(&mut board, &FakeEvidence::ok(true, true, true));
    assert!(archived.is_empty());
    assert!(blocked.is_empty());
    assert_eq!(board.active.len(), 1);
    assert!(board.active[0].current_activity.is_none());
}

// --- GhCliEvidenceSource: hermetic pure logic (repo slug + no-ref clauses) ---
//
// The `gh`/git-touching methods (gh_state, is_deployed) are deliberately NOT
// exercised here — they shell out and are non-hermetic. These tests cover the
// pure, network-free surface: repo-slug resolution and the "no tracked ref"
// short-circuits that must answer WITHOUT any subprocess.

#[test]
fn gh_source_repo_slug_resolves_all_four_forms() {
    let source = GhCliEvidenceSource::new("/nonexistent/repo/dir");

    // None → the default owner/repo.
    let mut g = simard_goal("g", GoalProgress::Completed);
    g.repo = None;
    assert_eq!(source.repo_slug(&g), "rysweet/Simard");

    // Explicit "Simard" (case-insensitive) → the default.
    g.repo = Some("Simard".to_string());
    assert_eq!(source.repo_slug(&g), "rysweet/Simard");
    g.repo = Some("sImArD".to_string());
    assert_eq!(source.repo_slug(&g), "rysweet/Simard");

    // Already-qualified owner/repo → verbatim.
    g.repo = Some("octo/widgets".to_string());
    assert_eq!(source.repo_slug(&g), "octo/widgets");

    // Bare slug → scoped under the default owner.
    g.repo = Some("amplihack-rs".to_string());
    assert_eq!(source.repo_slug(&g), "rysweet/amplihack-rs");
}

#[test]
fn gh_source_no_pr_ref_reports_unmerged_without_network() {
    // A goal with no PR wip_ref must resolve `any_pr_merged == false` cheaply and
    // hermetically (the branch that short-circuits before any `gh` call).
    let source = GhCliEvidenceSource::new("/nonexistent/repo/dir");
    let goal = no_signal_goal("g"); // off-repo, no wip_refs
    assert!(
        !source
            .any_pr_merged(&goal)
            .expect("no-ref path must not error"),
        "no tracked PR ⇒ no merge evidence"
    );
}

#[test]
fn gh_source_no_issue_ref_reports_closed_without_network() {
    // No issue wip_ref ⇒ nothing open to gate on ⇒ the clause vacuously holds
    // (`issue_closed == true`), again with no `gh` call.
    let source = GhCliEvidenceSource::new("/nonexistent/repo/dir");
    let goal = no_signal_goal("g");
    assert!(
        source
            .issue_closed(&goal)
            .expect("no-ref path must not error"),
        "no tracked issue ⇒ clause holds vacuously"
    );
}

#[test]
fn gh_source_first_ref_of_kind_matches_case_insensitively() {
    // `first_ref_of_kind` underpins both no-ref short-circuits; pin its matching.
    let mut goal = no_signal_goal("g");
    goal.wip_refs = vec![
        WipRef {
            kind: "PR".to_string(),
            ref_id: "101".to_string(),
            label: "the pr".to_string(),
            url: None,
        },
        WipRef {
            kind: "issue".to_string(),
            ref_id: "202".to_string(),
            label: "the issue".to_string(),
            url: None,
        },
    ];
    assert_eq!(super::first_ref_of_kind(&goal, "pr"), Some("101"));
    assert_eq!(super::first_ref_of_kind(&goal, "ISSUE"), Some("202"));
    assert_eq!(super::first_ref_of_kind(&goal, "commit"), None);
}

// ===========================================================================
// issue #4375 — cross-repo merged-PR completion reconciliation
//
// Root cause: `GhCliEvidenceSource::any_pr_merged` resolved the merged-PR check
// against the DEFAULT repo (rysweet/Simard) whenever `goal.repo` was `None`, and
// it read only `WipRef.ref_id` — never `WipRef.url`. So a genuinely-merged PR in
// a cross-repo goal (e.g. the 8 kgpacks goals targeting rysweet/agent-kgpacks-rs
// whose PR linkage was only ever persisted as a URL) could never be observed as
// MERGED. Those goals re-emitted `completion BLOCKED … PR not merged` every OODA
// cycle (21×/goal, 189 total over 6h) and never converged.
//
// The fix introduces a pure `parse_pr_url` helper and a private `resolve_pr_target`
// resolver on `GhCliEvidenceSource` so the merged-PR gate resolves against each
// goal's OWN target repo and reads the persisted PR/URL linkage. These tests are
// hermetic (no `gh`, no network): the pure helper is tested directly, the resolver
// is tested directly, and the archive flow is exercised through a goal-aware
// `RepoAwareEvidence` double that keys its merged answer off the REAL resolver.
// ===========================================================================

/// Build a "pr"-kind `WipRef` with an optional URL, for the #4375 tests.
fn pr_ref(ref_id: &str, url: Option<&str>) -> WipRef {
    WipRef {
        kind: "pr".to_string(),
        ref_id: ref_id.to_string(),
        label: "the pr".to_string(),
        url: url.map(|u| u.to_string()),
    }
}

/// Goal-aware evidence double for issue #4375. Reports `pr_merged == true` only
/// when the goal resolves — through the production `resolve_pr_target` — to
/// `merged_repo` with a PR number. This deliberately exercises the REAL resolver
/// (not a goal-ignoring constant like `FakeEvidence`), so the archive regression
/// fails unless the cross-repo/URL resolution fix is in place. `issue_closed` and
/// `is_deployed` are held true so the merged-PR clause is the sole gate under test.
struct RepoAwareEvidence {
    /// The `owner/repo` slug whose PR is merged; any other resolution ⇒ unmerged.
    merged_repo: String,
    /// The production resolver under test (rooted at a nonexistent dir; the
    /// resolver never touches the filesystem/network — it is pure).
    resolver: GhCliEvidenceSource,
}

impl RepoAwareEvidence {
    fn merged_in(repo: &str) -> Self {
        Self {
            merged_repo: repo.to_string(),
            resolver: GhCliEvidenceSource::new("/nonexistent/repo/dir"),
        }
    }
}

impl EvidenceSource for RepoAwareEvidence {
    fn any_pr_merged(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        match self.resolver.resolve_pr_target(goal) {
            Some((repo, _num)) => Ok(repo == self.merged_repo),
            None => Ok(false),
        }
    }
    fn issue_closed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(true)
    }
    fn is_deployed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(true)
    }
}

// --- parse_pr_url: pure, panic-free URL → (owner/repo, num) -----------------

#[test]
fn parse_pr_url_extracts_owner_repo_and_number() {
    assert_eq!(
        parse_pr_url("https://github.com/rysweet/agent-kgpacks-rs/pull/42"),
        Some(("rysweet/agent-kgpacks-rs".to_string(), "42".to_string()))
    );
}

#[test]
fn parse_pr_url_accepts_trailing_path_segments() {
    // `/pull/<digits>` followed by `/files`, `/commits`, etc. is still a PR URL.
    assert_eq!(
        parse_pr_url("https://github.com/octo/widgets/pull/7/files"),
        Some(("octo/widgets".to_string(), "7".to_string()))
    );
}

#[test]
fn parse_pr_url_rejects_issue_url() {
    // An issue URL is NOT a merged-PR signal — must not be mistaken for one.
    assert_eq!(
        parse_pr_url("https://github.com/rysweet/Simard/issues/99"),
        None
    );
}

#[test]
fn parse_pr_url_rejects_garbage_and_non_digit_number() {
    assert_eq!(parse_pr_url("not a url"), None);
    assert_eq!(parse_pr_url(""), None);
    // A non-digit PR "number" must be rejected (would be an invalid gh arg).
    assert_eq!(parse_pr_url("https://github.com/o/r/pull/abc"), None);
    // A missing number after /pull/ is rejected.
    assert_eq!(parse_pr_url("https://github.com/o/r/pull/"), None);
}

#[test]
fn parse_pr_url_rejects_argument_injection_shaped_slug() {
    // Security: a leading-dash owner/repo would be an argument-injection vector
    // into `gh --repo <slug>`; it must never survive parsing.
    assert_eq!(
        parse_pr_url("https://github.com/-badowner/repo/pull/1"),
        None
    );
    assert_eq!(
        parse_pr_url("https://github.com/owner/-badrepo/pull/1"),
        None
    );
    // Whitespace in the slug is rejected too.
    assert_eq!(parse_pr_url("https://github.com/ow ner/repo/pull/1"), None);
}

#[test]
fn parse_pr_url_is_total_on_pathological_input() {
    // Must be pure & panic-free on adversarial/empty/oversized input.
    let _ = parse_pr_url("");
    let _ = parse_pr_url("/pull/");
    let _ = parse_pr_url("https://github.com//pull/1");
    let huge = format!("https://github.com/o/r/pull/{}", "9".repeat(10_000));
    let _ = parse_pr_url(&huge); // no panic, no unbounded work
}

// --- resolve_pr_target: the (repo_slug, pr_num) precedence resolver ----------

#[test]
fn resolve_pr_target_uses_qualified_goal_repo_with_numeric_ref_id() {
    // Precedence 1: an already-qualified `goal.repo` + numeric `ref_id` resolves
    // atomically to that repo and number.
    let source = GhCliEvidenceSource::new("/nonexistent/repo/dir");
    let mut g = simard_goal("kg", GoalProgress::Completed);
    g.repo = Some("rysweet/agent-kgpacks-rs".to_string());
    g.wip_refs = vec![pr_ref("18", None)];
    assert_eq!(
        source.resolve_pr_target(&g),
        Some(("rysweet/agent-kgpacks-rs".to_string(), "18".to_string()))
    );
}

#[test]
fn resolve_pr_target_recovers_repo_and_number_from_pr_url_when_repo_none() {
    // THE kgpacks failure mode: `goal.repo` is `None` (old code defaulted to
    // rysweet/Simard) and the only PR linkage is a cross-repo URL with no numeric
    // `ref_id`. The resolver must recover BOTH the owning repo and the number from
    // the URL — never default to Simard.
    let source = GhCliEvidenceSource::new("/nonexistent/repo/dir");
    let mut g = simard_goal("kg", GoalProgress::Completed);
    g.repo = None;
    g.wip_refs = vec![pr_ref(
        "",
        Some("https://github.com/rysweet/agent-kgpacks-rs/pull/19"),
    )];
    assert_eq!(
        source.resolve_pr_target(&g),
        Some(("rysweet/agent-kgpacks-rs".to_string(), "19".to_string()))
    );
}

#[test]
fn resolve_pr_target_prefers_url_repo_over_simard_default_when_repo_none() {
    // Even when a numeric `ref_id` is present, a `None` `goal.repo` must resolve
    // the repo from the PR URL — NOT silently default to rysweet/Simard (the exact
    // cross-repo done-gate that could never be satisfied before the fix).
    let source = GhCliEvidenceSource::new("/nonexistent/repo/dir");
    let mut g = simard_goal("kg", GoalProgress::Completed);
    g.repo = None;
    g.wip_refs = vec![pr_ref(
        "20",
        Some("https://github.com/rysweet/agent-kgpacks-rs/pull/20"),
    )];
    assert_eq!(
        source.resolve_pr_target(&g),
        Some(("rysweet/agent-kgpacks-rs".to_string(), "20".to_string()))
    );
}

#[test]
fn resolve_pr_target_defaults_to_simard_for_repoless_goal_without_url() {
    // Non-breaking: a genuinely Simard-scoped PR (no `goal.repo`, no URL) still
    // resolves against the default repo, preserving prior behavior.
    let source = GhCliEvidenceSource::new("/nonexistent/repo/dir");
    let mut g = simard_goal("g", GoalProgress::Completed);
    g.repo = None;
    g.wip_refs = vec![pr_ref("500", None)];
    assert_eq!(
        source.resolve_pr_target(&g),
        Some(("rysweet/Simard".to_string(), "500".to_string()))
    );
}

#[test]
fn resolve_pr_target_returns_none_without_any_pr_ref() {
    // Fail-closed: no PR signal at all ⇒ `None` ⇒ `any_pr_merged` short-circuits to
    // Ok(false) with no network. (Distinct from "resolved but unmerged".)
    let source = GhCliEvidenceSource::new("/nonexistent/repo/dir");
    let g = no_signal_goal("g"); // off-repo, no wip_refs
    assert_eq!(source.resolve_pr_target(&g), None);
}

#[test]
fn resolve_pr_target_is_deterministic_across_repeated_calls() {
    // The resolver is pure: repeated evaluation must never flap (the goal must
    // reconcile deterministically, not oscillate block/pass across cycles).
    let source = GhCliEvidenceSource::new("/nonexistent/repo/dir");
    let mut g = simard_goal("kg", GoalProgress::Completed);
    g.repo = None;
    g.wip_refs = vec![pr_ref(
        "21",
        Some("https://github.com/rysweet/agent-kgpacks-rs/pull/21"),
    )];
    let first = source.resolve_pr_target(&g);
    for _ in 0..5 {
        assert_eq!(
            source.resolve_pr_target(&g),
            first,
            "resolution must be stable across cycles"
        );
    }
    assert_eq!(
        first,
        Some(("rysweet/agent-kgpacks-rs".to_string(), "21".to_string()))
    );
}

// --- archive flow: cross-repo merged PR archives; no merge still blocks ------

#[test]
fn cross_repo_completed_goal_with_merged_pr_archives_not_blocks() {
    // HEADLINE #4375 regression. A kgpacks goal marked Completed whose merged PR
    // lives in rysweet/agent-kgpacks-rs, persisted only as a URL with `goal.repo`
    // unset. Before the fix the gate queried the PR against rysweet/Simard (or
    // ignored the URL) and re-blocked EVERY cycle. After the fix the genuinely-
    // merged cross-repo PR satisfies the gate and the goal ARCHIVES.
    let mut board = GoalBoard::new();
    let mut g = simard_goal("fix-agent-kgpacks-rs-issue-18", GoalProgress::Completed);
    g.repo = None; // repo linkage was never persisted — only the PR URL was
    g.description = "land the kgpacks fix".to_string();
    g.wip_refs = vec![pr_ref(
        "18",
        Some("https://github.com/rysweet/agent-kgpacks-rs/pull/18"),
    )];
    board.active.push(g);

    let source = RepoAwareEvidence::merged_in("rysweet/agent-kgpacks-rs");
    let gate = CompletionEvidenceGate::new(&source);
    let (archived, blocked) = archive_completed_with_evidence(&mut board, &gate);

    assert_eq!(
        archived.len(),
        1,
        "a genuinely-merged cross-repo PR must archive the goal, not re-block it"
    );
    assert_eq!(archived[0].id, "fix-agent-kgpacks-rs-issue-18");
    assert!(
        blocked.is_empty(),
        "must NOT re-emit `completion BLOCKED … PR not merged` for a merged goal"
    );
    assert!(
        board.active.is_empty(),
        "archived goal leaves the active board"
    );
}

#[test]
fn cross_repo_completed_goal_with_qualified_repo_archives() {
    // Companion shape: `goal.repo` IS the qualified cross-repo slug and the PR is
    // a numeric ref. This must also archive (it exercises precedence-1 resolution
    // through the real resolver, guarding against regressing the qualified path).
    let mut board = GoalBoard::new();
    let mut g = simard_goal("fix-agent-kgpacks-rs-issue-23", GoalProgress::Completed);
    g.repo = Some("rysweet/agent-kgpacks-rs".to_string());
    g.description = "land another kgpacks fix".to_string();
    g.wip_refs = vec![pr_ref("23", None)];
    board.active.push(g);

    let source = RepoAwareEvidence::merged_in("rysweet/agent-kgpacks-rs");
    let gate = CompletionEvidenceGate::new(&source);
    let (archived, blocked) = archive_completed_with_evidence(&mut board, &gate);

    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, "fix-agent-kgpacks-rs-issue-23");
    assert!(blocked.is_empty());
    assert!(board.active.is_empty());
}

#[test]
fn cross_repo_goal_without_merged_pr_still_blocks() {
    // FAIL-CLOSED invariant: the fix must NOT weaken the gate into a silent
    // always-pass. A goal whose PR resolves to the correct repo but is NOT merged
    // must still block with `PrNotMerged` and stay on the active board.
    let mut board = GoalBoard::new();
    let mut g = simard_goal("fix-agent-kgpacks-rs-issue-99", GoalProgress::Completed);
    g.repo = Some("rysweet/agent-kgpacks-rs".to_string());
    g.description = "kgpacks fix not yet merged".to_string();
    g.wip_refs = vec![pr_ref(
        "99",
        Some("https://github.com/rysweet/agent-kgpacks-rs/pull/99"),
    )];
    board.active.push(g);

    // Nothing is merged in the resolved repo.
    let source = RepoAwareEvidence::merged_in("rysweet/some-other-repo");
    let gate = CompletionEvidenceGate::new(&source);
    let (archived, blocked) = archive_completed_with_evidence(&mut board, &gate);

    assert!(
        archived.is_empty(),
        "an unmerged PR must NOT archive the goal"
    );
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].0.id, "fix-agent-kgpacks-rs-issue-99");
    assert!(
        blocked[0].1.contains(&MissingEvidence::PrNotMerged),
        "unmerged cross-repo PR must still surface PrNotMerged, got {:?}",
        blocked[0].1
    );
    assert_eq!(board.active.len(), 1, "blocked goal stays active");
}

#[test]
fn goal_with_no_pr_ref_still_blocks_fail_closed() {
    // A goal with NO pr `wip_ref` at all must still block (truly no merged PR),
    // resolving to `PrNotMerged` without any network — proving the fix did not
    // turn "no evidence" into a pass.
    let mut board = GoalBoard::new();
    board.active.push(no_signal_goal("no-pr-goal"));
    let source = RepoAwareEvidence::merged_in("rysweet/agent-kgpacks-rs");
    let gate = CompletionEvidenceGate::new(&source);
    let (archived, blocked) = archive_completed_with_evidence(&mut board, &gate);

    assert!(archived.is_empty());
    assert_eq!(blocked.len(), 1);
    assert!(blocked[0].1.contains(&MissingEvidence::PrNotMerged));
    assert_eq!(board.active.len(), 1);
}

#[test]
fn merged_cross_repo_goal_reconciles_deterministically_each_cycle() {
    // The systemic symptom was re-blocking every cycle (burning work indefinitely).
    // Evaluating the same merged cross-repo goal repeatedly must CONVERGE to
    // Complete every time, never oscillate.
    let mut g = simard_goal("fix-agent-kgpacks-rs-issue-22", GoalProgress::Completed);
    g.repo = None;
    g.wip_refs = vec![pr_ref(
        "22",
        Some("https://github.com/rysweet/agent-kgpacks-rs/pull/22"),
    )];
    let source = RepoAwareEvidence::merged_in("rysweet/agent-kgpacks-rs");
    let gate = CompletionEvidenceGate::new(&source);
    for cycle in 0..3 {
        assert!(
            gate.evaluate(&g).is_complete(),
            "cycle {cycle}: merged goal must converge to Complete, not re-block"
        );
    }
}
