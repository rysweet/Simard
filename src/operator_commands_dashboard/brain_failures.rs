//! `/api/brain-failures` endpoint — surfaces when and how the OODA brain
//! failed so the operator can see Simard's self-awareness gaps (issue #2043).
//!
//! Data sources:
//!   1. `~/.simard/cycle_reports/cycle_*.json` — `brain_judgments[]` entries
//!      where `fallback == true` or `parse_failure != null`.
//!   2. `~/.simard/metrics/metrics.jsonl` — `brain_parse_failure` metric
//!      entries for a quick summary count.
//!
//! The endpoint returns a flat list of recent brain failures in reverse
//! chronological order, each entry rendered with:
//!   - failure type (parse failure vs deterministic fallback)
//!   - triggering component (act / decide / orient phase)
//!   - timestamp
//!   - whether recovery succeeded (fallback always "recovers" via the
//!     deterministic floor; parse failures that escalated to `gh issue`
//!     are marked as escalated)

use axum::Json;
use serde_json::{Value, json};

use super::routes::resolve_state_root;

/// Maximum number of cycle reports to scan (most recent first).
const MAX_CYCLES_TO_SCAN: usize = 50;

/// Maximum number of failure entries to return.
const MAX_FAILURES_RETURNED: usize = 200;

pub(crate) async fn brain_failures() -> Json<Value> {
    let state_root = resolve_state_root();
    let cycle_dir = state_root.join("cycle_reports");
    let metrics_path = state_root.join("metrics").join("metrics.jsonl");

    let mut failures: Vec<Value> = Vec::new();
    let mut total_fallback_count: u64 = 0;
    let mut total_parse_failure_count: u64 = 0;
    let mut cycles_scanned: u32 = 0;

    // Scan cycle reports for brain judgment failures.
    if let Ok(entries) = std::fs::read_dir(&cycle_dir) {
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

        for entry in paths.into_iter().take(MAX_CYCLES_TO_SCAN) {
            let content = match std::fs::read_to_string(entry.path()) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let report: Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(_) => continue,
            };

            cycles_scanned += 1;
            let cycle_number = report
                .get("cycle_number")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cycle_timestamp = report
                .get("timestamp")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let judgments = match report.get("brain_judgments").and_then(|v| v.as_array()) {
                Some(arr) => arr,
                None => continue,
            };

            for j in judgments {
                let is_fallback = j.get("fallback").and_then(|v| v.as_bool()).unwrap_or(false);
                let has_parse_failure =
                    j.get("parse_failure").is_some() && !j.get("parse_failure").unwrap().is_null();

                if !is_fallback && !has_parse_failure {
                    continue;
                }

                if is_fallback {
                    total_fallback_count += 1;
                }
                if has_parse_failure {
                    total_parse_failure_count += 1;
                }

                if failures.len() >= MAX_FAILURES_RETURNED {
                    continue; // keep counting but stop collecting
                }

                let phase = j.get("phase").and_then(|v| v.as_str()).unwrap_or("unknown");
                let decision = j.get("decision").and_then(|v| v.as_str()).unwrap_or("");
                let rationale = j.get("rationale").and_then(|v| v.as_str()).unwrap_or("");
                let confidence = j.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);

                let failure_type = if has_parse_failure {
                    "parse_failure"
                } else {
                    "deterministic_fallback"
                };

                let failure_description = if has_parse_failure {
                    let pf = j.get("parse_failure").unwrap();
                    let err_msg = pf
                        .get("error_message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown error");
                    let consec = pf
                        .get("consecutive_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let prompt = pf.get("prompt_name").and_then(|v| v.as_str()).unwrap_or("");
                    format!(
                        "The {} phase brain failed to parse a valid response from the language model \
                         (prompt: {}, {} consecutive failure{}). Error: {}",
                        phase,
                        prompt,
                        consec,
                        if consec == 1 { "" } else { "s" },
                        err_msg
                    )
                } else {
                    format!(
                        "The {} phase used its deterministic fallback instead of the language model brain. \
                         Decision: {} (confidence: {:.0}%).",
                        phase,
                        decision,
                        confidence * 100.0
                    )
                };

                let recovery_succeeded = true; // fallback always recovers
                let escalated = if has_parse_failure {
                    let pf = j.get("parse_failure").unwrap();
                    let consec = pf
                        .get("consecutive_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    consec >= 3 // ISSUE_ESCALATION_THRESHOLD
                } else {
                    false
                };

                let timestamp = if has_parse_failure {
                    j.get("parse_failure")
                        .and_then(|pf| pf.get("timestamp"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(cycle_timestamp)
                } else {
                    cycle_timestamp
                };

                failures.push(json!({
                    "failure_type": failure_type,
                    "failure_type_plain": if has_parse_failure {
                        "Brain could not understand model response"
                    } else {
                        "Brain used safe fallback rules instead of model"
                    },
                    "description": failure_description,
                    "phase": phase,
                    "phase_plain": match phase {
                        "act" => "Act — deciding what to do with a running engineer",
                        "decide" => "Decide — choosing which action to take for a goal",
                        "orient" => "Orient — ranking goal urgency after failures",
                        _ => "Unknown phase",
                    },
                    "decision": decision,
                    "rationale": rationale,
                    "confidence": confidence,
                    "cycle_number": cycle_number,
                    "timestamp": timestamp,
                    "recovery_succeeded": recovery_succeeded,
                    "escalated": escalated,
                    "parse_failure_detail": if has_parse_failure {
                        j.get("parse_failure").cloned()
                    } else {
                        None
                    },
                }));
            }
        }
    }

    // Quick count from metrics.jsonl for the summary stat.
    let metrics_parse_failure_count = count_brain_parse_failure_metrics(&metrics_path);

    Json(json!({
        "failures": failures,
        "summary": {
            "total_fallback_count": total_fallback_count,
            "total_parse_failure_count": total_parse_failure_count,
            "metrics_parse_failure_count": metrics_parse_failure_count,
            "cycles_scanned": cycles_scanned,
        },
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

/// Count `brain_parse_failure` metric entries in `metrics.jsonl`.
fn count_brain_parse_failure_metrics(path: &std::path::Path) -> u64 {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    content
        .lines()
        .filter(|line| line.contains("brain_parse_failure"))
        .count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_brain_parse_failure_metrics_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.jsonl");
        std::fs::write(&path, "").unwrap();
        assert_eq!(count_brain_parse_failure_metrics(&path), 0);
    }

    #[test]
    fn count_brain_parse_failure_metrics_counts_matching_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.jsonl");
        std::fs::write(
            &path,
            r#"{"name":"ooda_cycle","value":1}
{"name":"brain_parse_failure","value":1}
{"name":"brain_parse_failure","value":1}
{"name":"other_metric","value":42}
"#,
        )
        .unwrap();
        assert_eq!(count_brain_parse_failure_metrics(&path), 2);
    }

    #[test]
    fn count_brain_parse_failure_metrics_missing_file() {
        let path = std::path::Path::new("/nonexistent/metrics.jsonl");
        assert_eq!(count_brain_parse_failure_metrics(path), 0);
    }
}
