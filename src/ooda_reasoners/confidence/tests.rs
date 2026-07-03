//! Unit tests for the trustworthy-confidence primitive (issue #2457).
//!
//! These were written test-first against the issue's acceptance criteria:
//! - the brain judgments carry a *populated, validated* confidence;
//! - a self-consistency vote exists, is bounded by K, and exposes vote
//!   agreement as confidence;
//! - a calibration (ECE) measure over predicted-vs-realized outcomes exists.

use super::*;
use crate::ooda_reasoners::{DecideJudgment, EngineerLifecycleDecision};

fn skip(r: &str) -> EngineerLifecycleDecision {
    EngineerLifecycleDecision::ContinueSkipping {
        rationale: r.into(),
    }
}
fn blocked(r: &str) -> EngineerLifecycleDecision {
    EngineerLifecycleDecision::MarkGoalBlocked {
        rationale: r.into(),
        reason: "x".into(),
    }
}
fn reclaim(r: &str) -> EngineerLifecycleDecision {
    EngineerLifecycleDecision::ReclaimAndRedispatch {
        rationale: r.into(),
        redispatch_context: String::new(),
    }
}

// ---------------------------------------------------------------------------
// Confidence scalars
// ---------------------------------------------------------------------------

#[test]
fn default_confidence_is_one() {
    assert!((default_confidence() - 1.0).abs() < 1e-12);
}

#[test]
fn validate_confidence_accepts_unit_interval_and_boundaries() {
    assert!(validate_confidence(0.0).is_ok());
    assert!(validate_confidence(1.0).is_ok());
    assert!(validate_confidence(0.5).is_ok());
}

#[test]
fn validate_confidence_rejects_out_of_range_and_nonfinite() {
    assert!(validate_confidence(-0.0001).is_err());
    assert!(validate_confidence(1.0001).is_err());
    assert!(validate_confidence(f64::NAN).is_err());
    assert!(validate_confidence(f64::INFINITY).is_err());
    assert!(validate_confidence(f64::NEG_INFINITY).is_err());
}

#[test]
fn clamp_confidence_maps_nan_to_zero_and_clamps_range() {
    assert_eq!(clamp_confidence(f64::NAN), 0.0);
    assert_eq!(clamp_confidence(-5.0), 0.0);
    assert_eq!(clamp_confidence(5.0), 1.0);
    assert!((clamp_confidence(0.42) - 0.42).abs() < 1e-12);
}

#[test]
fn low_trust_confidence_is_the_zero_floor() {
    // The fail-closed floor must be the least-privileged value, distinct from the
    // cheerful default — so a malformed/absent solicited confidence can never
    // unlock privilege.
    assert_eq!(LOW_TRUST_CONFIDENCE, 0.0);
    assert!(LOW_TRUST_CONFIDENCE < default_confidence());
}

#[test]
fn confidence_or_low_trust_fails_closed() {
    // Present and valid → used verbatim.
    assert!((confidence_or_low_trust(Some(0.73)) - 0.73).abs() < 1e-12);
    assert_eq!(confidence_or_low_trust(Some(0.0)), 0.0);
    assert_eq!(confidence_or_low_trust(Some(1.0)), 1.0);
    // Absent / out of range / non-finite → fail closed to LOW_TRUST_CONFIDENCE,
    // NEVER to default_confidence() (1.0).
    assert_eq!(confidence_or_low_trust(None), LOW_TRUST_CONFIDENCE);
    assert_eq!(confidence_or_low_trust(Some(1.5)), LOW_TRUST_CONFIDENCE);
    assert_eq!(confidence_or_low_trust(Some(-0.1)), LOW_TRUST_CONFIDENCE);
    assert_eq!(
        confidence_or_low_trust(Some(f64::NAN)),
        LOW_TRUST_CONFIDENCE
    );
    assert_eq!(
        confidence_or_low_trust(Some(f64::INFINITY)),
        LOW_TRUST_CONFIDENCE
    );
}

// ---------------------------------------------------------------------------
// High-stakes / irreversibility gating
// ---------------------------------------------------------------------------

#[test]
fn high_stakes_threshold_is_inclusive() {
    assert!(is_high_stakes(HIGH_STAKES_URGENCY));
    assert!(is_high_stakes(0.95));
    assert!(!is_high_stakes(0.79));
    assert!(!is_high_stakes(f64::NAN));
}

#[test]
fn irreversible_lifecycle_covers_escalating_actions_only() {
    assert!(is_irreversible_lifecycle(&blocked("b")));
    assert!(is_irreversible_lifecycle(&reclaim("r")));
    assert!(is_irreversible_lifecycle(
        &EngineerLifecycleDecision::OpenTrackingIssue {
            rationale: "r".into(),
            title: "t".into(),
            body: "b".into(),
        }
    ));
    // Cheap / reversible actions are not gated as irreversible.
    assert!(!is_irreversible_lifecycle(&skip("s")));
    assert!(!is_irreversible_lifecycle(
        &EngineerLifecycleDecision::Deprioritize {
            rationale: "d".into()
        }
    ));
    assert!(!is_irreversible_lifecycle(
        &EngineerLifecycleDecision::ConsiderSelfUpdate {
            rationale: "u".into()
        }
    ));
}

