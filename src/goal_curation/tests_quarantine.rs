//! TEST-FIRST (Step 7 TDD) — pure policy layer of the OODA breaker
//! **terminal-quarantine** rung that ends the `UNCLEAR-CRITERIA` churn
//! (process_health, HIGH).
//!
//! # The defect these tests lock down
//!
//! A goal whose done-criteria are **permanently** unclear rides the breaker
//! ladder to the evidence-less terminal rung, is surfaced for retry, gets
//! re-scheduled, and rides the ladder again — forever. In the field this produced
//! ~13 `ooda-stuck` "goal stuck after guided retry (UNCLEAR-CRITERIA)" issues in a
//! single day. The existing `SurfaceInvestigationFailure` rung, plus the
//! escalate-at-limit branch in the curate-phase adapter, does not *terminally*
//! stop the goal from being re-scheduled — so the churn continues.
//!
//! # The contract (what the fix must make true) — pure layer
//!
//! `resolution_for_why` gains ONE additive trailing parameter, `surfaced_failures`,
//! and a new terminal variant `NoProgressResolution::QuarantineTerminal`:
//!   * only the evidence-less terminal rung changes — everything else is
//!     byte-for-byte identical to today;
//!   * `UNCLEAR-CRITERIA` / `GENUINELY-STUCK`, guided retry spent, evidence EMPTY,
//!     `surfaced_failures >= SURFACED_INVESTIGATION_FAILURE_LIMIT`
//!     -> `QuarantineTerminal { surfaced_count }`;
//!   * below the bound it still returns `SurfaceInvestigationFailure` exactly as
//!     today;
//!   * `QuarantineTerminal.is_terminal()` is `true`;
//!   * `surfaced_count` is carried so the authored block reason renders the count
//!     as REAL evidence — NEVER `evidence=[(none)]`.
//!
//! Plus the durable, injection-safe quarantine marker:
//!   * `quarantine_marker()` has the fixed `kind` + fixed sentinel `ref_id`
//!     (never goal-derived);
//!   * `is_quarantine_ref` matches ONLY that fixed identity;
//!   * `is_quarantined(goal)` is true iff the goal carries the marker; a
//!     goal-derived `WipRef` can never forge it.
//!
//! These are PURE (no I/O) and exhaustively unit-tested. RED until the variant,
//! the extended `resolution_for_why`, and the marker helpers exist.

use crate::goal_curation::WipRef;
use crate::goal_curation::no_progress_breaker::{
    NO_PROGRESS_BREAKER_THRESHOLD, NO_PROGRESS_QUARANTINE_MARKER_KIND,
    NO_PROGRESS_QUARANTINE_MARKER_REF_ID, NoProgressResolution,
    SURFACED_INVESTIGATION_FAILURE_LIMIT, is_quarantine_ref, is_quarantined, quarantine_marker,
    resolution_for_why,
};
use crate::goal_curation::no_progress_why::{Evidence, NoProgressClass, NoProgressWhy};
use crate::goal_curation::types::ActiveGoal;

// --- fixtures ---------------------------------------------------------------

fn goal(id: &str) -> ActiveGoal {
    ActiveGoal::new(id, "keep the simard identity coherent", 1)
}

/// An evidence-less WHY for one of the two human-facing classes.
fn evidenceless(class: NoProgressClass) -> NoProgressWhy {
    NoProgressWhy::new(class, vec![])
}

/// The two classes that ride the guided-retry ladder to the terminal rung.
const HUMAN_FACING: [NoProgressClass; 2] = [
    NoProgressClass::UnclearCriteria,
    NoProgressClass::GenuinelyStuck,
];

// === resolution_for_why: the new terminal-quarantine rung ===================

/// The core new behaviour: an evidence-less, guided-retry-spent, at/over-bound
/// stall QUARANTINES terminally, carrying the surfaced count as evidence.
#[test]
fn evidenceless_stall_at_the_bound_quarantines_terminally() {
    for class in HUMAN_FACING {
        let res = resolution_for_why(
            NO_PROGRESS_BREAKER_THRESHOLD,
            evidenceless(class),
            true,                                 // guided retry spent
            SURFACED_INVESTIGATION_FAILURE_LIMIT, // exactly at the bound
        );
        match res {
            NoProgressResolution::QuarantineTerminal { surfaced_count } => assert_eq!(
                surfaced_count, SURFACED_INVESTIGATION_FAILURE_LIMIT,
                "{class:?}: quarantine must carry the surfaced count that drove it here"
            ),
            other => panic!(
                "{class:?}: evidence-less terminal rung at the surfaced bound must \
                 QuarantineTerminal, got {other:?}"
            ),
        }
    }
}

