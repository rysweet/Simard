use super::types::{CoinGymError, Outcome, OutcomeCode, RunReport, Strategy, Target, TargetFamily};

fn target(id: &str, family: TargetFamily) -> Target {
    Target {
        id: id.to_string(),
        project: "proj".to_string(),
        commit: "abc".to_string(),
        harness: "h".to_string(),
        file: "src/x.c".to_string(),
        line: 42,
        family,
    }
}

#[test]
fn target_family_labels_are_stable() {
    assert_eq!(TargetFamily::Frontier.label(), "frontier");
    assert_eq!(
        TargetFamily::NonTrivialReachable.label(),
        "non-trivial-reachable"
    );
    assert_eq!(TargetFamily::Frontier.to_string(), "frontier");
}

#[test]
fn target_family_serde_round_trip_kebab() {
    let json = serde_json::to_string(&TargetFamily::NonTrivialReachable).unwrap();
    assert_eq!(json, "\"non-trivial-reachable\"");
    let back: TargetFamily = serde_json::from_str(&json).unwrap();
    assert_eq!(back, TargetFamily::NonTrivialReachable);
}

#[test]
fn target_locator_is_project_file_line() {
    let t = target("t1", TargetFamily::Frontier);
    assert_eq!(t.locator(), "proj:src/x.c:42");
}

#[test]
fn strategy_parse_accepts_known_and_rejects_unknown() {
    assert_eq!(Strategy::parse("baseline").unwrap(), Strategy::Baseline);
    assert_eq!(Strategy::parse("team").unwrap(), Strategy::Team);
    assert!(Strategy::parse("solo").is_err());
    assert_eq!(Strategy::Team.label(), "team");
    assert_eq!(Strategy::Baseline.to_string(), "baseline");
}

#[test]
fn outcome_code_letters_and_predicates() {
    assert_eq!(OutcomeCode::Reached.letter(), 'R');
    assert_eq!(OutcomeCode::WrongInput.letter(), 'W');
    assert_eq!(OutcomeCode::Abstained.letter(), 'A');
    assert_eq!(OutcomeCode::TimedOut.letter(), 'T');
    assert_eq!(OutcomeCode::NoSubmission.letter(), 'N');
    assert_eq!(OutcomeCode::Error.letter(), 'E');

    assert!(OutcomeCode::Reached.reached());
    assert!(!OutcomeCode::WrongInput.reached());

    // Only submitted inputs count toward precision's denominator.
    assert!(OutcomeCode::Reached.submitted());
    assert!(OutcomeCode::WrongInput.submitted());
    assert!(OutcomeCode::TimedOut.submitted());
    assert!(!OutcomeCode::Abstained.submitted());
    assert!(!OutcomeCode::NoSubmission.submitted());
    assert!(!OutcomeCode::Error.submitted());
}

#[test]
fn outcome_helpers_delegate_to_code() {
    let o = Outcome {
        target_id: "t".to_string(),
        family: TargetFamily::Frontier,
        code: OutcomeCode::Reached,
        cost_usd: 1.5,
    };
    assert!(o.reached());
    assert!(o.submitted());
}

#[test]
fn run_report_serde_round_trip() {
    let report = RunReport {
        run_id: "m-baseline-1".to_string(),
        model: "m".to_string(),
        strategy: Strategy::Baseline,
        snapshot: "you/coin@v1".to_string(),
        started_at_unix_ms: 123,
        outcomes: vec![Outcome {
            target_id: "t".to_string(),
            family: TargetFamily::Frontier,
            code: OutcomeCode::Reached,
            cost_usd: 0.0,
        }],
        offline_scaffold: true,
    };
    let json = serde_json::to_string(&report).unwrap();
    let back: RunReport = serde_json::from_str(&json).unwrap();
    assert_eq!(back, report);
    assert_eq!(back.target_count(), 1);
}

#[test]
fn error_display_is_readable() {
    assert!(
        CoinGymError::NotFound("run x".to_string())
            .to_string()
            .contains("not found")
    );
    assert_eq!(CoinGymError::Usage("boom".to_string()).to_string(), "boom");
}
