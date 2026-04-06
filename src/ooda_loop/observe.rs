//! Observe phase: gather goal statuses, environment state, gym health,
//! memory stats, and pending improvement signals.

use crate::error::SimardResult;
use crate::gym_bridge::ScoreDimensions;
use crate::gym_history::{GymSignal, ScoreHistory, generate_signals};
use crate::gym_scoring::{GymSuiteScore, detect_regression};
use crate::meeting_facilitator::load_meeting_handoff;
use crate::memory_cognitive::CognitiveStatistics;
use crate::self_improve::{ImprovementCycle, ImprovementPhase};

use super::{EnvironmentSnapshot, GoalSnapshot, Observation, OodaBridges, OodaState};

/// Gather a snapshot of the local environment (git status, issues, commits).
///
/// Each sub-command degrades honestly: if the tool is unavailable the
/// corresponding field is empty rather than causing a cycle failure.
pub fn gather_environment() -> EnvironmentSnapshot {
    let git_status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let open_issues = std::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--state",
            "open",
            "--limit",
            "20",
            "--json",
            "title",
            "--jq",
            ".[].title",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let recent_commits = std::process::Command::new("git")
        .args(["log", "--oneline", "-10"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    EnvironmentSnapshot {
        git_status,
        open_issues,
        recent_commits,
    }
}

/// Observe: gather goal statuses, environment state, gym health, memory stats,
/// and pending improvement signals from gym regressions and unprocessed handoffs.
/// Sub-system failures produce degraded fields rather than aborting (Pillar 11).
pub fn observe(state: &mut OodaState, bridges: &OodaBridges) -> SimardResult<Observation> {
    let goal_statuses: Vec<GoalSnapshot> = state
        .active_goals
        .active
        .iter()
        .map(GoalSnapshot::from)
        .collect();

    let environment = gather_environment();

    let gym_health = match bridges.gym.run_suite("progressive") {
        Ok(result) => {
            use crate::gym_scoring::suite_score_from_result;
            Some(suite_score_from_result(&result))
        }
        Err(e) => {
            eprintln!("[simard] OODA observe: gym bridge unavailable: {e}");
            None
        }
    };
    let memory_stats = match bridges.memory.get_statistics() {
        Ok(stats) => stats,
        Err(e) => {
            eprintln!("[simard] OODA observe: memory bridge unavailable: {e}");
            CognitiveStatistics::default()
        }
    };

    let pending_improvements = collect_pending_improvements(state, &gym_health);

    Ok(Observation {
        goal_statuses,
        gym_health,
        memory_stats,
        pending_improvements,
        environment,
    })
}

/// Collect pending improvement signals from gym regressions, unprocessed
/// meeting handoffs, and OODA review proposals. Each source degrades
/// independently — a failure in one does not prevent others from contributing.
fn collect_pending_improvements(
    state: &mut OodaState,
    current_gym: &Option<GymSuiteScore>,
) -> Vec<ImprovementCycle> {
    let mut improvements = Vec::new();

    // Signal 1: gym regressions vs last observation.
    if let (Some(current), Some(prev_obs)) = (current_gym, &state.last_observation)
        && let Some(baseline) = &prev_obs.gym_health
    {
        let regressions = detect_regression(current, baseline);
        if !regressions.is_empty() {
            improvements.push(ImprovementCycle {
                baseline: baseline.clone(),
                proposed_changes: Vec::new(),
                post_score: Some(current.clone()),
                regressions,
                decision: None,
                final_phase: ImprovementPhase::Eval,
                weak_dimensions: Vec::new(),
                target_dimension: None,
            });
        }
    }

    // Signal 2: unprocessed meeting handoffs.
    match scan_unprocessed_handoffs() {
        Ok(true) => {
            let baseline = current_gym.clone().unwrap_or_else(|| GymSuiteScore {
                suite_id: "handoff-signal".to_string(),
                overall: 0.0,
                dimensions: ScoreDimensions::default(),
                scenario_count: 0,
                scenarios_passed: 0,
                pass_rate: 0.0,
                recorded_at_unix_ms: None,
            });
            improvements.push(ImprovementCycle {
                baseline,
                proposed_changes: Vec::new(),
                post_score: None,
                regressions: Vec::new(),
                decision: None,
                final_phase: ImprovementPhase::Eval,
                weak_dimensions: Vec::new(),
                target_dimension: None,
            });
        }
        Ok(false) => {}
        Err(e) => {
            eprintln!("[simard] OODA observe: handoff scan failed: {e}");
        }
    }

    // Signal 3: improvement proposals from post-act review analysis.
    if !state.review_improvements.is_empty() {
        eprintln!(
            "[simard] OODA observe: draining {} review improvement(s) from prior cycle",
            state.review_improvements.len()
        );
        improvements.append(&mut state.review_improvements);
    }

    // Signal 4: persistent gym score history (regression / promotion signals).
    let history_path = std::path::Path::new("gym_history.db");
    if history_path.exists()
        && let Ok(history) = ScoreHistory::open(history_path)
    {
        let signals = generate_signals(&history, "progressive").unwrap_or_default();
        for sig in &signals {
            if matches!(sig.signal, GymSignal::Regression { .. }) {
                let baseline = current_gym.clone().unwrap_or_else(|| GymSuiteScore {
                    suite_id: "history-regression".to_string(),
                    overall: 0.0,
                    dimensions: ScoreDimensions::default(),
                    scenario_count: 0,
                    scenarios_passed: 0,
                    pass_rate: 0.0,
                    recorded_at_unix_ms: None,
                });
                eprintln!(
                    "[simard] OODA observe: gym history regression on scenario {}",
                    sig.scenario_id,
                );
                improvements.push(ImprovementCycle {
                    baseline,
                    proposed_changes: Vec::new(),
                    post_score: None,
                    regressions: Vec::new(),
                    decision: None,
                    final_phase: ImprovementPhase::Eval,
                    weak_dimensions: Vec::new(),
                    target_dimension: None,
                });
            }
        }
    }

    improvements
}

/// Check whether there is an unprocessed meeting handoff in the default
/// handoff directory. Returns `true` when one exists.
fn scan_unprocessed_handoffs() -> SimardResult<bool> {
    let dir = crate::meeting_facilitator::default_handoff_dir();
    scan_unprocessed_handoffs_in(&dir)
}

fn scan_unprocessed_handoffs_in(dir: &std::path::Path) -> SimardResult<bool> {
    match load_meeting_handoff(dir) {
        Ok(Some(h)) if !h.processed => Ok(true),
        Ok(_) => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal_curation::GoalBoard;
    use crate::gym_bridge::ScoreDimensions;
    use crate::gym_scoring::GymSuiteScore;
    use crate::meeting_facilitator::{MeetingDecision, MeetingHandoff, write_meeting_handoff};
    use crate::memory_cognitive::CognitiveStatistics;
    use crate::ooda_loop::OodaState;
    use crate::self_improve::ImprovementPhase;
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
            closed_at: "2026-04-03T00:00:00Z".to_string(),
            decisions: vec![MeetingDecision {
                description: "Ship v3".to_string(),
                rationale: "Rationale for Ship v3".to_string(),
                participants: vec!["alice".to_string()],
            }],
            action_items: Vec::new(),
            open_questions: Vec::new(),
            processed: false,
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
            closed_at: "2026-04-03T00:00:00Z".to_string(),
            decisions: vec![MeetingDecision {
                description: "Done".to_string(),
                rationale: "Rationale for Done".to_string(),
                participants: vec!["alice".to_string()],
            }],
            action_items: Vec::new(),
            open_questions: Vec::new(),
            processed: true,
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
}
