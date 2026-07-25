//! Tests for the no-progress breaker ([`super::no_progress_breaker`], Fix 3).
//!
//! These are **test-first** for the un-shipped no-action livelock breaker
//! described in `docs/concepts/steerable-ooda-daemon.md` ("The no-progress
//! breaker (Fix 3)"). The daemon livelocked because repeated `NO ACTION`
//! ("I'll verify concretely…") cycles on the *same* goal were recorded as
//! `success=true` no-ops forever — nothing counted them, and nothing forced a
//! definitive resolution.
//!
//! The breaker lives entirely in `src/goal_curation/` (per the incident's
//! coordination constraint — `ooda_brain`/reasoner/memory files are owned by
//! the naming-cleanup rename) and reuses the Fix-2 done-gate to make the
//! "verify concretely" step real rather than perpetual prose.

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::completion_gate::{CompletionEvidenceGate, EvidenceSource};
use crate::goal_curation::types::{ActiveGoal, GoalProgress, WipRef};

use super::no_progress_breaker::{
    NO_PROGRESS_BREAKER_THRESHOLD, NoProgressResolution, NoProgressTracker, StuckGoalDisposition,
    is_no_progress_marker, no_progress_blocked_reason, obsolescence_reason, resolve_no_progress,
    verify_stuck_goal,
};

// --- fixtures ---------------------------------------------------------------

/// A canned, hermetic [`EvidenceSource`] mirroring the completion-gate test
/// double — each answer is a `Result<bool, String>` so a test can model a clean
/// verdict or a transient query failure.
struct FakeEvidence {
    pr_merged: Result<bool, String>,
    issue_closed: Result<bool, String>,
    deployed: Result<bool, String>,
}

impl FakeEvidence {
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

/// A Simard-repo goal (routes to Simard ⇒ self-affecting) stuck at 0%.
fn stuck_goal(id: &str) -> ActiveGoal {
    let mut g = ActiveGoal::new(id, "harden the supply chain", 1);
    g.status = GoalProgress::NotStarted;
    g
}

fn issue_ref(num: &str, label: &str) -> WipRef {
    WipRef {
        kind: "issue".to_string(),
        ref_id: num.to_string(),
        label: label.to_string(),
        url: None,
    }
}

fn pr_ref(num: &str) -> WipRef {
    WipRef {
        kind: "pr".to_string(),
        ref_id: num.to_string(),
        label: format!("PR #{num}"),
        url: None,
    }
}

// --- the headline guarantee: bounded no-progress ----------------------------

#[test]
fn breaker_fires_at_threshold_and_never_emits_an_extra_no_action_cycle() {
    // The exact livelock reproduction: a goal that yields NO ACTION every cycle
    // must NOT keep returning `Continue` forever. It Continues below the
    // threshold, then fires exactly once at the threshold with a *terminal*
    // resolution — and its counter is cleared so there is no (N+1)th no-op loop.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    assert!(
        threshold >= 2,
        "breaker must be small (2-3), got {threshold}"
    );

    let mut tracker = NoProgressTracker::new();
    let goal = "ladybug-supply-chain";

    for cycle in 1..threshold {
        let res = tracker.record_and_resolve(goal, threshold, || StuckGoalDisposition::Unresolved);
        assert_eq!(
            res,
            NoProgressResolution::Continue,
            "cycle {cycle} (< threshold {threshold}) must Continue"
        );
        assert_eq!(tracker.consecutive(goal), cycle);
    }

    // Threshold cycle: the breaker fires. NOT another Continue.
    let fired = tracker.record_and_resolve(goal, threshold, || StuckGoalDisposition::Unresolved);
    assert!(
        fired.is_terminal(),
        "breaker must produce a terminal resolution at the threshold, got {fired:?}"
    );
    assert!(
        matches!(fired, NoProgressResolution::Escalate { .. }),
        "unresolved goal must escalate, got {fired:?}"
    );

    // The counter is cleared once the breaker fires: the goal has left the
    // no-action loop (blocked/dropped/done), so it can never accumulate an
    // (N+1)th consecutive no-action cycle.
    assert_eq!(
        tracker.consecutive(goal),
        0,
        "counter must reset after the breaker fires"
    );
}

#[test]
fn real_progress_resets_the_no_action_counter() {
    // A no-action cycle followed by concrete progress (an engineer spawn, a
    // commit, a PR) must NOT trip the breaker — only *consecutive* no-action.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let mut tracker = NoProgressTracker::new();
    let goal = "g";

    for _ in 0..(threshold - 1) {
        assert_eq!(
            tracker.record_and_resolve(goal, threshold, || StuckGoalDisposition::Unresolved),
            NoProgressResolution::Continue
        );
    }
    tracker.record_progress(goal);
    assert_eq!(tracker.consecutive(goal), 0);

    // Next no-action starts the count over → Continue, not fire.
    assert_eq!(
        tracker.record_and_resolve(goal, threshold, || StuckGoalDisposition::Unresolved),
        NoProgressResolution::Continue
    );
}

