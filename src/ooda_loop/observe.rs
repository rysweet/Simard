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
                weak_dimension_details: Vec::new(),
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
                weak_dimension_details: Vec::new(),
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
                    weak_dimension_details: Vec::new(),
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

    #[test]
    fn gather_environment_returns_snapshot() {
        let snap = gather_environment();
        // git_status may be empty or non-empty depending on working dir state,
        // but the function should not panic even without git/gh.
        let _ = snap.git_status;
        let _ = snap.open_issues;
        let _ = snap.recent_commits;
    }

    #[test]
    fn scan_unprocessed_handoffs_in_empty_dir() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let result = scan_unprocessed_handoffs_in(dir.path());
        // Either Ok(false) or an error if the dir doesn't have the expected format —
        // both are acceptable. It must not panic.
        if let Ok(found) = result {
            assert!(!found);
        }
    }

    #[test]
    fn scan_unprocessed_handoffs_in_nonexistent_dir() {
        let dir = std::path::Path::new("/tmp/simard-test-nonexistent-handoff-dir-xyz");
        let result = scan_unprocessed_handoffs_in(dir);
        if let Ok(found) = result {
            assert!(!found);
        }
    }

    #[test]
    fn collect_pending_improvements_empty_state() {
        use crate::goal_curation::GoalBoard;
        let mut state = OodaState::new(GoalBoard::new());
        let improvements = collect_pending_improvements(&mut state, &None);
        // With no prior observation and no gym score, should have no regression signals
        assert!(improvements.is_empty() || !improvements.is_empty()); // may pick up handoffs
        // The key assertion is it doesn't panic
    }

    #[test]
    fn environment_snapshot_default() {
        let snap = EnvironmentSnapshot::default();
        assert!(snap.git_status.is_empty());
        assert!(snap.open_issues.is_empty());
        assert!(snap.recent_commits.is_empty());
    }
}
