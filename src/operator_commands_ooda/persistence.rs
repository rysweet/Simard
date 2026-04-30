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
            let mut entry = json!({
                "action_kind": o.action.kind.to_string(),
                "action_description": o.action.description,
                "success": o.success,
                "detail": o.detail,
            });
            if let Some(spawn) = extract_spawn_engineer_outcome(&o.detail, o.success)
                && let Some(map) = entry.as_object_mut() {
                    map.insert("spawn_engineer".to_string(), spawn);
                }
            entry
        }).collect::<Vec<_>>(),
        // BrainJudgmentRecord is a flat data carrier whose external JSON
        // shape is governed entirely by its `Serialize` derive (including
        // `#[serde(skip_serializing_if = "String::is_empty")]` on
        // `prompt_version`). Defer to `serde_json::to_value` so the auto-
        // derive is the single source of truth — adding a new field to
        // the struct then forgetting to mirror it here is exactly the
        // divergence-class bug PR #1480 had to repair.
        //
        // The other sub-structs (observation, outcomes, …) are NOT pure
        // 1:1 mappings — they intentionally project / summarise (counts
        // vs full lists, derived `summary`, `kind.to_string()`-style
        // labels, the `spawn_engineer` enrichment block) and so stay
        // hand-rolled by design.
        //
        // `BrainJudgmentRecord` only carries primitives + a small enum
        // and so cannot fail to serialise; the `unwrap_or` keeps the
        // best-effort write contract of this function.
        "brain_judgments": report.brain_judgments.iter().map(|j| {
            serde_json::to_value(j).unwrap_or(serde_json::Value::Null)
        }).collect::<Vec<_>>(),
    });

    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&structured).unwrap_or_default(),
    );
}

/// Extract structured spawn_engineer outcome fields from an action's free-form
/// detail string.
///
/// The OODA goal-action dispatcher emits human-readable detail messages such
/// as `"spawn_engineer dispatched: agent='engineer-g1-1700', task='fix bug'
/// (goal 'g1', pid=1234)"`. Issue #946 surfaces these on the dashboard
/// Thinking tab, so we re-parse those messages back into structured fields:
///
/// - `subordinate_agent`: the agent name (used to build the agent-log link)
/// - `task_summary`: the LLM-supplied task text (truncated upstream)
/// - `last_action`: short verb describing what happened (`dispatched`,
///   `skipped`, `denied`, `failed`)
/// - `status`: live-status indicator (`live`, `skipped`, `denied`, `failed`)
///
/// Returns `None` when the detail does not reference `spawn_engineer`, so
/// callers can attach the structured block only when meaningful.
pub(crate) fn extract_spawn_engineer_outcome(
    detail: &str,
    success: bool,
) -> Option<serde_json::Value> {
    if !detail.contains("spawn_engineer") {
        return None;
    }

    let (last_action, status) = if detail.contains("spawn_engineer dispatched") {
        ("dispatched", if success { "live" } else { "failed" })
    } else if detail.contains("spawn_engineer skipped") {
        ("skipped", "skipped")
    } else if detail.contains("spawn_engineer denied") {
        ("denied", "denied")
    } else if detail.contains("spawn_engineer failed") {
        ("failed", "failed")
    } else if detail.contains("spawn_engineer requested") {
        ("requested", if success { "pending" } else { "failed" })
    } else {
        ("unknown", if success { "ok" } else { "failed" })
    };

    let subordinate_agent = extract_quoted_after(detail, "agent=")
        .or_else(|| extract_quoted_after(detail, "subordinate "));
    let task_summary = extract_quoted_after(detail, "task=");
    let goal_id = extract_quoted_after(detail, "goal ");

    Some(serde_json::json!({
        "subordinate_agent": subordinate_agent,
        "task_summary": task_summary,
        "goal_id": goal_id,
        "last_action": last_action,
        "status": status,
    }))
}

