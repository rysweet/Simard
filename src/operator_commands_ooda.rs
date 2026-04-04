use std::path::PathBuf;

use crate::bridge_launcher::{
    cognitive_memory_db_path, find_python_dir, launch_gym_bridge, launch_knowledge_bridge,
    launch_memory_bridge,
};
use crate::goal_curation::load_goal_board;
use crate::identity::OperatingMode;
use crate::ooda_loop::{
    OodaBridges, OodaConfig, OodaState, run_ooda_cycle, summarize_cycle_report,
};
use crate::session_builder::SessionBuilder;

/// Run one or more OODA cycles as a daemon-style loop.
///
/// Launches all bridges, opens a RustyClawd session via [`SessionBuilder`]
/// for real autonomous work, loads the goal board from cognitive memory,
/// and runs OODA cycles until `max_cycles` is reached (0 = infinite).
///
/// If no `ANTHROPIC_API_KEY` is set, the session will be `None` and the
/// daemon degrades honestly to bridge-only dispatch (Pillar 11).
pub fn run_ooda_daemon(
    max_cycles: u32,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = state_root_override.unwrap_or_else(|| {
        PathBuf::from(
            std::env::var("SIMARD_STATE_ROOT").unwrap_or_else(|_| "/tmp/simard-ooda".to_string()),
        )
    });

    std::fs::create_dir_all(&state_root)?;

    let agent_name =
        std::env::var("SIMARD_AGENT_NAME").unwrap_or_else(|_| "simard-ooda".to_string());

    let python_dir = find_python_dir()?;
    let db_path = cognitive_memory_db_path(&state_root);

    let memory = launch_memory_bridge(&agent_name, &db_path, &python_dir)?;
    let knowledge = launch_knowledge_bridge(&python_dir)?;
    let gym = launch_gym_bridge(&python_dir)?;

    // Try to open a RustyClawd session for real autonomous work.
    let session = SessionBuilder::new(OperatingMode::Orchestrator)
        .node_id("ooda-daemon")
        .address("ooda-daemon://local")
        .adapter_tag("ooda-rustyclawd")
        .open();

    if session.is_some() {
        eprintln!("[simard] OODA daemon: RustyClawd session opened for autonomous work");
    } else {
        eprintln!("[simard] OODA daemon: no API key — running in bridge-only mode");
    }

    let mut bridges = OodaBridges {
        memory,
        knowledge,
        gym,
        session,
    };

    let board = load_goal_board(&bridges.memory).unwrap_or_default();
    let mut state = OodaState::new(board);
    let config = OodaConfig::default();

    let interval_secs: u64 = std::env::var("SIMARD_OODA_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);

    eprintln!("[simard] OODA daemon: cycle interval = {interval_secs}s");

    let mut cycles_run = 0u32;

    loop {
        if max_cycles > 0 && cycles_run >= max_cycles {
            eprintln!("[simard] OODA daemon: completed {cycles_run} cycle(s), exiting");
            break;
        }

        match run_ooda_cycle(&mut state, &mut bridges, &config) {
            Ok(report) => {
                let summary = summarize_cycle_report(&report);
                eprintln!("[simard] {summary}");
                // Persist the cycle report to filesystem for auditability.
                persist_cycle_report(&state_root, &report);
                // Persist the cycle summary to cognitive memory as an episode.
                persist_cycle_to_memory(&bridges, &report);
            }
            Err(e) => {
                eprintln!("[simard] OODA cycle error: {e}");
            }
        }

        cycles_run += 1;

        // Skip the inter-cycle sleep if this was the last requested cycle.
        if max_cycles > 0 && cycles_run >= max_cycles {
            continue;
        }

        // Sleep between cycles to avoid busy-looping. Configurable via
        // SIMARD_OODA_INTERVAL_SECS; default is 300 seconds.
        std::thread::sleep(std::time::Duration::from_secs(interval_secs));
    }

    // Close the session cleanly if it was opened.
    if let Some(ref mut session) = bridges.session {
        let _ = session.close();
    }

    Ok(())
}

/// Persist cycle report to `<state_root>/cycle_reports/cycle_<N>.json`.
fn persist_cycle_report(state_root: &std::path::Path, report: &crate::ooda_loop::CycleReport) {
    let dir = state_root.join("cycle_reports");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("cycle_{}.json", report.cycle_number));
    let summary = crate::ooda_loop::summarize_cycle_report(report);
    // Write a lightweight summary rather than serializing the full report.
    let _ = std::fs::write(&path, summary);
}

