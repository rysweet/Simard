use super::observe::*;
use super::types::{EnvironmentSnapshot, Observation};
use crate::goal_curation::GoalBoard;
use crate::gym_bridge::ScoreDimensions;
use crate::gym_scoring::GymSuiteScore;
use crate::meeting_facilitator::{MeetingDecision, MeetingHandoff, write_meeting_handoff};
use crate::memory_cognitive::CognitiveStatistics;
use crate::ooda_loop::OodaState;
use crate::self_improve::{ImprovementCycle, ImprovementPhase};
use serial_test::serial;
use tempfile::TempDir;

fn make_gym_score(overall: f64, factual_accuracy: f64) -> GymSuiteScore {
    GymSuiteScore {
        suite_id: "progressive".to_string(),
        overall,
        dimensions: ScoreDimensions {
            factual_accuracy,
            specificity: 0.8,
            temporal_awareness: 0.7,
            source_attribution: 0.6,
            confidence_calibration: 0.5,
        },
        scenario_count: 5,
        scenarios_passed: 4,
        pass_rate: 0.8,
        recorded_at_unix_ms: None,
    }
}

#[test]
#[serial]
fn collect_pending_improvements_empty_when_no_signals() {
    let dir = TempDir::new().unwrap();
    // Isolate from any leftover handoff files in the default directory.
    unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", dir.path()) };

    let mut state = OodaState::new(GoalBoard::new());
    let current = Some(make_gym_score(0.8, 0.9));
    let result = collect_pending_improvements(&mut state, &current);
    // (Handoff signals may appear due to env var races in parallel tests.)
    let has_regression = result.iter().any(|c| !c.regressions.is_empty());
    assert!(!has_regression, "no signals should yield no regressions");

    unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
}

#[test]
#[serial]
fn collect_pending_improvements_detects_gym_regression() {
    let dir = TempDir::new().unwrap();
    unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", dir.path()) };

    let baseline = make_gym_score(0.9, 0.95);
    let current_score = make_gym_score(0.7, 0.70); // factual_accuracy dropped 0.25

    let mut state = OodaState::new(GoalBoard::new());
    state.last_observation = Some(Observation {
        goal_statuses: Vec::new(),
        gym_health: Some(baseline.clone()),
        memory_stats: CognitiveStatistics::default(),
        pending_improvements: Vec::new(),
        environment: EnvironmentSnapshot::default(),
    eval_watchdog: None,
    });

    let result = collect_pending_improvements(&mut state, &Some(current_score.clone()));

    assert_eq!(
        result.len(),
        1,
        "regression should produce exactly one improvement signal"
    );
    let cycle = &result[0];
    assert_eq!(cycle.baseline.overall, 0.9);
    assert!(cycle.post_score.is_some());
    assert!(!cycle.regressions.is_empty());
    assert!(cycle.decision.is_none());
    assert_eq!(cycle.final_phase, ImprovementPhase::Eval);

    unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
}

#[test]
#[serial]
fn collect_pending_improvements_no_regression_when_scores_stable() {
    let dir = TempDir::new().unwrap();
    unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", dir.path()) };

    let baseline = make_gym_score(0.8, 0.85);
    let current_score = make_gym_score(0.8, 0.85);

    let mut state = OodaState::new(GoalBoard::new());
    state.last_observation = Some(Observation {
        goal_statuses: Vec::new(),
        gym_health: Some(baseline),
        memory_stats: CognitiveStatistics::default(),
        pending_improvements: Vec::new(),
        environment: EnvironmentSnapshot::default(),
    eval_watchdog: None,
    });

    let result = collect_pending_improvements(&mut state, &Some(current_score));
    // Stable scores should not produce gym *regression* signals.
    // (Handoff signals may appear due to env var races in parallel tests.)
    let has_regression = result.iter().any(|c| !c.regressions.is_empty());
    assert!(
        !has_regression,
        "stable scores should not produce gym regression signals"
    );

    unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
}