/// Pull the contents of the next single-quoted string that follows `prefix`
/// in `text`. Returns `None` if either the prefix or the closing quote is
/// missing. Used by [`extract_spawn_engineer_outcome`] to recover structured
/// data from the human-readable outcome detail strings.
fn extract_quoted_after(text: &str, prefix: &str) -> Option<String> {
    let idx = text.find(prefix)?;
    let after = &text[idx + prefix.len()..];
    let after = after.strip_prefix('\'').unwrap_or(after);
    let end = after.find('\'')?;
    Some(after[..end].to_string())
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
                eval_watchdog: None,
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
            brain_judgments: Vec::new(),
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

    #[test]
    fn extract_spawn_engineer_dispatched_detail() {
        let detail = "spawn_engineer dispatched: agent='engineer-g1-1700', task='fix the auth bug' (goal 'g1', pid=1234)";
        let v = extract_spawn_engineer_outcome(detail, true).expect("should detect");
        assert_eq!(v["last_action"], "dispatched");
        assert_eq!(v["status"], "live");
        assert_eq!(v["subordinate_agent"], "engineer-g1-1700");
        assert_eq!(v["task_summary"], "fix the auth bug");
        assert_eq!(v["goal_id"], "g1");
    }

    #[test]
    fn extract_spawn_engineer_skipped_detail() {
        let detail =
            "spawn_engineer skipped: goal 'g1' already assigned to subordinate 'engineer-g1-old'";
        let v = extract_spawn_engineer_outcome(detail, true).expect("should detect");
        assert_eq!(v["last_action"], "skipped");
        assert_eq!(v["status"], "skipped");
        assert_eq!(v["goal_id"], "g1");
    }

    #[test]
    fn extract_spawn_engineer_denied_detail() {
        let detail =
            "spawn_engineer denied for goal 'g2': subordinate depth 2 >= configured limit 2";
        let v = extract_spawn_engineer_outcome(detail, false).expect("should detect");
        assert_eq!(v["last_action"], "denied");
        assert_eq!(v["status"], "denied");
        assert_eq!(v["goal_id"], "g2");
    }

    #[test]
    fn extract_spawn_engineer_returns_none_for_unrelated_detail() {
        assert!(extract_spawn_engineer_outcome("consolidated 20 episodes", true).is_none());
    }

    #[test]
    fn persist_cycle_report_includes_spawn_engineer_block() {
        let dir = tempfile::tempdir().unwrap();
        let mut report = minimal_report(7);
        report.outcomes.push(ActionOutcome {
            action: PlannedAction {
                kind: ActionKind::AdvanceGoal,
                goal_id: Some("g1".to_string()),
                description: "advance g1".to_string(),
            },
            success: true,
            detail:
                "spawn_engineer dispatched: agent='engineer-g1-9', task='do things' (goal 'g1', pid=42)"
                    .to_string(),
        });
        persist_cycle_report(dir.path(), &report);
        let content =
            std::fs::read_to_string(dir.path().join("cycle_reports/cycle_7.json")).unwrap();
        assert!(content.contains("\"spawn_engineer\""));
        assert!(content.contains("engineer-g1-9"));
        assert!(content.contains("\"status\": \"live\""));
    }

    #[test]
    fn persist_cycle_report_serialises_brain_judgments_when_present() {
        use crate::ooda_brain::{BrainJudgmentRecord, BrainPhase};

        let dir = tempfile::tempdir().unwrap();
        let mut report = minimal_report(11);
        report.brain_judgments.push(BrainJudgmentRecord {
            phase: BrainPhase::Decide,
            context_summary: "goal_id=ship-v1 urgency=0.900".to_string(),
            decision: "advance_goal".to_string(),
            rationale: "high priority".to_string(),
            confidence: 1.0,
            fallback: false,
            prompt_version: "abc123def456".to_string(),
        });
        report.brain_judgments.push(BrainJudgmentRecord {
            phase: BrainPhase::Orient,
            context_summary: "goal_id=g1 base_urgency=0.600 failures=2".to_string(),
            decision: "demote".to_string(),
            rationale: "two failures".to_string(),
            confidence: 0.8,
            fallback: true,
            prompt_version: String::new(),
        });
        persist_cycle_report(dir.path(), &report);
        let content =
            std::fs::read_to_string(dir.path().join("cycle_reports/cycle_11.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let arr = parsed["brain_judgments"].as_array().expect("array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["phase"], "decide");
        assert_eq!(arr[0]["decision"], "advance_goal");
        assert_eq!(arr[0]["fallback"], false);
        // Regression: PR #1476 added `prompt_version` to BrainJudgmentRecord
        // but the manual json! mapping in persist_cycle_report omitted it,
        // silently dropping the field on every persisted cycle report.
        assert_eq!(
            arr[0]["prompt_version"], "abc123def456",
            "prompt_version must round-trip through persist_cycle_report's manual json! mapping"
        );
        assert_eq!(arr[1]["phase"], "orient");
        assert_eq!(arr[1]["fallback"], true);
        // Empty prompt_version (deterministic-fallback judgments) stays
        // omitted, matching the struct-level skip_serializing_if behaviour.
        assert!(
            arr[1].get("prompt_version").is_none(),
            "empty prompt_version must be omitted (got {:?})",
            arr[1].get("prompt_version")
        );
    }

    #[test]
    fn persist_cycle_report_emits_empty_brain_judgments_array_when_none() {
        let dir = tempfile::tempdir().unwrap();
        // minimal_report(...) has brain_judgments = vec![] by default.
        let report = minimal_report(12);
        persist_cycle_report(dir.path(), &report);
        let content =
            std::fs::read_to_string(dir.path().join("cycle_reports/cycle_12.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        // Persistence writer always emits the field as [] for forensic
        // consistency on the dashboard side, even though the Rust struct's
        // `Vec` skips serialisation when empty.
        assert_eq!(parsed["brain_judgments"], serde_json::json!([]));
    }
}
