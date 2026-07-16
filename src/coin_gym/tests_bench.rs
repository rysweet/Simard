use super::bench::{BenchOutcome, bench_verdict};
use super::profiles::runs_dir;
use super::scorer::{Score, score_run};
use super::target_loader::DemoScenario;
use super::types::Strategy;
use super::{dispatch_with_home, execute_run};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn sample_scores(model: &str) -> (Score, Score) {
    let scenario = DemoScenario::sample().unwrap();
    let baseline = execute_run(model, Strategy::Baseline, &scenario).unwrap();
    let team = execute_run(model, Strategy::Team, &scenario).unwrap();
    (score_run(&baseline), score_run(&team))
}

#[test]
fn verdict_on_sample_is_multiagent_wins_on_precision_at_equal_reach() {
    let (baseline, team) = sample_scores("claude-opus-4.6");
    let verdict = bench_verdict(&baseline, &team);

    // On the bundled sample the team ties reach (0 pts) and lifts precision
    // (+40 pts) via its abstention gate, so the team strictly wins.
    assert_eq!(verdict.outcome, BenchOutcome::MultiagentWins);
    assert!(approx(verdict.reach_delta_pct, 0.0));
    assert!(approx(verdict.precision_delta_pct, 40.0));
    assert_eq!(verdict.model, "claude-opus-4.6");
    assert!(verdict.offline_scaffold);
    assert!(verdict.note.contains("LOCAL-ONLY"));
    assert!(verdict.note.contains("beats the single-model baseline"));
}

#[test]
fn verdict_is_tie_when_scores_are_identical() {
    let (baseline, _team) = sample_scores("claude-opus-4.6");
    // Compare an arm against itself: no metric moves.
    let verdict = bench_verdict(&baseline, &baseline);
    assert_eq!(verdict.outcome, BenchOutcome::Tie);
    assert!(approx(verdict.reach_delta_pct, 0.0));
    assert!(approx(verdict.precision_delta_pct, 0.0));
    assert!(verdict.note.contains("ties the single-model baseline"));
}

#[test]
fn verdict_is_regression_when_team_metric_drops() {
    let (baseline, team) = sample_scores("claude-opus-4.6");
    // The baseline has higher precision-denominator submissions; swapping the
    // arms (team as "baseline", baseline as "team") makes precision drop, which
    // must be reported as a regression, never silently as a win.
    let verdict = bench_verdict(&team, &baseline);
    assert_eq!(verdict.outcome, BenchOutcome::Regression);
    assert!(verdict.precision_delta_pct < 0.0);
    assert!(
        verdict
            .note
            .contains("regresses vs the single-model baseline")
    );
}

#[test]
fn bench_command_writes_both_arms_and_labels_the_profile() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    dispatch_with_home(
        home,
        args(&["bench", "claude-opus-4.6", "--profile", "opus"]),
    )
    .unwrap();

    let runs = runs_dir(home, "opus");
    let entries: Vec<_> = std::fs::read_dir(&runs)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect();
    // One baseline arm + one team arm are both persisted.
    assert_eq!(entries.len(), 2, "bench should persist both arms");
}

#[test]
fn bench_requires_model() {
    let dir = tempfile::tempdir().unwrap();
    let err = dispatch_with_home(dir.path(), args(&["bench"])).unwrap_err();
    assert!(err.to_string().contains("expected <model>"));
}

#[test]
fn bench_rejects_unknown_flag() {
    let dir = tempfile::tempdir().unwrap();
    let err =
        dispatch_with_home(dir.path(), args(&["bench", "m", "--strategy", "team"])).unwrap_err();
    assert!(err.to_string().contains("unknown flag"));
}