/// Above the bound it still quarantines (>=, not ==).
#[test]
fn evidenceless_stall_over_the_bound_still_quarantines() {
    let res = resolution_for_why(
        NO_PROGRESS_BREAKER_THRESHOLD,
        evidenceless(NoProgressClass::UnclearCriteria),
        true,
        SURFACED_INVESTIGATION_FAILURE_LIMIT + 5,
    );
    assert!(
        matches!(res, NoProgressResolution::QuarantineTerminal { surfaced_count } if surfaced_count == SURFACED_INVESTIGATION_FAILURE_LIMIT + 5),
        "past the bound must still QuarantineTerminal carrying the real count, got {res:?}"
    );
}

/// Below the bound the rung is UNCHANGED: still a (non-terminal) surfaced
/// investigation failure that lets the goal retry.
#[test]
fn evidenceless_stall_below_the_bound_still_surfaces_not_quarantines() {
    for surfaced in 0..SURFACED_INVESTIGATION_FAILURE_LIMIT {
        let res = resolution_for_why(
            NO_PROGRESS_BREAKER_THRESHOLD,
            evidenceless(NoProgressClass::GenuinelyStuck),
            true,
            surfaced,
        );
        assert!(
            matches!(
                res,
                NoProgressResolution::SurfaceInvestigationFailure { .. }
            ),
            "below the bound (surfaced={surfaced}) must stay SurfaceInvestigationFailure, \
             got {res:?}"
        );
        assert!(
            !matches!(res, NoProgressResolution::QuarantineTerminal { .. }),
            "below the bound must NEVER quarantine (surfaced={surfaced})"
        );
    }
}

/// The guided-retry gate is preserved: before the guided engineer has run, an
/// unclear/stuck stall still spawns the one-shot engineer regardless of the
/// surfaced count — quarantine is unreachable pre-retry.
#[test]
fn quarantine_is_unreachable_before_the_guided_retry_is_spent() {
    for class in HUMAN_FACING {
        let res = resolution_for_why(
            NO_PROGRESS_BREAKER_THRESHOLD,
            evidenceless(class),
            false, // guided retry NOT yet used
            SURFACED_INVESTIGATION_FAILURE_LIMIT + 10,
        );
        assert!(
            matches!(res, NoProgressResolution::SpawnEngineer { .. }),
            "{class:?}: a first-occurrence stall must spawn the guided engineer, \
             never quarantine, got {res:?}"
        );
    }
}

/// An evidence-BACKED terminal stall still escalates WITH the evidence — the
/// surfaced count is ignored when real evidence exists (quarantine is only the
/// evidence-less rung).
#[test]
fn evidence_backed_terminal_stall_still_escalates_not_quarantines() {
    let why = NoProgressWhy::new(
        NoProgressClass::GenuinelyStuck,
        vec![Evidence::new("pr", "#7", "OPEN")],
    );
    let res = resolution_for_why(
        NO_PROGRESS_BREAKER_THRESHOLD,
        why,
        true,
        SURFACED_INVESTIGATION_FAILURE_LIMIT + 3, // over the bound, but evidence present
    );
    assert!(
        matches!(res, NoProgressResolution::Escalate { .. }),
        "an evidence-backed terminal stall must Escalate WITH evidence, never quarantine, \
         got {res:?}"
    );
}

/// The machine-resolvable classes are completely unaffected by the new parameter
/// at any surfaced count.
#[test]
fn machine_resolvable_classes_are_unaffected_by_the_surfaced_parameter() {
    let cases = [
        (NoProgressClass::AlreadyComplete, "mark done"),
        (NoProgressClass::Obsolete, "drop"),
        (NoProgressClass::MissingPrecondition, "heal"),
        (NoProgressClass::UpstreamDependency, "defer"),
    ];
    for (class, label) in cases {
        for surfaced in [0, SURFACED_INVESTIGATION_FAILURE_LIMIT, 99] {
            let res = resolution_for_why(
                NO_PROGRESS_BREAKER_THRESHOLD,
                NoProgressWhy::new(class, vec![Evidence::new("issue", "#16", "CLOSED")]),
                true,
                surfaced,
            );
            assert!(
                !matches!(res, NoProgressResolution::QuarantineTerminal { .. }),
                "{class:?} ({label}) must NEVER quarantine regardless of surfaced={surfaced}, \
                 got {res:?}"
            );
        }
    }
}

