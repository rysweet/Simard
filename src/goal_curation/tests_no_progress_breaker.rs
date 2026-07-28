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

/// # TDD (Step 7) — goal-key backstop: pure `fold_goal_identity` contract
///
/// FAILING BY DESIGN until `fold_goal_identity` (and its `pub(crate)`
/// visibility) exists in `super::no_progress_breaker`, and the redaction
/// helpers in `crate::stewardship::dedup` are promoted to `pub(crate)`.
///
/// Specifies the pure, injection-safe identity-folding helper documented in
/// `docs/reference/no-progress-breaker-goal-key-backstop-api.md` — the stable
/// key that lets the open-issue backstop dedup across goal-id churn / board
/// reset without ever letting a churny, attacker-influenced goal id leak into a
/// `gh --search` query (SR1).
#[cfg(test)]
mod tests_goal_key_backstop {
    use super::super::no_progress_breaker::fold_goal_identity;

    /// A goal id carrying every character class that would corrupt a `gh
    /// --search` query if interpolated raw: whitespace, quotes, and GitHub
    /// search qualifiers (`is:`, `label:`, `in:body`).
    const ADVERSARIAL_ID: &str =
        "simard-identity-\"drop\" is:open label:ooda-stuck in:body a8f57a50 --json";

    fn is_lower_hex_16(s: &str) -> bool {
        s.len() == 16
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    /// Case 4 (charset): the folded key is ALWAYS exactly 16 lowercase hex
    /// chars, even for an adversarial id — so it can be interpolated into a
    /// `--search` argument without carrying spaces / quotes / qualifiers.
    #[test]
    fn folded_key_is_pure_16_lower_hex_even_for_adversarial_id() {
        assert!(
            is_lower_hex_16(&fold_goal_identity(ADVERSARIAL_ID)),
            "an adversarial goal id must fold to a pure [0-9a-f]{{16}} token (SR1)"
        );
        assert!(is_lower_hex_16(&fold_goal_identity("g")));
        assert!(is_lower_hex_16(&fold_goal_identity("")));
        assert!(is_lower_hex_16(&fold_goal_identity(
            "simard-identity-atelier-industrial-furniture-de"
        )));
    }

    /// Determinism: the same id always folds to the same key (this is what lets
    /// two OODA cycles on the same goal dedup to one open issue).
    #[test]
    fn fold_is_deterministic() {
        assert_eq!(
            fold_goal_identity(ADVERSARIAL_ID),
            fold_goal_identity(ADVERSARIAL_ID),
        );
    }

    /// Distinct ids fold to distinct keys (no accidental over-collapse that
    /// would suppress a genuinely different stuck goal).
    #[test]
    fn distinct_ids_fold_to_distinct_keys() {
        assert_ne!(fold_goal_identity("goal-a"), fold_goal_identity("goal-b"));
    }

    /// SR1/SR3: the raw id (and any secret-shaped substring inside it) must NOT
    /// survive into the folded key — it is a one-way hash, not an encoding.
    #[test]
    fn raw_id_never_leaks_into_the_folded_key() {
        let folded = fold_goal_identity(ADVERSARIAL_ID);
        for needle in ["is:", "label:", "in:body", "\"", " ", "--json", "a8f57a50"] {
            assert!(
                !folded.contains(needle),
                "folded key must not echo id fragment {needle:?}"
            );
        }
    }

    /// Implementation prerequisite #1: the stewardship redaction helpers must be
    /// promoted from module-private `fn` to `pub(crate) fn` so the breaker (in
    /// `goal_curation`) can redact goal-derived free-text before embedding it in
    /// an escalation body (SR3/SR6). This test only COMPILES once that
    /// visibility widening lands.
    #[test]
    fn redaction_helpers_are_reachable_from_goal_curation() {
        let uuid = "0191b2c3-4d5e-7f80-9abc-def012345678";
        assert_eq!(crate::stewardship::dedup::redact_token(uuid), "<UUID>");
        assert_eq!(
            crate::stewardship::dedup::redact_uuids(&format!("session={uuid}")),
            "session=<UUID>",
        );
    }
}