// --- the definitive-resolution ladder ---------------------------------------

#[test]
fn ladder_marks_done_when_evidence_present() {
    let res = resolve_no_progress("g", 3, 3, || StuckGoalDisposition::Done);
    assert_eq!(res, NoProgressResolution::MarkDone);
}

#[test]
fn ladder_drops_when_goal_is_obsolete() {
    let res = resolve_no_progress("g", 3, 3, || StuckGoalDisposition::Obsolete {
        reason: "out-of-scope issue #1 filed".to_string(),
    });
    match res {
        NoProgressResolution::Drop { reason } => assert!(reason.contains("out-of-scope")),
        other => panic!("expected Drop, got {other:?}"),
    }
}

#[test]
fn ladder_escalates_with_sentinel_block_and_issue_when_unresolved() {
    let res = resolve_no_progress("stuck-goal", 3, 3, || StuckGoalDisposition::Unresolved);
    match res {
        NoProgressResolution::Escalate {
            blocked_reason,
            issue_title,
            issue_body,
        } => {
            assert!(
                is_no_progress_marker(&blocked_reason),
                "blocked reason must carry the no-progress sentinel: {blocked_reason:?}"
            );
            assert!(issue_title.contains("stuck-goal"));
            assert!(issue_body.contains("stuck-goal"));
            assert!(
                issue_body.contains("no_progress_breaker"),
                "issue body should attribute the safeguard module"
            );
        }
        other => panic!("expected Escalate, got {other:?}"),
    }
}

#[test]
fn below_threshold_always_continues_without_consulting_the_verifier() {
    // The verification (done-gate) must run ONCE, only when the breaker fires —
    // never on every no-action cycle. A panicking closure proves it is not
    // consulted below the threshold.
    let res = resolve_no_progress("g", 1, 3, || {
        panic!("verifier must not run below threshold")
    });
    assert_eq!(res, NoProgressResolution::Continue);
}

// --- verify_stuck_goal reuses the Fix-2 done-gate ---------------------------

#[test]
fn verify_maps_complete_gate_verdict_to_done() {
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(true, true, true));
    let goal = stuck_goal("g");
    assert_eq!(verify_stuck_goal(&goal, &gate), StuckGoalDisposition::Done);
}

#[test]
fn verify_maps_blocked_out_of_scope_goal_to_obsolete() {
    // Blocked by the gate (issue still open), but the linked issue is an
    // explicit out-of-scope handoff → the goal is obsolete, not escalatable.
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(false, false, false));
    let mut goal = stuck_goal("g");
    goal.wip_refs = vec![issue_ref("42", "filed as out-of-scope; tracked elsewhere")];
    match verify_stuck_goal(&goal, &gate) {
        StuckGoalDisposition::Obsolete { reason } => assert!(reason.contains("out-of-scope")),
        other => panic!("expected Obsolete, got {other:?}"),
    }
}

#[test]
fn verify_maps_blocked_actionable_goal_to_unresolved() {
    // Blocked with no obsolescence signal → a human must resolve it.
    let gate = CompletionEvidenceGate::new(FakeEvidence::ok(false, true, false));
    let mut goal = stuck_goal("g");
    goal.wip_refs = vec![pr_ref("7")]; // an open PR that is not merged
    assert_eq!(
        verify_stuck_goal(&goal, &gate),
        StuckGoalDisposition::Unresolved
    );
}

// --- obsolescence predicate -------------------------------------------------

#[test]
fn obsolescence_detected_from_out_of_scope_issue_label() {
    let mut goal = stuck_goal("g");
    goal.wip_refs = vec![issue_ref("1", "out of scope — split into follow-up")];
    assert!(obsolescence_reason(&goal).is_some());
}

#[test]
fn obsolescence_detected_from_current_activity_marker() {
    let mut goal = stuck_goal("g");
    goal.current_activity = Some("superseded by the new hardening plan".to_string());
    assert!(obsolescence_reason(&goal).is_some());
}

#[test]
fn ordinary_goal_is_not_obsolete() {
    let mut goal = stuck_goal("g");
    goal.wip_refs = vec![pr_ref("9"), issue_ref("3", "harden ladybug-rust deps")];
    goal.current_activity = Some("engineer working the PR".to_string());
    assert!(obsolescence_reason(&goal).is_none());
}

// --- sentinel round-trip ----------------------------------------------------