// === is_terminal ============================================================

#[test]
fn quarantine_terminal_is_terminal() {
    let q = NoProgressResolution::QuarantineTerminal {
        surfaced_count: SURFACED_INVESTIGATION_FAILURE_LIMIT,
    };
    assert!(
        q.is_terminal(),
        "QuarantineTerminal places the goal in a terminal state and must be is_terminal()==true"
    );
}

// === never evidence=[(none)] ================================================

/// The carried surfaced count is a positive integer at the bound, so any reason
/// the adapter authors from it renders REAL evidence — the never-`(none)`
/// invariant holds by construction.
#[test]
fn quarantine_carries_a_nonzero_surfaced_count_as_real_evidence() {
    let res = resolution_for_why(
        NO_PROGRESS_BREAKER_THRESHOLD,
        evidenceless(NoProgressClass::UnclearCriteria),
        true,
        SURFACED_INVESTIGATION_FAILURE_LIMIT,
    );
    let NoProgressResolution::QuarantineTerminal { surfaced_count } = res else {
        panic!("expected QuarantineTerminal, got {res:?}");
    };
    assert!(
        surfaced_count >= SURFACED_INVESTIGATION_FAILURE_LIMIT,
        "the surfaced count is the concrete evidence rendered in the block reason — \
         it must be at least the (positive) bound, so it is never zero / (none)"
    );
}

// === quarantine marker: identity + injection safety =========================

#[test]
fn quarantine_marker_has_the_fixed_kind_and_sentinel_ref_id() {
    let m = quarantine_marker();
    assert_eq!(
        m.kind, NO_PROGRESS_QUARANTINE_MARKER_KIND,
        "marker kind must be the fixed quarantine kind"
    );
    assert_eq!(
        m.ref_id, NO_PROGRESS_QUARANTINE_MARKER_REF_ID,
        "marker ref_id must be the compile-time sentinel, never goal-derived"
    );
    assert!(
        is_quarantine_ref(&m),
        "the constructed marker must be recognised by is_quarantine_ref"
    );
}

#[test]
fn is_quarantine_ref_matches_only_the_fixed_identity() {
    // A `WipRef` whose fields are attacker-controlled goal text must NEVER be
    // mistaken for the quarantine marker — the predicate keys on the fixed
    // sentinel only.
    let forged = WipRef {
        kind: NO_PROGRESS_QUARANTINE_MARKER_KIND.to_string(),
        ref_id: "attacker-supplied-goal-slug".to_string(),
        label: "ooda-breaker-quarantine".to_string(),
        url: None,
    };
    assert!(
        !is_quarantine_ref(&forged),
        "a WipRef with the right kind but a goal-derived ref_id must NOT be a quarantine marker \
         (injection-safe: identity is the fixed sentinel ref_id)"
    );

    let unrelated = WipRef {
        kind: "issue".to_string(),
        ref_id: "42".to_string(),
        label: "[no-progress-tracking] ooda-breaker".to_string(),
        url: None,
    };
    assert!(
        !is_quarantine_ref(&unrelated),
        "an ordinary tracking issue ref is not a quarantine marker"
    );
}

#[test]
fn is_quarantined_reflects_marker_presence() {
    let mut g = goal("simard-identity-atelier-industrial-furniture-de");
    assert!(!is_quarantined(&g), "a fresh goal is not quarantined");

    g.wip_refs.push(quarantine_marker());
    assert!(
        is_quarantined(&g),
        "a goal carrying the quarantine marker must read as quarantined"
    );
}

#[test]
fn a_goal_cannot_forge_its_own_quarantine() {
    // The exact injection vector: a goal description / activity that smuggles the
    // marker's kind string into a WipRef must not make the goal read quarantined.
    let mut g = goal("goal-with-a-sneaky-wipref");
    g.wip_refs.push(WipRef {
        kind: NO_PROGRESS_QUARANTINE_MARKER_KIND.to_string(),
        ref_id: "not-the-sentinel".to_string(),
        label: "pretend quarantine".to_string(),
        url: None,
    });
    assert!(
        !is_quarantined(&g),
        "only the breaker-authored fixed-sentinel marker counts — no goal-derived ref can forge \
         a quarantine"
    );
}