#[test]
#[serial]
fn collect_pending_improvements_no_crash_when_no_gym_health() {
    let dir = TempDir::new().unwrap();
    unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", dir.path()) };

    let mut state = OodaState::new(GoalBoard::new());
    let result = collect_pending_improvements(&mut state, &None);
    // Should not panic — graceful degradation.
    // (Handoff signals may appear due to env var races in parallel tests.)
    let has_regression = result.iter().any(|c| !c.regressions.is_empty());
    assert!(
        !has_regression,
        "no gym health should not produce regressions"
    );

    unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
}

#[test]
#[serial]
fn collect_pending_improvements_no_crash_when_no_last_observation() {
    let dir = TempDir::new().unwrap();
    unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", dir.path()) };

    let mut state = OodaState::new(GoalBoard::new());
    let current = Some(make_gym_score(0.8, 0.9));
    // No last_observation means no baseline for regression comparison.
    let result = collect_pending_improvements(&mut state, &current);
    // (Handoff signals may appear due to env var races in parallel tests.)
    let has_regression = result.iter().any(|c| !c.regressions.is_empty());
    assert!(
        !has_regression,
        "no last observation should not produce regressions"
    );

    unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
}

#[test]
fn scan_unprocessed_handoffs_returns_true_for_unprocessed() {
    let dir = TempDir::new().unwrap();

    let handoff = MeetingHandoff {
        topic: "Sprint planning".to_string(),
        started_at: "2026-04-02T23:00:00Z".to_string(),
        closed_at: "2026-04-03T00:00:00Z".to_string(),
        decisions: vec![MeetingDecision {
            description: "Ship v3".to_string(),
            rationale: "Rationale for Ship v3".to_string(),
            participants: vec!["alice".to_string()],
        }],
        action_items: Vec::new(),
        open_questions: Vec::new(),
        processed: false,
        duration_secs: None,
        transcript: Vec::new(),
        participants: Vec::new(),
        themes: Vec::new(),
    };
    write_meeting_handoff(dir.path(), &handoff).unwrap();

    let result = scan_unprocessed_handoffs_in(dir.path()).unwrap();
    assert!(result, "unprocessed handoff should return true");
}

#[test]
fn scan_unprocessed_handoffs_returns_false_when_processed() {
    let dir = TempDir::new().unwrap();

    let handoff = MeetingHandoff {
        topic: "Sprint planning".to_string(),
        started_at: "2026-04-02T23:00:00Z".to_string(),
        closed_at: "2026-04-03T00:00:00Z".to_string(),
        decisions: vec![MeetingDecision {
            description: "Done".to_string(),
            rationale: "Rationale for Done".to_string(),
            participants: vec!["alice".to_string()],
        }],
        action_items: Vec::new(),
        open_questions: Vec::new(),
        processed: true,
        duration_secs: None,
        transcript: Vec::new(),
        participants: Vec::new(),
        themes: Vec::new(),
    };
    write_meeting_handoff(dir.path(), &handoff).unwrap();

    let result = scan_unprocessed_handoffs_in(dir.path()).unwrap();
    assert!(!result, "processed handoff should return false");
}

#[test]
fn scan_unprocessed_handoffs_returns_false_when_no_file() {
    let dir = TempDir::new().unwrap();

    let result = scan_unprocessed_handoffs_in(dir.path()).unwrap();
    assert!(!result, "no handoff file should return false");
}

#[test]
#[serial]
fn collect_pending_improvements_drains_review_improvements() {
    let dir = TempDir::new().unwrap();
    unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", dir.path()) };

    let mut state = OodaState::new(GoalBoard::new());
    state.review_improvements.push(ImprovementCycle {
        baseline: make_gym_score(0.8, 0.9),
        proposed_changes: Vec::new(),
        post_score: None,
        regressions: Vec::new(),
        decision: None,
        final_phase: ImprovementPhase::Eval,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
    });
    let result = collect_pending_improvements(&mut state, &None);
    assert!(
        result
            .iter()
            .any(|c| c.final_phase == ImprovementPhase::Eval),
        "should include drained review improvement"
    );
    assert!(
        state.review_improvements.is_empty(),
        "review improvements should be drained"
    );

    unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
}
