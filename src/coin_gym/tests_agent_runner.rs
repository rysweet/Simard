use std::collections::HashMap;

use super::agent_runner::{
    AgentRunner, AgentStrategy, BaselineStrategy, Candidate, DEFAULT_THRESHOLD_HINT,
    FixtureReasoner, Reasoner, SubmissionDecision, TeamStrategy,
};
use super::executor::MockHarnessExecutor;
use super::scorer::score_run;
use super::types::{OutcomeCode, Strategy, Target, TargetFamily};

fn t(id: &str, family: TargetFamily) -> Target {
    Target {
        id: id.to_string(),
        project: "proj".to_string(),
        commit: "c".to_string(),
        harness: "h".to_string(),
        file: "src/x.c".to_string(),
        line: 10,
        family,
    }
}

fn cand(input: &str, confidence: f64) -> Candidate {
    Candidate {
        input: input.to_string(),
        confidence,
        rationale: "because".to_string(),
    }
}

/// Four targets that make the baseline-vs-team precision trade-off explicit:
/// two high-confidence correct (both submit → R), two low-confidence wrong
/// (baseline submits → W; team abstains → A).
fn ab_scenario() -> (Vec<Target>, FixtureReasoner, MockHarnessExecutor) {
    let targets = vec![
        t("hit-a", TargetFamily::Frontier),
        t("wrong-lowconf-a", TargetFamily::Frontier),
        t("wrong-lowconf-b", TargetFamily::NonTrivialReachable),
        t("hit-b", TargetFamily::NonTrivialReachable),
    ];
    let mut script = HashMap::new();
    script.insert("hit-a".to_string(), cand("correct-a", 0.9));
    script.insert("wrong-lowconf-a".to_string(), cand("guess-a", 0.3));
    script.insert("wrong-lowconf-b".to_string(), cand("guess-b", 0.4));
    script.insert("hit-b".to_string(), cand("correct-b", 0.8));
    let reasoner = FixtureReasoner::new(script);

    let oracle: HashMap<String, String> = [
        ("hit-a", "correct-a"),
        ("wrong-lowconf-a", "actual-a"),
        ("wrong-lowconf-b", "actual-b"),
        ("hit-b", "correct-b"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
    .collect();
    let executor = MockHarnessExecutor::from_oracle(oracle);
    (targets, reasoner, executor)
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn baseline_submits_everything() {
    let (targets, reasoner, executor) = ab_scenario();
    let strategy = BaselineStrategy::new(reasoner);
    let report = AgentRunner::new(&strategy, &executor, "m", "snap")
        .run(&targets)
        .unwrap();
    assert_eq!(report.strategy, Strategy::Baseline);
    let codes: Vec<OutcomeCode> = report.outcomes.iter().map(|o| o.code).collect();
    assert_eq!(
        codes,
        vec![
            OutcomeCode::Reached,
            OutcomeCode::WrongInput,
            OutcomeCode::WrongInput,
            OutcomeCode::Reached,
        ]
    );
    let score = score_run(&report);
    assert!(approx(score.overall.reach_rate, 0.5));
    assert!(approx(score.overall.precision, 0.5)); // 2 reached / 4 submitted
}

#[test]
fn team_abstains_on_low_confidence_and_lifts_precision() {
    let (targets, reasoner, executor) = ab_scenario();
    let strategy = TeamStrategy::new(reasoner);
    assert!(approx(strategy.threshold_hint(), DEFAULT_THRESHOLD_HINT));
    let report = AgentRunner::new(&strategy, &executor, "m", "snap")
        .run(&targets)
        .unwrap();
    assert_eq!(report.strategy, Strategy::Team);
    let codes: Vec<OutcomeCode> = report.outcomes.iter().map(|o| o.code).collect();
    assert_eq!(
        codes,
        vec![
            OutcomeCode::Reached,
            OutcomeCode::Abstained,
            OutcomeCode::Abstained,
            OutcomeCode::Reached,
        ]
    );
    let score = score_run(&report);
    // Same reach as baseline, but precision rises to 1.0 (no wrong submissions).
    assert!(approx(score.overall.reach_rate, 0.5));
    assert!(approx(score.overall.precision, 1.0));
}

#[test]
fn run_ids_are_unique_across_repeated_runs() {
    let (targets, reasoner, executor) = ab_scenario();
    let strategy = BaselineStrategy::new(reasoner);
    let r1 = AgentRunner::new(&strategy, &executor, "m", "snap")
        .run(&targets)
        .unwrap();
    let r2 = AgentRunner::new(&strategy, &executor, "m", "snap")
        .run(&targets)
        .unwrap();
    assert_ne!(r1.run_id, r2.run_id, "run ids must not collide");
}

#[test]
fn no_candidate_yields_no_submission() {
    let targets = vec![t("orphan", TargetFamily::Frontier)];
    let reasoner = FixtureReasoner::default(); // empty script
    let executor = MockHarnessExecutor::new();
    let strategy = BaselineStrategy::new(reasoner);
    let report = AgentRunner::new(&strategy, &executor, "m", "snap")
        .run(&targets)
        .unwrap();
    assert_eq!(report.outcomes[0].code, OutcomeCode::NoSubmission);
}

#[test]
fn team_threshold_is_clamped() {
    let reasoner = FixtureReasoner::default();
    let hi = TeamStrategy::with_threshold(reasoner.clone(), 5.0);
    assert!(approx(hi.threshold_hint(), 1.0));
    let lo = TeamStrategy::with_threshold(reasoner, -1.0);
    assert!(approx(lo.threshold_hint(), 0.0));
}

#[test]
fn team_evaluate_produces_expected_decisions() {
    let mut script = HashMap::new();
    script.insert("go".to_string(), cand("x", 0.75));
    script.insert("stop".to_string(), cand("y", 0.2));
    let reasoner = FixtureReasoner::new(script);
    let strategy = TeamStrategy::new(reasoner);

    let submit = strategy.evaluate(&t("go", TargetFamily::Frontier));
    assert!(matches!(submit.decision, SubmissionDecision::Submit { .. }));

    let abstain = strategy.evaluate(&t("stop", TargetFamily::Frontier));
    assert!(matches!(
        abstain.decision,
        SubmissionDecision::Abstain { .. }
    ));

    let none = strategy.evaluate(&t("missing", TargetFamily::Frontier));
    assert!(matches!(none.decision, SubmissionDecision::NoSubmission));
}

#[test]
fn custom_reasoner_assess_can_override_confidence() {
    // A skeptic that always distrusts: assess returns 0.0 → team always abstains.
    #[derive(Clone)]
    struct Skeptic;
    impl Reasoner for Skeptic {
        fn propose(&self, _target: &Target) -> Option<Candidate> {
            Some(cand("anything", 0.99))
        }
        fn assess(&self, _target: &Target, _candidate: &Candidate) -> f64 {
            0.0
        }
    }
    let strategy = TeamStrategy::new(Skeptic);
    let out = strategy.evaluate(&t("x", TargetFamily::Frontier));
    assert!(matches!(out.decision, SubmissionDecision::Abstain { .. }));
}
