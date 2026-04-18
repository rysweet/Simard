/// Persist cycle report to `<state_root>/cycle_reports/cycle_<N>.json`.
///
/// Writes a structured JSON report so the dashboard can display
/// the full OODA internal reasoning for each cycle.
pub(super) fn persist_cycle_report(
    state_root: &std::path::Path,
    report: &crate::ooda_loop::CycleReport,
) {
    use serde_json::json;

    let dir = state_root.join("cycle_reports");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("cycle_{}.json", report.cycle_number));

    let structured = json!({
        "cycle_number": report.cycle_number,
        "summary": crate::ooda_loop::summarize_cycle_report(report),
        "observation": {
            "goal_count": report.observation.goal_statuses.len(),
            "goals": report.observation.goal_statuses.iter().map(|g| {
                json!({
                    "id": g.id,
                    "description": g.description,
                    "progress": g.progress.to_string(),
                })
            }).collect::<Vec<_>>(),
            "gym_health": report.observation.gym_health.as_ref().map(|g| {
                json!({
                    "overall": g.overall,
                    "pass_rate": g.pass_rate,
                    "scenario_count": g.scenario_count,
                })
            }),
            "memory_stats": {
                "sensory_count": report.observation.memory_stats.sensory_count,
                "working_count": report.observation.memory_stats.working_count,
                "episodic_count": report.observation.memory_stats.episodic_count,
                "semantic_count": report.observation.memory_stats.semantic_count,
                "procedural_count": report.observation.memory_stats.procedural_count,
                "prospective_count": report.observation.memory_stats.prospective_count,
            },
            "environment": {
                "git_status": report.observation.environment.git_status,
                "open_issues": report.observation.environment.open_issues.len(),
                "recent_commits": report.observation.environment.recent_commits.len(),
            },
        },
        "priorities": report.priorities.iter().map(|p| {
            json!({
                "goal_id": p.goal_id,
                "urgency": p.urgency,
                "reason": p.reason,
            })
        }).collect::<Vec<_>>(),
        "planned_actions": report.planned_actions.iter().map(|a| {
            json!({
                "kind": a.kind.to_string(),
                "goal_id": a.goal_id,
                "description": a.description,
            })
        }).collect::<Vec<_>>(),
        "outcomes": report.outcomes.iter().map(|o| {
            json!({
                "action_kind": o.action.kind.to_string(),
                "action_description": o.action.description,
                "success": o.success,
                "detail": o.detail,
            })
        }).collect::<Vec<_>>(),
    });

    let _ = std::fs::write(&path, serde_json::to_string_pretty(&structured).unwrap_or_default());
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
