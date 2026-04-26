use axum::Json;
use serde_json::{Value, json};

use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory, as_f64, as_i64, as_str};
use super::routes::resolve_state_root;
use super::subagent::{count_json_records, file_metrics};

// ---------------------------------------------------------------------------
// Memory metrics panel
// ---------------------------------------------------------------------------

pub(crate) async fn memory_metrics() -> Json<Value> {
    let state_root = resolve_state_root();

    let memory_path = state_root.join("memory_records.json");
    let evidence_path = state_root.join("evidence_records.json");
    let goal_path = state_root.join("goal_records.json");
    let handoff_path = state_root.join("latest_handoff.json");

    let memory_info = file_metrics(&memory_path);
    let evidence_info = file_metrics(&evidence_path);
    let goal_info = file_metrics(&goal_path);
    let handoff_info = file_metrics(&handoff_path);

    let fact_count = count_json_records(&memory_path);
    let evidence_count = count_json_records(&evidence_path);
    let goal_count = count_json_records(&goal_path);

    // Query NativeCognitiveMemory (LadybugDB) for live statistics (#419).
    // Capture the error so the dashboard can show *why* data is missing
    // instead of silently returning zeros.
    let native_result =
        NativeCognitiveMemory::open_read_only(&state_root).and_then(|mem| mem.get_statistics());
    let native_error = native_result.as_ref().err().map(|e| e.to_string());
    let native_stats = native_result.ok();

    let last_consolidation = [&memory_path, &evidence_path, &goal_path]
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
            "path": goal_path.to_string_lossy().to_string(),
            "count": goal_count,
            "size_bytes": goal_info.0,
            "modified": goal_info.1,
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
