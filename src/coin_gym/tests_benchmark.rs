//! Unit tests for the head-to-head baseline-vs-team benchmark.

use super::benchmark::{DEFAULT_MARGIN_PCT, HeadToHead, MetricMove, Verdict};
use super::execute_run;
use super::target_loader::DemoScenario;
use super::types::{Outcome, OutcomeCode, RunReport, Strategy, TargetFamily};

/// Build a synthetic report from a list of outcome codes (all `frontier`; family
/// does not affect the overall reach/precision the verdict keys off).
fn report(model: &str, strategy: Strategy, codes: &[OutcomeCode]) -> RunReport {
    let outcomes = codes
        .iter()
        .enumerate()
        .map(|(i, &code)| Outcome {
            target_id: format!("t{i}"),
            family: TargetFamily::Frontier,
            code,
            cost_usd: 0.0,
        })
        .collect();
    RunReport {
        run_id: format!("{}-{}-0-0", model, strategy.label()),
        model: model.to_string(),
        strategy,
        snapshot: "you/coin@v1-sample".to_string(),
        started_at_unix_ms: 0,
        outcomes,
        offline_scaffold: true,
    }
}

use OutcomeCode::{Abstained, NoSubmission, Reached, WrongInput};

#[test]
fn metric_move_classifies_against_margin() {
    assert_eq!(MetricMove::classify(5.0, 1.0), MetricMove::Improved);
    assert_eq!(MetricMove::classify(-5.0, 1.0), MetricMove::Regressed);
    assert_eq!(MetricMove::classify(0.0, 1.0), MetricMove::Flat);
    // Exactly at the margin is NOT measurable (strict inequality).
    assert_eq!(MetricMove::classify(1.0, 1.0), MetricMove::Flat);
    assert_eq!(MetricMove::classify(-1.0, 1.0), MetricMove::Flat);
    assert_eq!(MetricMove::classify(1.0001, 1.0), MetricMove::Improved);
}

#[test]
fn verdict_covers_all_quadrants() {
    use MetricMove::{Flat, Improved, Regressed};
    assert_eq!(
        Verdict::from_moves(Flat, Improved),
        Verdict::TeamBeatsBaseline
    );
    assert_eq!(
        Verdict::from_moves(Improved, Flat),
        Verdict::TeamBeatsBaseline
    );
    assert_eq!(
        Verdict::from_moves(Regressed, Flat),
        Verdict::BaselineBeatsTeam
    );
    assert_eq!(Verdict::from_moves(Flat, Flat), Verdict::Tie);
    assert_eq!(
        Verdict::from_moves(Improved, Regressed),
        Verdict::MixedTradeoff
    );
    assert!(Verdict::TeamBeatsBaseline.is_team_win());
    assert!(!Verdict::MixedTradeoff.is_team_win());
    assert!(!Verdict::Tie.is_team_win());
    assert!(!Verdict::BaselineBeatsTeam.is_team_win());
}

#[test]
fn sample_scenario_team_measurably_beats_baseline() {
    let scenario = DemoScenario::sample().unwrap();
    let baseline = execute_run("claude-opus-4.6", Strategy::Baseline, &scenario).unwrap();
    let team = execute_run("claude-opus-4.6", Strategy::Team, &scenario).unwrap();
    let h2h = HeadToHead::from_reports(&baseline, &team, DEFAULT_MARGIN_PCT);

    // Reach ties at 60%; precision climbs 60% → 100% via the abstain gate.
    assert!((h2h.reach_delta_pct - 0.0).abs() < 1e-9);
    assert!((h2h.precision_delta_pct - 40.0).abs() < 1e-9);
    assert_eq!(h2h.reach_move, MetricMove::Flat);
    assert_eq!(h2h.precision_move, MetricMove::Improved);
    assert_eq!(h2h.verdict, Verdict::TeamBeatsBaseline);
    assert!(h2h.verdict.is_team_win());
    assert!(h2h.offline_scaffold);
    assert_eq!(h2h.baseline_run_id, baseline.run_id);
    assert_eq!(h2h.team_run_id, team.run_id);
}

#[test]
fn baseline_beats_team_when_team_regresses_both() {
    let baseline = report("m", Strategy::Baseline, &[Reached, Reached]);
    let team = report("m", Strategy::Team, &[Reached, WrongInput]);
    let h2h = HeadToHead::from_reports(&baseline, &team, DEFAULT_MARGIN_PCT);
    // baseline reach 100/prec 100; team reach 50/prec 50.
    assert_eq!(h2h.reach_move, MetricMove::Regressed);
    assert_eq!(h2h.precision_move, MetricMove::Regressed);
    assert_eq!(h2h.verdict, Verdict::BaselineBeatsTeam);
}