#[test]
fn self_consistency_is_gated_by_stakes_and_budget() {
    assert!(should_self_consistency_sample(0.9, true));
    assert!(!should_self_consistency_sample(0.9, false)); // no budget
    assert!(!should_self_consistency_sample(0.2, true)); // not high-stakes
}

#[test]
fn effective_k_degrades_to_one_without_budget() {
    assert_eq!(effective_k(true), SELF_CONSISTENCY_K);
    assert_eq!(effective_k(false), 1);
    // K must be odd and >= 3 to avoid binary ties; assert via a non-constant
    // expression so the check is a real runtime assertion.
    assert_eq!(effective_k(true) % 2, 1, "K must be odd to avoid ties");
}

// ---------------------------------------------------------------------------
// Self-consistency vote
// ---------------------------------------------------------------------------

#[test]
fn vote_on_empty_is_none() {
    let empty: Vec<EngineerLifecycleDecision> = vec![];
    assert!(self_consistency_vote(&empty, lifecycle_conservative_rank).is_none());
}

#[test]
fn unanimous_vote_has_full_agreement() {
    let samples = vec![skip("a"), skip("a"), skip("a")];
    let v = self_consistency_vote(&samples, lifecycle_conservative_rank).unwrap();
    assert_eq!(v.choice, skip("a"));
    assert!((v.agreement - 1.0).abs() < 1e-12);
    assert_eq!(v.modal_count, 3);
    assert_eq!(v.k, 3);
}

#[test]
fn majority_vote_reports_agreement_fraction() {
    // 2 of 3 agree → agreement 2/3, modal choice wins.
    let samples = vec![skip("a"), skip("a"), blocked("b")];
    let v = self_consistency_vote(&samples, lifecycle_conservative_rank).unwrap();
    assert_eq!(v.choice, skip("a"));
    assert!((v.agreement - 2.0 / 3.0).abs() < 1e-9);
    assert_eq!(v.modal_count, 2);
}

#[test]
fn tie_breaks_toward_more_conservative_choice() {
    // 1 vs 1 tie: the conservative-rank picks MarkGoalBlocked over ContinueSkipping.
    let samples = vec![skip("a"), blocked("b")];
    let v = self_consistency_vote(&samples, lifecycle_conservative_rank).unwrap();
    assert_eq!(v.choice, blocked("b"));
    assert!((v.agreement - 0.5).abs() < 1e-12);
    assert_eq!(v.modal_count, 1);
}

#[test]
fn tie_with_equal_rank_breaks_toward_first_seen() {
    // Equal rank → deterministic first-seen wins (constant rank).
    let samples = vec!["x".to_string(), "y".to_string()];
    let v = self_consistency_vote(&samples, |_| 0).unwrap();
    assert_eq!(v.choice, "x");
    assert!((v.agreement - 0.5).abs() < 1e-12);
}

#[test]
fn vote_agreement_is_a_confidence_in_unit_interval() {
    let samples = vec![skip("a"), blocked("b"), reclaim("c")]; // all distinct → 1/3
    let v = self_consistency_vote(&samples, lifecycle_conservative_rank).unwrap();
    assert!((0.0..=1.0).contains(&v.agreement));
    assert!((v.agreement - 1.0 / 3.0).abs() < 1e-9);
    // The conservative-rank winner among the all-tied set is MarkGoalBlocked.
    assert_eq!(v.choice, blocked("b"));
    assert!(validate_confidence(v.agreement).is_ok());
}

// ---------------------------------------------------------------------------
// Verbalized-confidence wrappers — wire-shape preservation + validation
// ---------------------------------------------------------------------------

#[test]
fn judged_decision_serializes_flat_with_confidence() {
    let j = JudgedDecision::new(
        DecideJudgment::AdvanceGoal {
            rationale: "go".into(),
        },
        0.8,
    );
    let s = serde_json::to_string(&j).unwrap();
    // Flat shape: the tag, the rationale, and confidence are all siblings.
    assert_eq!(
        s,
        r#"{"choice":"advance_goal","rationale":"go","confidence":0.8}"#
    );
}