/// Persist cycle results to cognitive memory as an episodic record.
///
/// Records the cycle summary and outcome counts so that future OODA cycles
/// and goal curation sessions can recall what happened. Best-effort: failures
/// are logged but do not abort the daemon.
fn persist_cycle_to_memory(
    bridges: &crate::ooda_loop::OodaBridges,
    report: &crate::ooda_loop::CycleReport,
) {
    use serde_json::json;

    let summary = crate::ooda_loop::summarize_cycle_report(report);
    let succeeded = report.outcomes.iter().filter(|o| o.success).count();
    let failed = report.outcomes.len() - succeeded;

    let metadata = json!({
        "cycle_number": report.cycle_number,
        "actions_succeeded": succeeded,
        "actions_failed": failed,
        "goal_count": report.observation.goal_statuses.len(),
        "open_issues": report.observation.environment.open_issues.len(),
    });

    if let Err(e) = bridges
        .memory
        .store_episode(&summary, "ooda-daemon", Some(&metadata))
    {
        eprintln!("[simard] OODA persist: failed to store episode: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooda_loop::{
        ActionKind, ActionOutcome, CycleReport, EnvironmentSnapshot, GoalSnapshot, Observation,
        PlannedAction, Priority,
    };
    use crate::{CognitiveStatistics, GoalProgress};

    fn make_minimal_observation() -> Observation {
        Observation {
            goal_statuses: vec![],
            gym_health: None,
            memory_stats: CognitiveStatistics::default(),
            pending_improvements: vec![],
            environment: EnvironmentSnapshot::default(),
        }
    }

    fn make_test_report(cycle_number: u32) -> CycleReport {
        CycleReport {
            cycle_number,
            observation: make_minimal_observation(),
            priorities: vec![],
            planned_actions: vec![],
            outcomes: vec![],
        }
    }

    fn make_report_with_goals_and_outcomes() -> CycleReport {
        CycleReport {
            cycle_number: 7,
            observation: Observation {
                goal_statuses: vec![
                    GoalSnapshot {
                        id: "goal-1".to_string(),
                        description: "First goal".to_string(),
                        progress: GoalProgress::InProgress { percent: 50 },
                    },
                    GoalSnapshot {
                        id: "goal-2".to_string(),
                        description: "Second goal".to_string(),
                        progress: GoalProgress::NotStarted,
                    },
                ],
                gym_health: None,
                memory_stats: CognitiveStatistics::default(),
                pending_improvements: vec![],
                environment: EnvironmentSnapshot {
                    git_status: "clean".to_string(),
                    open_issues: vec!["issue-1".to_string()],
                    recent_commits: vec![],
                },
            },
            priorities: vec![Priority {
                goal_id: "goal-1".to_string(),
                urgency: 0.8,
                reason: "High priority".to_string(),
            }],
            planned_actions: vec![PlannedAction {
                kind: ActionKind::AdvanceGoal,
                goal_id: Some("goal-1".to_string()),
                description: "Work on goal 1".to_string(),
            }],
            outcomes: vec![
                ActionOutcome {
                    action: PlannedAction {
                        kind: ActionKind::AdvanceGoal,
                        goal_id: Some("goal-1".to_string()),
                        description: "Work on goal 1".to_string(),
                    },
                    success: true,
                    detail: "Completed".to_string(),
                },
                ActionOutcome {
                    action: PlannedAction {
                        kind: ActionKind::RunGymEval,
                        goal_id: None,
                        description: "Run gym eval".to_string(),
                    },
                    success: false,
                    detail: "Failed".to_string(),
                },
            ],
        }
    }

    // --- OodaConfig defaults ---

    #[test]
    fn ooda_config_default_values() {
        let config = OodaConfig::default();
        assert_eq!(config.max_concurrent_actions, 3);
        assert!(
            (config.improvement_threshold - 0.02).abs() < f64::EPSILON,
            "improvement_threshold should be 0.02"
        );
        assert_eq!(config.gym_suite_id, "progressive");
    }

    // --- persist_cycle_report ---

    #[test]
    fn persist_cycle_report_creates_directory_and_file() {
        let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!("ooda-persist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);

        let report = make_test_report(42);
        persist_cycle_report(&scratch, &report);

        let path = scratch.join("cycle_reports").join("cycle_42.json");
        assert!(path.exists(), "cycle report file should be created");

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("42"),
            "content should reference cycle number"
        );

        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn persist_cycle_report_uses_cycle_number_in_filename() {
        let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!("ooda-filename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);

        persist_cycle_report(&scratch, &make_test_report(99));
        let path = scratch.join("cycle_reports").join("cycle_99.json");
        assert!(path.exists());

        persist_cycle_report(&scratch, &make_test_report(100));
        let path2 = scratch.join("cycle_reports").join("cycle_100.json");
        assert!(path2.exists());

        let _ = std::fs::remove_dir_all(&scratch);
    }

    // --- summarize_cycle_report ---

    #[test]
    fn summarize_empty_report() {
        let report = make_test_report(1);
        let summary = crate::ooda_loop::summarize_cycle_report(&report);
        assert!(
            summary.contains("#1"),
            "summary should contain cycle number: {summary}"
        );
    }

    #[test]
    fn summarize_report_with_outcomes() {
        let report = make_report_with_goals_and_outcomes();
        let summary = crate::ooda_loop::summarize_cycle_report(&report);
        assert!(
            summary.contains("#7"),
            "summary should contain cycle number: {summary}"
        );
        assert!(
            summary.contains("1/2"),
            "summary should contain success ratio: {summary}"
        );
    }

    #[test]
    fn summarize_report_mentions_goals() {
        let report = make_report_with_goals_and_outcomes();
        let summary = crate::ooda_loop::summarize_cycle_report(&report);
        assert!(
            summary.contains("goals=2"),
            "summary should mention goal count: {summary}"
        );
    }

    #[test]
    fn summarize_report_mentions_issues() {
        let report = make_report_with_goals_and_outcomes();
        let summary = crate::ooda_loop::summarize_cycle_report(&report);
        assert!(
            summary.contains("issues=1"),
            "summary should mention issue count: {summary}"
        );
    }
}