#[test]
fn tie_when_both_within_margin() {
    let baseline = report("m", Strategy::Baseline, &[Reached, WrongInput]);
    let team = report("m", Strategy::Team, &[Reached, WrongInput]);
    let h2h = HeadToHead::from_reports(&baseline, &team, DEFAULT_MARGIN_PCT);
    assert_eq!(h2h.verdict, Verdict::Tie);
    assert!((h2h.reach_delta_pct).abs() < 1e-9);
    assert!((h2h.precision_delta_pct).abs() < 1e-9);
}

#[test]
fn mixed_tradeoff_when_reach_up_but_precision_down() {
    // baseline: reach 25% (1/4), precision 100% (1/1 submitted; 3 abstain).
    let baseline = report(
        "m",
        Strategy::Baseline,
        &[Reached, Abstained, Abstained, Abstained],
    );
    // team: reach 50% (2/4), precision 50% (2/4 submitted).
    let team = report(
        "m",
        Strategy::Team,
        &[Reached, Reached, WrongInput, WrongInput],
    );
    let h2h = HeadToHead::from_reports(&baseline, &team, DEFAULT_MARGIN_PCT);
    assert_eq!(h2h.reach_move, MetricMove::Improved);
    assert_eq!(h2h.precision_move, MetricMove::Regressed);
    assert_eq!(h2h.verdict, Verdict::MixedTradeoff);
}

#[test]
fn negative_margin_is_clamped_to_zero() {
    let baseline = report("m", Strategy::Baseline, &[Reached, WrongInput]);
    let team = report("m", Strategy::Team, &[Reached, Abstained]);
    let h2h = HeadToHead::from_reports(&baseline, &team, -5.0);
    assert!((h2h.margin_pct - 0.0).abs() < 1e-9);
    // precision 50% → 100% is measurable even at margin 0.
    assert_eq!(h2h.verdict, Verdict::TeamBeatsBaseline);
}

#[test]
fn no_submission_does_not_inflate_precision_denominator() {
    // A NoSubmission is neither reached nor submitted; precision keys off
    // submitted only, so team abstaining/no-submitting protects precision.
    let baseline = report(
        "m",
        Strategy::Baseline,
        &[Reached, WrongInput, NoSubmission],
    );
    let team = report("m", Strategy::Team, &[Reached, Abstained, NoSubmission]);
    let h2h = HeadToHead::from_reports(&baseline, &team, DEFAULT_MARGIN_PCT);
    // baseline precision 1/2 = 50%; team precision 1/1 = 100%.
    assert!((h2h.precision_delta_pct - 50.0).abs() < 1e-9);
    assert_eq!(h2h.verdict, Verdict::TeamBeatsBaseline);
}

#[test]
fn head_to_head_json_round_trips_and_carries_payload() {
    let baseline = report(
        "claude-opus-4.6",
        Strategy::Baseline,
        &[Reached, WrongInput],
    );
    let team = report("claude-opus-4.6", Strategy::Team, &[Reached, Abstained]);
    let h2h = HeadToHead::from_reports(&baseline, &team, DEFAULT_MARGIN_PCT);
    let json = serde_json::to_string(&h2h).unwrap();
    // The Signal milestone payload carries the verdict, deltas, and both scores.
    assert!(json.contains("\"verdict\":\"team-beats-baseline\""));
    assert!(json.contains("\"precision_delta_pct\""));
    assert!(json.contains("\"baseline\""));
    assert!(json.contains("\"team\""));
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["model"], "claude-opus-4.6");
}

#[test]
fn signal_line_names_model_snapshot_and_verdict() {
    let baseline = report("gpt-5.4", Strategy::Baseline, &[Reached, WrongInput]);
    let team = report("gpt-5.4", Strategy::Team, &[Reached, Abstained]);
    let h2h = HeadToHead::from_reports(&baseline, &team, DEFAULT_MARGIN_PCT);
    let line = h2h.signal_line();
    assert!(line.contains("gpt-5.4"));
    assert!(line.contains("you/coin@v1-sample"));
    assert!(line.contains("TEAM-BEATS-BASELINE"));
    assert!(line.contains("offline scaffold"));
}

#[test]
fn render_shows_both_strategies_and_verdict() {
    let scenario = DemoScenario::sample().unwrap();
    let baseline = execute_run("m", Strategy::Baseline, &scenario).unwrap();
    let team = execute_run("m", Strategy::Team, &scenario).unwrap();
    let text = HeadToHead::from_reports(&baseline, &team, DEFAULT_MARGIN_PCT).render();
    assert!(text.contains("baseline:"));
    assert!(text.contains("team:"));
    assert!(text.contains("delta:"));
    assert!(text.contains("verdict:"));
    assert!(text.contains("TEAM-BEATS-BASELINE"));
}
