/// Persist cycle report to `<state_root>/cycle_reports/cycle_<N>.json`.
pub(super) fn persist_cycle_report(
    state_root: &std::path::Path,
    report: &crate::ooda_loop::CycleReport,
) {
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
pub(super) fn persist_cycle_to_memory(
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
    use crate::goal_curation::GoalProgress;
    use crate::memory_cognitive::CognitiveStatistics;
    use crate::ooda_loop::{
        ActionKind, ActionOutcome, CycleReport, EnvironmentSnapshot, GoalSnapshot, Observation,
        PlannedAction, Priority,
    };

    fn minimal_report(cycle: u32) -> CycleReport {
        CycleReport {
            cycle_number: cycle,
            observation: Observation {
                goal_statuses: vec![GoalSnapshot {
                    id: "g1".to_string(),
                    description: "test goal".to_string(),
                    progress: GoalProgress::NotStarted,
                }],
                gym_health: None,
                memory_stats: CognitiveStatistics {
                    sensory_count: 0,
                    working_count: 0,
                    episodic_count: 0,
                    semantic_count: 0,
                    procedural_count: 0,
                    prospective_count: 0,
                },
                pending_improvements: vec![],
                environment: EnvironmentSnapshot::default(),
            },
            priorities: vec![Priority {
                goal_id: "g1".to_string(),
                urgency: 0.8,
                reason: "important".to_string(),
            }],
            planned_actions: vec![PlannedAction {
                kind: ActionKind::AdvanceGoal,
                goal_id: Some("g1".to_string()),
                description: "advance".to_string(),
            }],
            outcomes: vec![ActionOutcome {
                action: PlannedAction {
                    kind: ActionKind::AdvanceGoal,
                    goal_id: Some("g1".to_string()),
                    description: "advance".to_string(),
                },
                success: true,
                detail: "done".to_string(),
            }],
        }
    }

    #[test]
    fn persist_cycle_report_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let report = minimal_report(42);
        persist_cycle_report(dir.path(), &report);
        let path = dir.path().join("cycle_reports").join("cycle_42.json");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.is_empty());
    }

    #[test]
    fn persist_cycle_report_creates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let report = minimal_report(1);
        persist_cycle_report(dir.path(), &report);
        assert!(dir.path().join("cycle_reports").is_dir());
    }

    #[test]
    fn persist_cycle_report_multiple_cycles() {
        let dir = tempfile::tempdir().unwrap();
        persist_cycle_report(dir.path(), &minimal_report(1));
        persist_cycle_report(dir.path(), &minimal_report(2));
        assert!(dir.path().join("cycle_reports/cycle_1.json").exists());
        assert!(dir.path().join("cycle_reports/cycle_2.json").exists());
    }

    #[test]
    fn persist_cycle_report_overwrites_same_cycle() {
        let dir = tempfile::tempdir().unwrap();
        persist_cycle_report(dir.path(), &minimal_report(1));
        let first = std::fs::read_to_string(dir.path().join("cycle_reports/cycle_1.json")).unwrap();
        persist_cycle_report(dir.path(), &minimal_report(1));
        let second =
            std::fs::read_to_string(dir.path().join("cycle_reports/cycle_1.json")).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn persist_cycle_report_with_no_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let mut report = minimal_report(10);
        report.outcomes.clear();
        persist_cycle_report(dir.path(), &report);
        let path = dir.path().join("cycle_reports/cycle_10.json");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("10"));
    }

    #[test]
    fn persist_cycle_report_with_mixed_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let mut report = minimal_report(5);
        report.outcomes.push(ActionOutcome {
            action: PlannedAction {
                kind: ActionKind::ConsolidateMemory,
                goal_id: None,
                description: "consolidate".to_string(),
            },
            success: false,
            detail: "failed".to_string(),
        });
        persist_cycle_report(dir.path(), &report);
        let path = dir.path().join("cycle_reports/cycle_5.json");
        assert!(path.exists());
    }

    #[test]
    fn persist_cycle_report_nonexistent_deep_path() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("c");
        let report = minimal_report(99);
        persist_cycle_report(&nested, &report);
        assert!(nested.join("cycle_reports/cycle_99.json").exists());
    }
}