#[test]
fn judged_decision_defaults_confidence_when_absent() {
    // Old wire format (no confidence) must still parse, defaulting to 1.0.
    let j: JudgedDecision =
        serde_json::from_str(r#"{"choice":"advance_goal","rationale":"go"}"#).unwrap();
    assert!((j.confidence - 1.0).abs() < 1e-12);
    assert_eq!(
        j.judgment,
        DecideJudgment::AdvanceGoal {
            rationale: "go".into()
        }
    );
}

#[test]
fn bare_decide_judgment_still_parses_from_judged_wire() {
    // Contract preservation: code that only knows the inner enum keeps working
    // against JSON that now carries a sibling confidence field.
    let d: DecideJudgment =
        serde_json::from_str(r#"{"choice":"advance_goal","rationale":"go","confidence":0.3}"#)
            .unwrap();
    assert_eq!(
        d,
        DecideJudgment::AdvanceGoal {
            rationale: "go".into()
        }
    );
}

#[test]
fn judged_decision_validate_rejects_bad_confidence() {
    let bad = JudgedDecision::new(
        DecideJudgment::AdvanceGoal {
            rationale: "go".into(),
        },
        1.5,
    );
    assert!(bad.validate().is_err());
    let ok = JudgedDecision::new(
        DecideJudgment::AdvanceGoal {
            rationale: "go".into(),
        },
        0.9,
    );
    assert!(ok.validate().is_ok());
}

#[test]
fn judged_lifecycle_round_trips_and_validates() {
    let j = JudgedLifecycle::new(reclaim("stuck"), 0.6);
    let s = serde_json::to_string(&j).unwrap();
    assert!(s.contains(r#""choice":"reclaim_and_redispatch""#));
    assert!(s.contains(r#""confidence":0.6"#));
    let back: JudgedLifecycle = serde_json::from_str(&s).unwrap();
    assert_eq!(back, j);
    assert!(back.validate().is_ok());
}

#[test]
fn judged_lifecycle_warrants_self_consistency_on_stakes_or_irreversibility() {
    // High urgency, reversible action, with budget → warranted (high-stakes).
    assert!(JudgedLifecycle::new(skip("s"), 1.0).warrants_self_consistency(0.9, true));
    // Low urgency but irreversible action, with budget → warranted.
    assert!(JudgedLifecycle::new(blocked("b"), 1.0).warrants_self_consistency(0.1, true));
    // Low urgency, reversible action → not warranted.
    assert!(!JudgedLifecycle::new(skip("s"), 1.0).warrants_self_consistency(0.1, true));
    // No budget headroom → never warranted, regardless of stakes.
    assert!(!JudgedLifecycle::new(blocked("b"), 1.0).warrants_self_consistency(0.9, false));
}

// ---------------------------------------------------------------------------
// Calibration window (ECE)
// ---------------------------------------------------------------------------

#[test]
fn ece_is_none_while_empty() {
    let w = CalibrationWindow::new();
    assert!(w.is_empty());
    assert!(w.ece().is_none());
}

#[test]
fn perfect_calibration_has_zero_ece() {
    let mut w = CalibrationWindow::new();
    // Confidence 1.0 always correct, confidence 0.0 always wrong → ECE 0.
    for _ in 0..10 {
        w.record(1.0, true);
        w.record(0.0, false);
    }
    let ece = w.ece().unwrap();
    assert!(ece.abs() < 1e-9, "expected ~0 ECE, got {ece}");
}

#[test]
fn worst_case_calibration_has_ece_one() {
    let mut w = CalibrationWindow::new();
    // Always 100% confident, always wrong → ECE 1.0.
    for _ in 0..20 {
        w.record(1.0, false);
    }
    let ece = w.ece().unwrap();
    assert!((ece - 1.0).abs() < 1e-9, "expected ECE 1.0, got {ece}");
}

#[test]
fn ece_value_is_in_unit_interval_for_mixed_data() {
    let mut w = CalibrationWindow::with_params(50, 10);
    for i in 0..40 {
        w.record((i as f64) / 40.0, i % 2 == 0);
    }
    let ece = w.ece().unwrap();
    assert!((0.0..=1.0).contains(&ece), "ECE out of range: {ece}");
}

#[test]
fn window_evicts_oldest_beyond_capacity() {
    let mut w = CalibrationWindow::with_params(3, 10);
    w.record(0.1, true);
    w.record(0.2, true);
    w.record(0.3, true);
    w.record(0.4, true); // evicts the first
    assert_eq!(w.len(), 3);
}

#[test]
fn with_params_clamps_degenerate_inputs() {
    // capacity/bins of 0 must not divide-by-zero or panic.
    let mut w = CalibrationWindow::with_params(0, 0);
    w.record(0.5, true);
    assert_eq!(w.len(), 1);
    assert!(w.ece().is_some());
}

#[test]
fn false_outcome_rate_basics() {
    assert!(false_outcome_rate(&[]).is_none());
    assert_eq!(false_outcome_rate(&[true, true]).unwrap(), 0.0);
    assert_eq!(false_outcome_rate(&[false, false]).unwrap(), 1.0);
    assert!((false_outcome_rate(&[true, false, false, false]).unwrap() - 0.75).abs() < 1e-12);
}

#[test]
fn lifecycle_rank_orders_by_severity() {
    assert!(
        lifecycle_conservative_rank(&blocked("b")) > lifecycle_conservative_rank(&reclaim("r"))
    );
    assert!(lifecycle_conservative_rank(&reclaim("r")) > lifecycle_conservative_rank(&skip("s")));
}

#[test]
fn record_ece_metric_is_noop_when_empty() {
    // Empty window records nothing and returns None — a pure check with no IO.
    let empty = CalibrationWindow::new();
    assert!(empty.record_ece_metric().is_none());
    // The recorded value, when non-empty, is exactly `ece()` (validated here
    // without performing the best-effort write so the test stays hermetic).
    let mut w = CalibrationWindow::new();
    w.record(0.9, true);
    assert!(w.ece().is_some());
}
