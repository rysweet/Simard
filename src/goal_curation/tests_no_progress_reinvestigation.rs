//! TEST-FIRST (Step 7 TDD) for the **pure** primitives of the already-blocked
//! re-investigation pass (issue #17):
//!
//! * the thin deterministic rail [`is_bare_no_progress_block`] that gates the
//!   agentic classification — it must recognise a *bare* safeguard block (marker
//!   present, no WHY class token) and never mistake a WHY-bearing or
//!   other-kind block for one; and
//! * the [`NoProgressTracker`] persisted `reinvestigated` dedupe set that bounds
//!   the re-investigation to **one terminal action per `(goal, class)`** across a
//!   daemon restart — with its lifecycle hooks (`record_progress` clears,
//!   `retain_goals` prunes) and its serde/fail-to-empty on-disk contract.
//!
//! The side-effecting population-driven pass is specified in
//! `crate::ooda_loop::tests_no_progress_reinvestigation`.
//!
//! RED until the rail, the token vocabulary, and the tracker dedupe set exist.

use super::no_progress_breaker::{
    NO_PROGRESS_BREAKER_THRESHOLD, NoProgressTracker, is_bare_no_progress_block,
    is_no_progress_marker, no_progress_blocked_reason, no_progress_blocked_reason_with_why,
};
use super::no_progress_why::{Evidence, NoProgressClass, NoProgressWhy};

// === the thin deterministic rail: is_bare_no_progress_block =================

#[test]
fn a_bare_safeguard_reason_reads_as_bare() {
    // The exact production string the daemon parked goals with: a no-progress
    // marker that carries NO WHY classification.
    let bare = no_progress_blocked_reason(NO_PROGRESS_BREAKER_THRESHOLD);
    assert!(
        is_no_progress_marker(&bare),
        "sanity: the bare reason is a no-progress marker"
    );
    assert!(
        is_bare_no_progress_block(&bare),
        "a bare '[OODA-SAFEGUARD] … needs human review' reason MUST read as bare: {bare}"
    );
}

#[test]
fn a_why_bearing_reason_is_never_bare_for_any_class() {
    // For EVERY class the WHY-bearing renderer must produce a reason that is a
    // no-progress marker (so self-heal/overseer still recognise it) yet is NOT
    // bare (so the re-investigation pass never re-processes it) — this is the
    // primary idempotency guarantee.
    for class in NoProgressClass::ALL {
        let why = NoProgressWhy::new(class, vec![Evidence::new("issue", "#16", "OPEN")]);
        let reason = no_progress_blocked_reason_with_why(NO_PROGRESS_BREAKER_THRESHOLD, &why);
        assert!(
            is_no_progress_marker(&reason),
            "{class:?}: a WHY-bearing reason must still be a no-progress marker: {reason}"
        );
        assert!(
            reason.contains(class.token()),
            "{class:?}: a WHY-bearing reason must embed the class token: {reason}"
        );
        assert!(
            !is_bare_no_progress_block(&reason),
            "{class:?}: a WHY-bearing reason must NOT read as bare: {reason}"
        );
    }
}

#[test]
fn non_marker_blocks_are_never_bare_no_progress_blocks() {
    // The rail must never mistake another kind of block (operator-set, scope,
    // dependency, brain-failure, arbitrary text) for a bare no-progress block —
    // it keys strictly on the no-progress marker sentinel, not on any generic
    // "blocked" wording.
    let not_bare = [
        "",
        "blocked by operator: waiting on design review",
        "scope-blocked: out of milestone",
        "dependency: waiting on upstream goal 'foo'",
        "\u{1F512} [BRAIN-FAILURE] brain failed 3 consecutive cycles; needs human review",
        "needs human review",
        "OODA goal made no shippable progress", // prefix fragment only, not the full marker
    ];
    for reason in not_bare {
        assert!(
            !is_bare_no_progress_block(reason),
            "a non-no-progress-marker string must NOT read as a bare no-progress block: {reason:?}"
        );
    }
}

#[test]
fn a_bare_reason_with_an_arbitrary_higher_count_is_still_bare() {
    // The rail must not depend on the specific consecutive count embedded in the
    // marker — any bare marker (regardless of count) is bare.
    for n in [3_u32, 7, 42] {
        let bare = no_progress_blocked_reason(n);
        assert!(
            is_bare_no_progress_block(&bare),
            "count={n}: a bare marker is bare regardless of its count: {bare}"
        );
    }
}

// === NoProgressTracker persisted dedupe set =================================

#[test]
fn marking_reinvestigated_is_recorded_per_goal_and_class() {
    let mut tracker = NoProgressTracker::new();
    assert!(
        !tracker.reinvestigated("g1", NoProgressClass::GenuinelyStuck),
        "an untouched goal must not read as already re-investigated"
    );

    tracker.mark_reinvestigated("g1", NoProgressClass::GenuinelyStuck);
    assert!(
        tracker.reinvestigated("g1", NoProgressClass::GenuinelyStuck),
        "after marking, the (goal, class) pair must read as re-investigated"
    );
    // Dedupe is keyed on BOTH goal and class: a different class for the same goal,
    // and the same class for a different goal, are independent.
    assert!(
        !tracker.reinvestigated("g1", NoProgressClass::UpstreamDependency),
        "a different class for the same goal must be independent"
    );
    assert!(
        !tracker.reinvestigated("g2", NoProgressClass::GenuinelyStuck),
        "the same class for a different goal must be independent"
    );
}

