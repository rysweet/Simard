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
pub(super) fn collect_pending_improvements(
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

pub(super) fn scan_unprocessed_handoffs_in(dir: &std::path::Path) -> SimardResult<bool> {
    match load_meeting_handoff(dir) {
        Ok(Some(h)) if !h.processed => Ok(true),
        Ok(_) => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gym_bridge::ScoreDimensions;
    use serial_test::serial;

    fn make_score(overall: f64) -> GymSuiteScore {
        GymSuiteScore {
            suite_id: "test".into(),
            overall,
            dimensions: ScoreDimensions {
                factual_accuracy: overall,
                specificity: overall * 0.9,
                temporal_awareness: overall * 0.8,
                source_attribution: overall * 0.7,
                confidence_calibration: overall * 0.85,
            },
            scenario_count: 4,
            scenarios_passed: 4,
            pass_rate: 1.0,
            recorded_at_unix_ms: None,
        }
    }

    // ---- scan_unprocessed_handoffs_in ----

    #[test]
    fn scan_unprocessed_handoffs_in_nonexistent_dir() {
        // Non-existent directory should return false or an error depending
        // on load_meeting_handoff behavior — either is acceptable.
        let result = scan_unprocessed_handoffs_in(std::path::Path::new("/nonexistent/handoff/dir"));
        match result {
            Ok(false) => {} // no handoff found
            Ok(true) => panic!("should not find handoff in nonexistent dir"),
            Err(_) => {} // error is also acceptable
        }
    }

    // ---- gather_environment ----

    #[test]
    fn gather_environment_returns_snapshot() {
        let snap = gather_environment();
        // git status should succeed in this repo
        // Just verify the snapshot is constructed without panic
        let _ = snap.git_status.len();
        let _ = snap.recent_commits.len();
    }

    // ---- collect_pending_improvements ----

    #[test]
    #[serial]
    fn collect_pending_improvements_no_signals() {
        // Point handoff dir to a nonexistent path so scan_unprocessed_handoffs
        // doesn't pick up stale files from other tests or CI artifacts.
        unsafe {
            std::env::set_var("SIMARD_HANDOFF_DIR", "/tmp/nonexistent-handoff-dir-test");
        }
        let mut state = OodaState::new(crate::goal_curation::GoalBoard::new());
        let improvements = collect_pending_improvements(&mut state, &None);
        unsafe {
            std::env::remove_var("SIMARD_HANDOFF_DIR");
        }
        assert!(improvements.is_empty());
    }

    #[test]
    #[serial]
    fn collect_pending_improvements_drains_review_improvements() {
        unsafe {
            std::env::set_var("SIMARD_HANDOFF_DIR", "/tmp/nonexistent-handoff-dir-test");
        }
        let baseline = make_score(0.5);
        let cycle = ImprovementCycle {
            baseline: baseline.clone(),
            proposed_changes: Vec::new(),
            post_score: None,
            regressions: Vec::new(),
            decision: None,
            final_phase: ImprovementPhase::Eval,
            weak_dimensions: Vec::new(),
            target_dimension: None,
        };
        let mut state = OodaState::new(crate::goal_curation::GoalBoard::new());
        state.review_improvements = vec![cycle];
        let improvements = collect_pending_improvements(&mut state, &None);
        unsafe {
            std::env::remove_var("SIMARD_HANDOFF_DIR");
        }
        assert_eq!(improvements.len(), 1);
        assert!(state.review_improvements.is_empty());
    }

    #[test]
    #[serial]
    fn collect_pending_improvements_regression_signal() {
        unsafe {
            std::env::set_var("SIMARD_HANDOFF_DIR", "/tmp/nonexistent-handoff-dir-test");
        }
        let baseline = make_score(0.8);
        let current = make_score(0.5);

        let prev_observation = Observation {
            goal_statuses: Vec::new(),
            gym_health: Some(baseline),
            memory_stats: CognitiveStatistics::default(),
            pending_improvements: Vec::new(),
            environment: EnvironmentSnapshot {
                git_status: String::new(),
                open_issues: Vec::new(),
                recent_commits: Vec::new(),
            },
        };

        let mut state = OodaState::new(crate::goal_curation::GoalBoard::new());
        state.last_observation = Some(prev_observation);
        let improvements = collect_pending_improvements(&mut state, &Some(current));
        unsafe {
            std::env::remove_var("SIMARD_HANDOFF_DIR");
        }
        // Should detect regression since 0.5 < 0.8
        assert!(!improvements.is_empty());
    }

    #[test]
    #[serial]
    fn collect_pending_improvements_no_regression_when_scores_match() {
        unsafe {
            std::env::set_var("SIMARD_HANDOFF_DIR", "/tmp/nonexistent-handoff-dir-test");
        }
        let score = make_score(0.8);

        let prev_observation = Observation {
            goal_statuses: Vec::new(),
            gym_health: Some(score.clone()),
            memory_stats: CognitiveStatistics::default(),
            pending_improvements: Vec::new(),
            environment: EnvironmentSnapshot {
                git_status: String::new(),
                open_issues: Vec::new(),
                recent_commits: Vec::new(),
            },
        };

        let mut state = OodaState::new(crate::goal_curation::GoalBoard::new());
        state.last_observation = Some(prev_observation);
        let improvements = collect_pending_improvements(&mut state, &Some(score));
        unsafe {
            std::env::remove_var("SIMARD_HANDOFF_DIR");
        }
        // No regression when scores are the same
        assert_eq!(
            improvements
                .iter()
                .filter(|c| !c.regressions.is_empty())
                .count(),
            0
        );
    }
}