#[test]
fn no_progress_sentinel_round_trips_and_is_distinct_from_other_reasons() {
    let reason = no_progress_blocked_reason(3);
    assert!(is_no_progress_marker(&reason));
    assert!(!is_no_progress_marker("blocked: waiting on review"));
    assert!(!is_no_progress_marker(
        "\u{1F512} [OODA-SAFEGUARD] OODA brain failing for 3 consecutive cycles; needs human review"
    ));
}

// --- the incident reproduction: the four ladybug supply-chain goals ----------

#[test]
fn four_stuck_supply_chain_goals_all_leave_the_active_loop_via_the_ladder() {
    // The exact production livelock: four supply-chain goals re-selected every
    // cycle, all objectively done — two merged hardening PRs, and an
    // out-of-scope issue filed — yet stuck at 0% forever. With the breaker,
    // after the threshold each terminates: merged-PR goals → MarkDone,
    // out-of-scope-issue goal → Drop. NONE stays in the no-action loop.
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;

    // Two goals whose hardening PRs merged (and their linked issues closed):
    // the done-gate certifies them Complete → MarkDone.
    for (id, pr) in [("ladybug-rust", "1"), ("ladybug-graph-rs", "1")] {
        let mut tracker = NoProgressTracker::new();
        let mut goal = stuck_goal(id);
        goal.wip_refs = vec![pr_ref(pr)];
        let gate = CompletionEvidenceGate::new(FakeEvidence::ok(true, true, true));

        let mut last = NoProgressResolution::Continue;
        for _ in 0..threshold {
            last = tracker.record_and_resolve(id, threshold, || verify_stuck_goal(&goal, &gate));
        }
        assert_eq!(
            last,
            NoProgressResolution::MarkDone,
            "merged-PR goal '{id}' must auto-complete via the ladder"
        );
    }

    // The out-of-scope goal: a filed issue marks it tracked elsewhere → Drop.
    {
        let id = "lbug-patched";
        let mut tracker = NoProgressTracker::new();
        let mut goal = stuck_goal(id);
        goal.wip_refs = vec![issue_ref(
            "1",
            "out-of-scope for this daemon; filed upstream",
        )];
        let gate = CompletionEvidenceGate::new(FakeEvidence::ok(false, false, false));

        let mut last = NoProgressResolution::Continue;
        for _ in 0..threshold {
            last = tracker.record_and_resolve(id, threshold, || verify_stuck_goal(&goal, &gate));
        }
        match last {
            NoProgressResolution::Drop { .. } => {}
            other => panic!("out-of-scope goal '{id}' must Drop, got {other:?}"),
        }
    }
}

// ===========================================================================
// #4441 — awaiting-merge disposition (TEST-FIRST / failing until implemented).
//
// A goal whose workstream already has an OPEN, non-draft, MERGEABLE PR is
// awaiting an external merge — NOT stalled. The breaker must classify it
// `AwaitingMerge` → `AwaitMerge` (non-terminal) instead of reaping/escalating
// it, which is what produced the duplicate PRs in the live incident. See
// docs/reference/no-progress-awaiting-merge-api.md.
// ===========================================================================

/// Evidence double that also answers the new `open_mergeable_pr` signal.
struct AwaitMergeEvidence {
    pr_merged: bool,
    issue_closed: bool,
    deployed: bool,
    open_mergeable: bool,
}

impl EvidenceSource for AwaitMergeEvidence {
    fn any_pr_merged(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(self.pr_merged)
    }
    fn issue_closed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(self.issue_closed)
    }
    fn is_deployed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(self.deployed)
    }
    fn open_mergeable_pr(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        Ok(self.open_mergeable)
    }
}

#[test]
fn verify_maps_open_mergeable_pr_goal_to_awaiting_merge() {
    // Gate says Blocked (PR not merged), the goal is not obsolete, and the PR is
    // open+non-draft+mergeable → AwaitingMerge (not Unresolved).
    let gate = CompletionEvidenceGate::new(AwaitMergeEvidence {
        pr_merged: false,
        issue_closed: true,
        deployed: false,
        open_mergeable: true,
    });
    let mut goal = stuck_goal("g");
    goal.wip_refs = vec![pr_ref("4440")];
    assert_eq!(
        verify_stuck_goal(&goal, &gate),
        StuckGoalDisposition::AwaitingMerge,
        "a completed-with-open-mergeable-PR goal must be AwaitingMerge"
    );
}

#[test]
fn verify_without_open_mergeable_pr_stays_unresolved() {
    // Same blocked verdict but the PR is NOT open+mergeable → the existing
    // reap/escalate semantics are preserved (Unresolved).
    let gate = CompletionEvidenceGate::new(AwaitMergeEvidence {
        pr_merged: false,
        issue_closed: true,
        deployed: false,
        open_mergeable: false,
    });
    let mut goal = stuck_goal("g");
    goal.wip_refs = vec![pr_ref("7")];
    assert_eq!(
        verify_stuck_goal(&goal, &gate),
        StuckGoalDisposition::Unresolved,
        "a draft/dirty/closed PR must NOT be treated as awaiting-merge"
    );
}

