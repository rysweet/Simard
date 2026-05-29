use axum::Json;
use serde_json::{Value, json};

use super::dashboard_goal_board_snapshot;
use super::routes::resolve_state_root;
use super::subagent::{count_json_records, file_metrics};
use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};

// ---------------------------------------------------------------------------
// Memory metrics panel
// ---------------------------------------------------------------------------

pub(crate) async fn memory_metrics() -> Json<Value> {
    let state_root = resolve_state_root();

    let memory_path = state_root.join("memory_records.json");
    let evidence_path = state_root.join("evidence_records.json");
    let handoff_path = state_root.join("latest_handoff.json");

    let memory_info = file_metrics(&memory_path);
    let evidence_info = file_metrics(&evidence_path);
    let handoff_info = file_metrics(&handoff_path);

    let fact_count = count_json_records(&memory_path);
    let evidence_count = count_json_records(&evidence_path);

    // Goal records now live in cognitive memory (issue #1590); render a
    // metadata-only panel so the dashboard's "Goal Records" tile keeps
    // working without any disk file.
    let goal_board = dashboard_goal_board_snapshot(&state_root).ok();
    let goal_count = goal_board
        .as_ref()
        .map(|b| (b.active.len() + b.backlog.len()) as u64)
        .unwrap_or(0);

    // Query NativeCognitiveMemory (LadybugDB) for live statistics (#419).
    // Capture the error so the dashboard can show *why* data is missing
    // instead of silently returning zeros.
    let native_result =
        NativeCognitiveMemory::open_read_only(&state_root).and_then(|mem| mem.get_statistics());
    let native_error = native_result.as_ref().err().map(|e| e.to_string());
    let native_stats = native_result.ok();

    let last_consolidation = [&memory_path, &evidence_path]
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .filter_map(|m| m.modified().ok())
        .max()
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.to_rfc3339()
        });

    // Use LadybugDB counts when available; JSON file counts are the legacy source.
    let total = native_stats
        .as_ref()
        .map(|s| s.total())
        .unwrap_or(fact_count + evidence_count + goal_count);

    let db_path = state_root.join("cognitive_memory.ladybug");

    Json(json!({
        "state_root": state_root.to_string_lossy(),
        "memory_records": {
            "path": memory_path.to_string_lossy().to_string(),
            "count": fact_count,
            "size_bytes": memory_info.0,
            "modified": memory_info.1,
        },
        "evidence_records": {
            "path": evidence_path.to_string_lossy().to_string(),
            "count": evidence_count,
            "size_bytes": evidence_info.0,
            "modified": evidence_info.1,
        },
        "goal_records": {
            "source": "cognitive-memory:goal-board:snapshot",
            "count": goal_count,
        },
        "handoff": {
            "path": handoff_path.to_string_lossy().to_string(),
            "size_bytes": handoff_info.0,
            "modified": handoff_info.1,
        },
        "native_memory": native_stats.as_ref().map(|s| json!({
            "sensory": s.sensory_count,
            "working": s.working_count,
            "episodic": s.episodic_count,
            "semantic": s.semantic_count,
            "procedural": s.procedural_count,
            "prospective": s.prospective_count,
            "total": s.total(),
        })),
        "native_memory_error": native_error,
        "native_memory_db_path": db_path.to_string_lossy(),
        "native_memory_db_exists": db_path.exists(),
        "total_facts": total,
        "last_consolidation": last_consolidation,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

pub(crate) async fn ooda_thinking() -> Json<Value> {
    let state_root = resolve_state_root();
    let dir = state_root.join("cycle_reports");
    let mut reports = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        paths.sort_by(|a, b| {
            let num = |p: &std::fs::DirEntry| -> u32 {
                p.file_name()
                    .to_str()
                    .unwrap_or("")
                    .strip_prefix("cycle_")
                    .unwrap_or("")
                    .strip_suffix(".json")
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(0)
            };
            num(b).cmp(&num(a))
        });

        for entry in paths.into_iter().take(20) {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                if let Ok(val) = serde_json::from_str::<Value>(&content) {
                    reports.push(val);
                } else {
                    // Legacy one-line summary
                    let cycle_num = entry
                        .file_name()
                        .to_str()
                        .unwrap_or("")
                        .strip_prefix("cycle_")
                        .unwrap_or("")
                        .strip_suffix(".json")
                        .unwrap_or("")
                        .parse::<u32>()
                        .unwrap_or(0);
                    reports.push(json!({
                        "cycle_number": cycle_num,
                        "summary": content.trim(),
                        "legacy": true,
                    }));
                }
            }
        }
    }

    Json(json!({ "reports": reports }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ooda_thinking with temp cycle reports ----------------------------

    #[test]
    fn ooda_thinking_reads_cycle_reports_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let cycle_dir = dir.path().join("cycle_reports");
        std::fs::create_dir_all(&cycle_dir).unwrap();

        let report = json!({
            "cycle_number": 1,
            "summary": "test cycle",
            "observation": {"goal_count": 2}
        });
        std::fs::write(
            cycle_dir.join("cycle_1.json"),
            serde_json::to_string(&report).unwrap(),
        )
        .unwrap();

        // Verify the cycle report is readable
        let content = std::fs::read_to_string(cycle_dir.join("cycle_1.json")).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["cycle_number"], 1);
        assert_eq!(parsed["summary"], "test cycle");
    }

    #[test]
    fn ooda_thinking_handles_missing_cycle_dir() {
        let dir = tempfile::tempdir().unwrap();
        // No cycle_reports directory — should not panic
        let cycle_dir = dir.path().join("cycle_reports");
        assert!(!cycle_dir.exists());
    }

    #[test]
    fn ooda_thinking_sorts_reports_by_cycle_number_descending() {
        let dir = tempfile::tempdir().unwrap();
        let cycle_dir = dir.path().join("cycle_reports");
        std::fs::create_dir_all(&cycle_dir).unwrap();

        for i in [3, 1, 2] {
            std::fs::write(
                cycle_dir.join(format!("cycle_{i}.json")),
                format!(r#"{{"cycle_number":{i}}}"#),
            )
            .unwrap();
        }

        let mut paths: Vec<_> = std::fs::read_dir(&cycle_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        paths.sort_by(|a, b| {
            let num = |p: &std::fs::DirEntry| -> u32 {
                p.file_name()
                    .to_str()
                    .unwrap_or("")
                    .strip_prefix("cycle_")
                    .unwrap_or("")
                    .strip_suffix(".json")
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(0)
            };
            num(b).cmp(&num(a))
        });

        let nums: Vec<u32> = paths
            .iter()
            .map(|p| {
                p.file_name()
                    .to_str()
                    .unwrap()
                    .strip_prefix("cycle_")
                    .unwrap()
                    .strip_suffix(".json")
                    .unwrap()
                    .parse()
                    .unwrap()
            })
            .collect();
        assert_eq!(nums, vec![3, 2, 1]);
    }
}