#[test]
fn record_progress_clears_the_reinvestigated_flag() {
    // Real forward progress must earn a FRESH future re-investigation — symmetric
    // with how `record_progress` clears the counter and the guided-retry flag.
    let mut tracker = NoProgressTracker::new();
    tracker.mark_reinvestigated("g1", NoProgressClass::GenuinelyStuck);
    assert!(tracker.reinvestigated("g1", NoProgressClass::GenuinelyStuck));

    tracker.record_progress("g1");
    assert!(
        !tracker.reinvestigated("g1", NoProgressClass::GenuinelyStuck),
        "forward progress must clear the goal's re-investigation dedupe entry"
    );
}

#[test]
fn retain_goals_prunes_reinvestigated_entries_for_departed_goals() {
    // The dedupe set must not leak ids for goals that left the board (cascade
    // delete), mirroring the counter/guided-retry pruning.
    let mut tracker = NoProgressTracker::new();
    tracker.mark_reinvestigated("still-here", NoProgressClass::UpstreamDependency);
    tracker.mark_reinvestigated("departed", NoProgressClass::GenuinelyStuck);

    let live: std::collections::HashSet<String> = ["still-here".to_string()].into_iter().collect();
    tracker.retain_goals(&live);

    assert!(
        tracker.reinvestigated("still-here", NoProgressClass::UpstreamDependency),
        "a live goal's dedupe entry must be retained"
    );
    assert!(
        !tracker.reinvestigated("departed", NoProgressClass::GenuinelyStuck),
        "a departed goal's dedupe entry must be pruned"
    );
}

#[test]
fn reinvestigated_set_round_trips_through_serde() {
    // The dedupe set must survive a daemon restart (it persists alongside the
    // no-action counter in `state/goal_board.json`), so idempotency holds across
    // a crash between the board rewrite and the tracker persist.
    let mut tracker = NoProgressTracker::new();
    tracker.mark_reinvestigated("g1", NoProgressClass::UpstreamDependency);
    tracker.mark_reinvestigated("g2", NoProgressClass::AlreadyComplete);

    let json = serde_json::to_string(&tracker).expect("tracker serialises");
    let restored: NoProgressTracker = serde_json::from_str(&json).expect("tracker deserialises");

    assert!(
        restored.reinvestigated("g1", NoProgressClass::UpstreamDependency),
        "the dedupe entry must survive a serde round-trip (restart)"
    );
    assert!(
        restored.reinvestigated("g2", NoProgressClass::AlreadyComplete),
        "the dedupe entry must survive a serde round-trip (restart)"
    );
    assert!(
        !restored.reinvestigated("g1", NoProgressClass::AlreadyComplete),
        "the class distinction must survive the round-trip"
    );
}

#[test]
fn reinvestigated_set_persists_class_as_the_stable_token_string() {
    // Fail-to-empty (C1) safety: the on-disk form must store the class as its
    // stable TOKEN string, never an enum-tagged representation that an older /
    // rolled-back binary (or a future 7th variant) could fail to parse and thereby
    // wipe the whole board. Assert the serialized JSON carries the token verbatim.
    let mut tracker = NoProgressTracker::new();
    tracker.mark_reinvestigated("g1", NoProgressClass::UpstreamDependency);

    let json = serde_json::to_string(&tracker).expect("tracker serialises");
    assert!(
        json.contains(NoProgressClass::UpstreamDependency.token()),
        "the dedupe set must persist the stable class token string, got: {json}"
    );
}

#[test]
fn a_pre_issue17_snapshot_deserialises_with_an_empty_reinvestigated_set() {
    // Backward compatibility: a `state/goal_board.json` written by a daemon build
    // that predates issue #17 has no `reinvestigated` field. `#[serde(default)]`
    // must load it with an empty set — never a deserialize error (which the
    // fail-to-empty store would turn into a full board wipe).
    let pre_17 = r#"{"counts":{"g1":2},"guided_retries":["g1"]}"#;
    let tracker: NoProgressTracker =
        serde_json::from_str(pre_17).expect("a pre-#17 snapshot must still deserialise");

    assert_eq!(
        tracker.consecutive("g1"),
        2,
        "the existing counter must load unchanged"
    );
    assert!(
        tracker.guided_retry_used("g1"),
        "the existing guided-retry flag must load unchanged"
    );
    assert!(
        !tracker.reinvestigated("g1", NoProgressClass::GenuinelyStuck),
        "a pre-#17 snapshot must load with an EMPTY re-investigation dedupe set"
    );
}