#[test]
fn verify_merged_pr_is_done_and_wins_over_awaiting_merge() {
    // A MERGED PR (with issue closed + deployed) is certified Complete → Done
    // BEFORE the awaiting-merge branch is even consulted.
    let gate = CompletionEvidenceGate::new(AwaitMergeEvidence {
        pr_merged: true,
        issue_closed: true,
        deployed: true,
        open_mergeable: true,
    });
    let mut goal = stuck_goal("g");
    goal.wip_refs = vec![pr_ref("4440")];
    assert_eq!(
        verify_stuck_goal(&goal, &gate),
        StuckGoalDisposition::Done,
        "a merged PR must resolve as Done, never AwaitingMerge"
    );
}

#[test]
fn verify_obsolete_wins_over_awaiting_merge() {
    // An out-of-scope goal is dropped even if it has an open mergeable PR:
    // obsolescence is checked before the awaiting-merge branch.
    let gate = CompletionEvidenceGate::new(AwaitMergeEvidence {
        pr_merged: false,
        issue_closed: false,
        deployed: false,
        open_mergeable: true,
    });
    let mut goal = stuck_goal("g");
    goal.wip_refs = vec![
        pr_ref("4440"),
        issue_ref("42", "out-of-scope; tracked elsewhere"),
    ];
    match verify_stuck_goal(&goal, &gate) {
        StuckGoalDisposition::Obsolete { .. } => {}
        other => panic!("expected Obsolete to win over AwaitingMerge, got {other:?}"),
    }
}

#[test]
fn resolve_maps_awaiting_merge_disposition_to_await_merge() {
    let res = resolve_no_progress(
        "g",
        NO_PROGRESS_BREAKER_THRESHOLD,
        NO_PROGRESS_BREAKER_THRESHOLD,
        || StuckGoalDisposition::AwaitingMerge,
    );
    assert_eq!(res, NoProgressResolution::AwaitMerge);
}

#[test]
fn resolve_below_threshold_never_consults_awaiting_merge() {
    // Below threshold the disposition closure is never evaluated.
    let res = resolve_no_progress("g", 1, NO_PROGRESS_BREAKER_THRESHOLD, || {
        panic!("disposition must not be evaluated below threshold");
    });
    assert_eq!(res, NoProgressResolution::Continue);
}

#[test]
fn await_merge_resolution_is_not_terminal() {
    // Non-terminal, alongside Continue and SurfaceInvestigationFailure: the goal
    // stays tracked and its counter is preserved.
    assert!(
        !NoProgressResolution::AwaitMerge.is_terminal(),
        "AwaitMerge must be non-terminal so a degraded PR can fall back instantly"
    );
}

#[test]
fn record_and_resolve_await_merge_preserves_counter_and_falls_back_on_degradation() {
    // While the PR is open+mergeable the breaker idles the goal every cycle
    // WITHOUT clearing its no-action counter. The instant the PR degrades the
    // very next cycle escalates — proving the counter was never reset (a reset
    // would need another full threshold run before firing).
    let threshold = NO_PROGRESS_BREAKER_THRESHOLD;
    let mut tracker = NoProgressTracker::new();

    // Build up to the threshold (sub-threshold cycles stay `Continue` — the
    // disposition closure is evaluated lazily), then idle as awaiting-merge for
    // two extra cycles once the breaker fires. Across the awaiting-merge idles
    // the counter is NOT reset (AwaitMerge is non-terminal).
    for cycle in 1..=(threshold + 2) {
        let res =
            tracker.record_and_resolve("g", threshold, || StuckGoalDisposition::AwaitingMerge);
        if cycle < threshold {
            assert_eq!(
                res,
                NoProgressResolution::Continue,
                "cycle {cycle} is sub-threshold"
            );
        } else {
            assert_eq!(
                res,
                NoProgressResolution::AwaitMerge,
                "cycle {cycle} idles awaiting-merge"
            );
        }
    }
    assert!(
        tracker.consecutive("g") >= threshold,
        "the counter must be preserved across awaiting-merge idles, got {}",
        tracker.consecutive("g")
    );

    // PR degrades → the next single cycle escalates (terminal), clearing the
    // counter.
    let res = tracker.record_and_resolve("g", threshold, || StuckGoalDisposition::Unresolved);
    assert!(
        matches!(res, NoProgressResolution::Escalate { .. }),
        "a degraded PR must fall back to Escalate on the next cycle, got {res:?}"
    );
    assert_eq!(
        tracker.consecutive("g"),
        0,
        "the terminal escalation must clear the counter"
    );
}
