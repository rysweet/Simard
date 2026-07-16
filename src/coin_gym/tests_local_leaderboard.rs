use super::local_leaderboard::build_local_leaderboard;
use super::types::{Outcome, OutcomeCode, RunReport, Strategy, TargetFamily};

const EPS: f64 = 1e-9;

/// Build a `RunReport` from a list of outcome codes (all `frontier`), so tests
/// can pin exact reach/precision without touching disk or the executor.
fn report(
    run_id: &str,
    model: &str,
    strategy: Strategy,
    codes: &[OutcomeCode],
    offline: bool,
) -> RunReport {
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
        run_id: run_id.to_string(),
        model: model.to_string(),
        strategy,
        snapshot: "you/coin@v1-sample".to_string(),
        started_at_unix_ms: 0,
        outcomes,
        offline_scaffold: offline,
    }
}

/// The bundled-fixture shape: baseline reaches 3/5 by over-claiming twice;
/// team reaches the same 3/5 but abstains on the two low-confidence targets.
fn baseline_report(run_id: &str, offline: bool) -> RunReport {
    use OutcomeCode::{Reached, WrongInput};
    report(
        run_id,
        "Claude Opus 4.6",
        Strategy::Baseline,
        &[Reached, Reached, Reached, WrongInput, WrongInput],
        offline,
    )
}

fn team_report(run_id: &str, offline: bool) -> RunReport {
    use OutcomeCode::{Abstained, Reached};
    report(
        run_id,
        "Claude Opus 4.6",
        Strategy::Team,
        &[Reached, Reached, Reached, Abstained, Abstained],
        offline,
    )
}

#[test]
fn ranks_team_above_baseline_at_equal_reach() {
    let board = build_local_leaderboard(
        "profile 'p'",
        vec![
            ("p".to_string(), baseline_report("b-1", true)),
            ("p".to_string(), team_report("t-1", true)),
        ],
    );
    assert_eq!(board.standings.len(), 2);
    // Equal reach (60%) but the team's higher precision (100% vs 60%) ranks it first.
    assert_eq!(board.standings[0].rank, 1);
    assert_eq!(board.standings[0].strategy, Strategy::Team);
    assert_eq!(board.standings[1].strategy, Strategy::Baseline);
    assert!((board.standings[0].reach_pct - 60.0).abs() < EPS);
    assert!((board.standings[0].precision_pct - 100.0).abs() < EPS);
    assert!((board.standings[1].precision_pct - 60.0).abs() < EPS);
}

#[test]
fn baseline_vs_team_reports_the_climb() {
    let board = build_local_leaderboard(
        "profile 'p'",
        vec![
            ("p".to_string(), baseline_report("b-1", true)),
            ("p".to_string(), team_report("t-1", true)),
        ],
    );
    let cmp = board.baseline_vs_team.expect("both arms present");
    assert_eq!(cmp.baseline_run_id, "b-1");
    assert_eq!(cmp.team_run_id, "t-1");
    assert!((cmp.reach_delta_pct).abs() < EPS, "equal reach");
    assert!(
        (cmp.precision_delta_pct - 40.0).abs() < EPS,
        "precision +40 pts"
    );
    assert!(cmp.team_beats_baseline);
    assert!(cmp.verdict.contains("CLIMBS ABOVE"));
}

#[test]
fn one_arm_board_has_no_comparison() {
    let board = build_local_leaderboard(
        "profile 'p'",
        vec![("p".to_string(), baseline_report("b-1", true))],
    );
    assert!(board.baseline_vs_team.is_none());
    assert_eq!(board.standings.len(), 1);
}

#[test]
fn team_below_baseline_when_it_reaches_less() {
    use OutcomeCode::{NoSubmission, Reached};
    // A degenerate team that abstains itself out of most targets: lower reach.
    let weak_team = report(
        "t-weak",
        "Claude Opus 4.6",
        Strategy::Team,
        &[
            Reached,
            NoSubmission,
            NoSubmission,
            NoSubmission,
            NoSubmission,
        ],
        true,
    );
    let board = build_local_leaderboard(
        "profile 'p'",
        vec![
            ("p".to_string(), baseline_report("b-1", true)),
            ("p".to_string(), weak_team),
        ],
    );
    let cmp = board.baseline_vs_team.expect("both arms present");
    assert!(!cmp.team_beats_baseline);
    assert!(cmp.reach_delta_pct < -EPS);
    assert!(cmp.verdict.contains("BELOW"));
    // The higher-reach baseline ranks first.
    assert_eq!(board.standings[0].strategy, Strategy::Baseline);
}

#[test]
fn identical_arms_are_tied() {
    let board = build_local_leaderboard(
        "profile 'p'",
        vec![
            ("p".to_string(), baseline_report("b-1", true)),
            (
                "p".to_string(),
                report(
                    "t-1",
                    "Claude Opus 4.6",
                    Strategy::Team,
                    &[
                        OutcomeCode::Reached,
                        OutcomeCode::Reached,
                        OutcomeCode::Reached,
                        OutcomeCode::WrongInput,
                        OutcomeCode::WrongInput,
                    ],
                    true,
                ),
            ),
        ],
    );
    let cmp = board.baseline_vs_team.expect("both arms present");
    assert!(!cmp.team_beats_baseline);
    assert!(cmp.verdict.contains("TIED"));
}

#[test]
fn offline_flag_propagates_only_when_present() {
    let offline = build_local_leaderboard("s", vec![("p".to_string(), team_report("t-1", true))]);
    assert!(offline.any_offline_scaffold);
    let live = build_local_leaderboard("s", vec![("p".to_string(), team_report("t-1", false))]);
    assert!(!live.any_offline_scaffold);
}

#[test]
fn ordering_is_deterministic_across_input_order_and_ties() {
    // Two identical-score baseline runs must order by (profile, run_id), stably,
    // regardless of the order they are supplied in.
    let mk = || {
        vec![
            ("z".to_string(), baseline_report("b-2", true)),
            ("a".to_string(), baseline_report("b-1", true)),
        ]
    };
    let forward = build_local_leaderboard("s", mk());
    let reversed = build_local_leaderboard("s", mk().into_iter().rev().collect::<Vec<_>>());
    let ids = |b: &super::local_leaderboard::LocalLeaderboard| -> Vec<(String, String)> {
        b.standings
            .iter()
            .map(|s| (s.profile.clone(), s.run_id.clone()))
            .collect()
    };
    assert_eq!(ids(&forward), ids(&reversed));
    // (profile) is the higher-priority tiebreak: 'a' before 'z'.
    assert_eq!(forward.standings[0].profile, "a");
    assert_eq!(forward.standings[1].profile, "z");
}

#[test]
fn empty_board_is_empty() {
    let board = build_local_leaderboard("profile 'none'", Vec::new());
    assert!(board.standings.is_empty());
    assert!(board.baseline_vs_team.is_none());
    assert!(!board.any_offline_scaffold);
}
