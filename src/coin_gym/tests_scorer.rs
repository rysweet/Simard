use super::scorer::{OutcomeHistogram, ReachPrecision, score_run};
use super::types::{Outcome, OutcomeCode, RunReport, Strategy, TargetFamily};

fn outcome(id: &str, family: TargetFamily, code: OutcomeCode) -> Outcome {
    Outcome {
        target_id: id.to_string(),
        family,
        code,
        cost_usd: 0.0,
    }
}

fn report(outcomes: Vec<Outcome>) -> RunReport {
    RunReport {
        run_id: "r1".to_string(),
        model: "m".to_string(),
        strategy: Strategy::Baseline,
        snapshot: "snap".to_string(),
        started_at_unix_ms: 0,
        outcomes,
        offline_scaffold: true,
    }
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn histogram_tallies_every_code() {
    let outcomes = vec![
        outcome("a", TargetFamily::Frontier, OutcomeCode::Reached),
        outcome("b", TargetFamily::Frontier, OutcomeCode::WrongInput),
        outcome("c", TargetFamily::Frontier, OutcomeCode::Abstained),
        outcome("d", TargetFamily::Frontier, OutcomeCode::TimedOut),
        outcome("e", TargetFamily::Frontier, OutcomeCode::NoSubmission),
        outcome("f", TargetFamily::Frontier, OutcomeCode::Error),
    ];
    let h = OutcomeHistogram::tally(&outcomes);
    assert_eq!(h.reached, 1);
    assert_eq!(h.wrong_input, 1);
    assert_eq!(h.abstained, 1);
    assert_eq!(h.timed_out, 1);
    assert_eq!(h.no_submission, 1);
    assert_eq!(h.error, 1);
    assert_eq!(h.total(), 6);
    assert_eq!(h.render(), "R:1/W:1/A:1/T:1/N:1/E:1");
}

#[test]
fn reach_and_precision_use_correct_denominators() {
    // 1 reached, 1 wrong (submitted), 1 abstained (not submitted).
    let outcomes = vec![
        outcome("a", TargetFamily::Frontier, OutcomeCode::Reached),
        outcome("b", TargetFamily::Frontier, OutcomeCode::WrongInput),
        outcome("c", TargetFamily::Frontier, OutcomeCode::Abstained),
    ];
    let rp = ReachPrecision::compute(&outcomes);
    assert_eq!(rp.reached, 1);
    assert_eq!(rp.submitted, 2);
    assert_eq!(rp.total, 3);
    assert!(approx(rp.reach_rate, 1.0 / 3.0));
    assert!(approx(rp.precision, 0.5));
    assert!(approx(rp.reach_pct(), 100.0 / 3.0));
    assert!(approx(rp.precision_pct(), 50.0));
}

#[test]
fn precision_is_zero_when_nothing_submitted() {
    let outcomes = vec![
        outcome("a", TargetFamily::Frontier, OutcomeCode::Abstained),
        outcome("b", TargetFamily::Frontier, OutcomeCode::NoSubmission),
    ];
    let rp = ReachPrecision::compute(&outcomes);
    assert!(approx(rp.precision, 0.0));
    assert!(approx(rp.reach_rate, 0.0));
}

#[test]
fn score_run_splits_by_family_and_skips_empty() {
    let outcomes = vec![
        outcome("f1", TargetFamily::Frontier, OutcomeCode::Reached),
        outcome("f2", TargetFamily::Frontier, OutcomeCode::WrongInput),
        outcome(
            "n1",
            TargetFamily::NonTrivialReachable,
            OutcomeCode::Reached,
        ),
    ];
    let score = score_run(&report(outcomes));
    assert_eq!(score.by_family.len(), 2);

    let frontier = score
        .by_family
        .iter()
        .find(|f| f.family == TargetFamily::Frontier)
        .unwrap();
    assert!(approx(frontier.score.reach_rate, 0.5));
    assert!(approx(frontier.score.precision, 0.5));

    let ntr = score
        .by_family
        .iter()
        .find(|f| f.family == TargetFamily::NonTrivialReachable)
        .unwrap();
    assert!(approx(ntr.score.reach_rate, 1.0));

    assert!(score.offline_scaffold);
    assert_eq!(score.model, "m");
}

#[test]
fn single_family_run_yields_one_family_entry() {
    let outcomes = vec![outcome("f1", TargetFamily::Frontier, OutcomeCode::Reached)];
    let score = score_run(&report(outcomes));
    assert_eq!(score.by_family.len(), 1);
    assert_eq!(score.by_family[0].family, TargetFamily::Frontier);
}
